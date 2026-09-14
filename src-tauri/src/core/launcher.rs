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

/// 游戏启动器
pub struct GameLauncher;

impl GameLauncher {
    /// 启动本地 exe，返回进程 PID 用于后续进程树追踪
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

        let child = cmd.spawn()?;
        let pid = child.id();

        tracing::info!("启动游戏: {} (PID: {})", game.name, pid);
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
