//! 启动守卫：WebView 创建失败（黑屏）的自愈与可观测性
//!
//! ## 背景
//!
//! `tauri-runtime-wry` 在处理 `Message::CreateWindow` 失败时**只写一行日志**，
//! 不向应用上报（上游 tauri-apps/tauri#11969，至今未解决）。结果是：
//! 窗口 HWND 已经建好并显示，但里面的 WebView 并不存在 —— 窗口内一片空白。
//! 再叠加本项目 `decorations: false`（无边框自绘标题栏），黑屏时连关闭按钮都没有，
//! 用户只能强杀进程重启。
//!
//! 触发该错误（`HRESULT(0x8007139F)` = `ERROR_INVALID_STATE`）的典型场景：
//! 上次退出残留的 `msedgewebview2.exe` 仍占着 WebView2 用户数据目录，
//! 或开机登录风暴期 WebView2 Runtime 尚未就绪 —— 这也解释了「有时才犯、开机必犯」。
//!
//! ## 对策
//!
//! 1. **可观测**：安装 panic hook，把 `Failed to setup app: ...` 这类原本沉没在
//!    无控制台 release 进程里的 panic 写进日志文件。
//! 2. **自愈**：前端挂载成功后主动「报到」；后端看门狗超时未收到报到即判定
//!    WebView 创建失败，自动重启（带次数上限，避免无限循环）。
//! 3. **除因**：重启前清理 WebView2 用户数据目录里残留的 Chromium 单例锁；
//!    并以「运行标记」识别上次异常退出，在下次启动做一次性清理。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tauri::AppHandle;

use crate::utils::constants::{
    BOOT_WATCHDOG_POLL_MS, FRONTEND_READY_TIMEOUT_SECS, MAX_BOOT_RESTART_ATTEMPTS,
};

/// Chromium 系（WebView2 基于 Chromium）在用户数据目录里留下的单例锁文件名。
/// 进程异常终止（强杀 / 崩溃 / 断电）时不会被清除，残留会导致下次启动
/// `CreateCoreWebView2EnvironmentWithOptions` 报 `ERROR_INVALID_STATE`。
const STALE_LOCK_FILES: &[&str] = &[
    "lockfile",
    "SingletonLock",
    "SingletonCookie",
    "SingletonSocket",
];

/// WebView2 用户数据根目录。
///
/// 与 Tauri 默认口径保持一致：`tauri/src/manager/webview.rs` 在 Windows 上会把
/// `data_directory` 兜底设为 `BaseDirectory::LocalData` + `config.identifier`，
/// 即 `%LOCALAPPDATA%\<identifier>`。
fn webview_user_data_dir(identifier: &str) -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join(identifier))
}

/// 运行标记文件：进程启动时写入，正常退出时删除。
/// 下次启动若发现它还在，说明上次是异常退出（强杀/崩溃），据此清理残留锁。
fn session_marker_path() -> PathBuf {
    crate::utils::path::get_app_data_dir()
        .join("logs")
        .join(".boot_session")
}

/// 连续自动重启次数计数器（持久化，进程重启后仍有效）。
fn restart_count_path() -> PathBuf {
    crate::utils::path::get_app_data_dir()
        .join("logs")
        .join(".boot_restart_count")
}

/// 读取累计的连续重启次数（文件缺失/损坏一律视为 0）
fn read_restart_count() -> u32 {
    std::fs::read_to_string(restart_count_path())
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0)
}

fn write_restart_count(n: u32) {
    let path = restart_count_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, n.to_string());
}

/// 前端就绪标志。
///
/// 用全局静态而非 Tauri state：看门狗在 setup 极早期就要开始计时，
/// 若走 `app.manage` + `State<...>` 注入，前端报到与 state 注册之间存在时序竞态。
/// 静态量没有注册步骤，天然规避。
static FRONTEND_READY: AtomicBool = AtomicBool::new(false);

/// 前端挂载成功后调用：置位就绪标志并清零重启计数。
pub fn mark_frontend_ready() {
    if !FRONTEND_READY.swap(true, Ordering::AcqRel) {
        tracing::info!("前端已就绪，黑屏看门狗解除");
    }
    // 清零计数，保证后续偶发黑屏仍有完整的重启额度
    reset_restart_count();
}

/// 前端成功报到后清零计数，使下一次偶发黑屏仍能获得完整的重启额度。
fn reset_restart_count() {
    let _ = std::fs::remove_file(restart_count_path());
}

/// 清理 WebView2 用户数据目录里残留的单例锁文件，返回实际删除的数量。
///
/// 只在「判定上次异常退出」或「看门狗即将重启」时调用 —— 正常运行时锁由
/// WebView2 自身持有，贸然删除会让两个实例共用同一用户数据目录。
///
/// 删除失败一律忽略：锁文件可能正被占用（说明无需清理）或权限不足，
/// 此处属于尽力而为的兜底，不应让启动流程因它失败。
fn cleanup_stale_locks(identifier: &str) -> usize {
    let Some(root) = webview_user_data_dir(identifier) else {
        return 0;
    };
    let mut removed = 0usize;
    // WebView2 真正的 Chromium 用户目录是 <root>\EBWebView，
    // 少数版本会把锁直接放在 <root> 下，两处都扫一遍。
    for base in [root.clone(), root.join("EBWebView")] {
        for name in STALE_LOCK_FILES {
            let path = base.join(name);
            if path.exists() && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// 启动最早期调用（在 `tauri::Builder` 构建之前）。
///
/// 动作：识别上次是否异常退出 → 清残留锁 → 写入本次运行标记。
pub fn prepare_launch(identifier: &str) {
    let marker = session_marker_path();

    if marker.exists() {
        let prev = std::fs::read_to_string(&marker).unwrap_or_default();
        tracing::warn!(
            "检测到上次未正常退出（标记时间: {}），清理 WebView2 遗留学生数据",
            prev.trim()
        );
        let removed = cleanup_stale_locks(identifier);
        tracing::info!("WebView2 残留锁清理完成，删除 {} 个文件", removed);
    }

    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let stamp = chrono::Local::now().to_rfc3339();
    if let Err(e) = std::fs::write(&marker, &stamp) {
        tracing::warn!("写入启动标记失败（不影响运行）: {}", e);
    }
}

/// 正常退出路径调用（graceful_exit / RunEvent::Exit 均会走到）。
/// 删除运行标记，使下次启动不再误判为异常退出。
pub fn mark_clean_exit() {
    let marker = session_marker_path();
    if marker.exists() {
        let _ = std::fs::remove_file(&marker);
    }
}

/// 达到重启上限后向用户交代清楚：黑屏窗口留着没有意义，给出原因与建议后退出。
fn report_fatal_failure(app: &AppHandle) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

    let msg = "\
Game Vault 连续多次无法初始化界面：WebView2 创建失败。

这通常是 WebView2 运行时状态异常导致的（错误码 0x8007139F）。

可尝试：
1. 重启系统后再次打开；
2. 打开任务管理器，结束所有 msedgewebview2.exe 与 game-vault.exe 进程后重试；
3. 若仍不行，可在「应用和功能」里修复 Microsoft Edge WebView2 Runtime。

程序即将退出。";

    app.dialog()
        .message(msg)
        .title("Game Vault 启动失败")
        .kind(MessageDialogKind::Error)
        .blocking_show();

    tracing::error!("已弹出启动失败提示，退出进程");
    std::process::exit(1);
}

/// 启动黑屏看门狗线程。
///
/// 必须在本项目自己的 `setup` 回调里、窗口创建之后调用，且不依赖任何已注册的
/// state —— 这样即便后续初始化步骤卡住或失败，看门狗也在正常计时。
///
/// 前端挂载成功后会通过 `mark_frontend_ready` 置位标志。
pub fn start_watchdog(app: AppHandle, identifier: String) {
    std::thread::spawn(move || {
        let timeout = Duration::from_secs(FRONTEND_READY_TIMEOUT_SECS);
        let start = Instant::now();

        // 轮询等待，前端一旦报到立即收工
        while start.elapsed() < timeout {
            if FRONTEND_READY.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(Duration::from_millis(BOOT_WATCHDOG_POLL_MS));
        }

        // 最后再确认一次，避免恰好在超时瞬间报到被误判
        if FRONTEND_READY.load(Ordering::Acquire) {
            return;
        }

        let attempts = read_restart_count() + 1;
        if attempts > MAX_BOOT_RESTART_ATTEMPTS {
            tracing::error!(
                "前端连续 {} 次未在 {} 秒内就绪，放弃自动重启",
                attempts - 1,
                FRONTEND_READY_TIMEOUT_SECS
            );
            report_fatal_failure(&app);
            return;
        }

        write_restart_count(attempts);
        tracing::error!(
            "前端 {} 秒内未就绪，判定 WebView 创建失败（黑屏），自动重启 {}/{}",
            FRONTEND_READY_TIMEOUT_SECS,
            attempts,
            MAX_BOOT_RESTART_ATTEMPTS
        );

        // 残留锁是二次启动再次失败的常见原因，顺手清掉以提高重启成功率
        let removed = cleanup_stale_locks(&identifier);
        if removed > 0 {
            tracing::info!("重启前清理 WebView2 残留锁 {} 个", removed);
        }

        // 从后台线程触发重启：非阻塞，由事件循环处理退出并拉起新进程
        app.request_restart();
    });
}
