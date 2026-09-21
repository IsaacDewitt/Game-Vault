use anyhow::Result;
use std::path::Path;
use crate::models::*;
use super::platform;

/// 启动产出：区分「直启拿到 PID」与「委托客户端启动（拿不到 PID）」
#[derive(Debug)]
pub enum LaunchOutcome {
    /// 本地 exe 直接启动：拿到 PID，可走进程树追踪
    Spawned(u32),
    /// 经 URI 委托 Steam/Epic 客户端启动：**拿不到 PID**，
    /// 必须在 Tracker 侧 armed 待命，靠 install_path 前缀匹配发现进程
    Delegated { uri: String },
}

/// 原样把参数串追加到命令行（不经 shell）
///
/// 刻意用 `raw_arg` 而非 `args()`：`args()` 会按 Rust 自己的转义规则逐参数重排，
/// 而这里要的恰恰是「用户在 bat 里怎么写就怎么传」——引号、空格、`=` 全部由用户负责，
/// 我们不增不减。例如 `-savedir="D:\我的 存档"` 会连引号一起原样送出。
///
/// 因为不经 cmd.exe，`%USERPROFILE%` 这类环境变量**不会展开**，
/// `& | > ^` 也无特殊含义（整串直接交给游戏进程）。
#[cfg(windows)]
fn append_raw_args(cmd: &mut std::process::Command, args: &str) {
    use std::os::windows::process::CommandExt;
    // raw_arg 会在命令行为空时直接追加、否则自带一个空格分隔，无需手工补空格
    cmd.raw_arg(args);
}

/// 非 Windows 平台忽略自定义参数（本项目只出 Windows 便携版，仅为保持可编译）
#[cfg(not(windows))]
fn append_raw_args(_cmd: &mut std::process::Command, _args: &str) {}

/// 游戏启动器
pub struct GameLauncher;

impl GameLauncher {
    /// 启动本地 exe，返回进程 PID 用于后续进程树追踪
    ///
    /// 若该条目填了 `launch_args`（0.8.3），**原样**追加到命令行之后——
    /// 语义等价于用户自己在 bat 里手打那串（如《寂静岭 f》必须的 `-savetouserdir`）。
    pub fn launch(game: &Game) -> Result<u32> {
        let exe_path = game.exe_path.as_ref()
            .ok_or_else(|| anyhow::anyhow!("游戏没有可执行文件路径"))?;

        if !Path::new(exe_path).exists() {
            anyhow::bail!("游戏可执行文件不存在: {}", exe_path);
        }

        let mut cmd = std::process::Command::new(exe_path);

        if let Some(ref install_path) = game.install_path {
            cmd.current_dir(install_path);
        }

        // 原样透传：trim 后非空才追加，空串/None 与旧行为完全一致（不加任何参数）。
        let launch_args = game
            .launch_args
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let Some(args) = launch_args {
            append_raw_args(&mut cmd, args);
        }

        let child = cmd.spawn()?;
        let pid = child.id();

        // 参数一并打进日志：写错了能直接从日志里看出传的是什么
        match launch_args {
            Some(args) => tracing::info!("启动游戏: {} (PID: {}, 参数: {})", game.name, pid, args),
            None => tracing::info!("启动游戏: {} (PID: {})", game.name, pid),
        }
        Ok(pid)
    }

    /// 按平台分流启动
    ///
    /// - `local`：直接 spawn exe（原有行为，PID 可用）；
    /// - `steam` / `epic`：构造协议 URI 交 Windows shell 转给客户端。
    ///
    /// 平台分支**刻意不自己跑 exe**：Steam 有 Steamworks DRM、Epic 有 EOS 校验，
    /// 直接跑 exe 会认证失败或缺失云存档/成就/覆盖层。委托客户端启动后，
    /// 游戏体验与手动从客户端点启动**完全一致**——这正是本方案的核心价值。
    pub fn launch_game(game: &Game) -> Result<LaunchOutcome> {
        match game.platform.as_str() {
            platform::PLATFORM_STEAM | platform::PLATFORM_EPIC => {
                let pid = game.platform_id.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("平台游戏缺少 platform_id，无法构造启动 URI")
                })?;
                let uri = platform::launch_uri(&game.platform, pid)?;
                open_uri(&uri)?;
                tracing::info!(
                    "已委托 {} 客户端启动「{}」: {}",
                    game.platform, game.name, uri
                );
                Ok(LaunchOutcome::Delegated { uri })
            }
            // 未知平台值按本地处理，保证老数据/脏数据不至于无法启动
            _ => Ok(LaunchOutcome::Spawned(Self::launch(game)?)),
        }
    }
}

/// 交给 Windows shell 打开 URI（自定义协议只有 shell 会认）
///
/// 必须走 `ShellExecuteW` 而非 `Command::new`：`steam://` 与
/// `com.epicgames.launcher://` 是注册在 `HKEY_CLASSES_ROOT` 下的协议处理器，
/// 只有 shell 会按注册表把请求转发给对应客户端。
/// （实测注册表：`steam` → `steam.exe -- "%1"`；`com.epicgames.launcher` →
/// `EpicGamesLauncher.exe %1`，整串 URI 作为参数原样传递。）
#[cfg(windows)]
fn open_uri(uri: &str) -> Result<()> {
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let target = HSTRING::from(uri);
    // ShellExecute 的历史约定：返回值 >32 为成功，<=32 是错误码
    let ret = unsafe {
        ShellExecuteW(
            None, // 无父窗口
            w!("open"),
            &target,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    let code = ret.0 as isize;
    if code <= 32 {
        anyhow::bail!("ShellExecuteW 打开 URI 失败（返回码 {}）: {}", code, uri);
    }
    Ok(())
}

#[cfg(not(windows))]
fn open_uri(uri: &str) -> Result<()> {
    anyhow::bail!("仅 Windows 支持协议启动: {}", uri)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// 参数原样追加：raw_arg 挂上去的就是用户写的那一串，不做转义重排。
    /// 只断言「挂上了、内容一致」，不真的 spawn —— 单测不起进程。
    #[test]
    fn raw_args_appended_verbatim() {
        let mut cmd = std::process::Command::new("notepad.exe");
        append_raw_args(&mut cmd, "-savetouserdir");
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(args, vec!["-savetouserdir".to_string()]);
    }

    /// 带引号、空格、`&`、中文路径的整串原样保留：
    /// 不切分、不转义、不因 `&` 被当成管道（这是 raw_arg 相对 args() 的核心价值）
    #[test]
    fn raw_args_keep_quotes_and_symbols() {
        let raw = "-savedir=\"D:\\我的 存档\" & -windowed";
        let mut cmd = std::process::Command::new("notepad.exe");
        append_raw_args(&mut cmd, raw);
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(args, vec![raw.to_string()]);
    }
}
