//! 截图相关 Tauri 命令：热键触发的截图入口 + 打开截图文件夹。

use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use tauri::{Emitter, State};
use windows_capture::window::Window;

use crate::core::tonemap::ToneMapPath;
use crate::core::{Database, PlayTimeTracker};
use crate::core::screenshot;
use crate::models::settings::Settings;
use super::lock_or_recover;

/// 截图处理防抖：热键回调改为后台线程异步执行后（避免 WGC 最长 8s 阻塞
/// tauri 主事件循环导致整个应用 UI 假死），用此标志忽略处理期间的重按，
/// 与 Steam 的连按行为一致。
static SHOT_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// 截图触发结果（序列化给前端用于提示）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ScreenshotOutcome {
    /// 成功截图
    Captured {
        path: String,
        process_name: String,
        width: u32,
        height: u32,
        tone_map_path: String,
    },
    /// 没有从本库启动的游戏在运行，忽略
    NoActiveGame,
    /// 前台窗口不是从本库启动的游戏，忽略
    ForegroundMismatch,
    /// 截图失败
    Failed { message: String },
}

fn tone_path_str(p: ToneMapPath) -> &'static str {
    match p {
        ToneMapPath::DirectSrgb => "direct_srgb",
        ToneMapPath::DivideWhiteLevel => "divide_white_level",
        ToneMapPath::Reinhard => "reinhard",
    }
}

/// 热键触发的截图入口（供 lib.rs 的全局热键回调调用）。
///
/// 逻辑（与 Steam 对齐，仅限从本库启动的游戏）：
/// 1. 没有活跃会话（从本库启动的游戏在运行）→ 忽略；
/// 2. 取前台窗口，进程名与活跃会话的 exe 名匹配才截图；
/// 3. WGC 同步抓屏（HDR 源 → tonemap → PNG）。
pub fn trigger_screenshot(
    db: &Arc<Mutex<Database>>,
    tracker: &Arc<Mutex<PlayTimeTracker>>,
) -> Option<ScreenshotOutcome> {
    // 1. 取前台窗口 + 进程名 + PID
    let foreground = match Window::foreground() {
        Ok(w) => w,
        Err(_) => {
            screenshot::play_feedback(screenshot::FeedbackTone::Notice);
            return Some(ScreenshotOutcome::ForegroundMismatch);
        }
    };
    let process_name = match foreground.process_name() {
        Ok(n) => n,
        Err(_) => {
            screenshot::play_feedback(screenshot::FeedbackTone::Notice);
            return Some(ScreenshotOutcome::ForegroundMismatch);
        }
    };
    let pid = match foreground.process_id() {
        Ok(p) => p,
        Err(_) => {
            screenshot::play_feedback(screenshot::FeedbackTone::Notice);
            return Some(ScreenshotOutcome::ForegroundMismatch);
        }
    };
    let hwnd = foreground.as_raw_hwnd() as isize;

    // 2. 匹配活跃会话（从本库启动的游戏）
    {
        let tracker_guard = lock_or_recover(tracker);
        if tracker_guard.get_active_games().is_empty() {
            screenshot::play_feedback(screenshot::FeedbackTone::Notice);
            return Some(ScreenshotOutcome::NoActiveGame);
        }
        if tracker_guard.find_active_game_by_exe(&process_name).is_none() {
            screenshot::play_feedback(screenshot::FeedbackTone::Notice);
            return Some(ScreenshotOutcome::ForegroundMismatch);
        }
    }

    // 3. 读取截图目录设置
    let screenshot_dir = {
        let db_guard = lock_or_recover(db);
        match Settings::load_from_db(&db_guard) {
            Ok(s) => s.screenshot_dir,
            Err(e) => {
                screenshot::play_feedback(screenshot::FeedbackTone::Error);
                return Some(ScreenshotOutcome::Failed { message: format!("读取设置失败: {e}") });
            }
        }
    };

    // 4. WGC 同步抓屏
    match screenshot::capture_and_save(hwnd, &process_name, &screenshot_dir) {
        Ok(r) => {
            screenshot::play_feedback(screenshot::FeedbackTone::Success);
            Some(ScreenshotOutcome::Captured {
                path: r.path,
                process_name: r.process_name,
                width: r.width,
                height: r.height,
                tone_map_path: tone_path_str(r.tone_map_path).to_string(),
            })
        }
        Err(e) => {
            // 【关键】必须记日志。之前 WGC 失败只发前端 toast，而截图时焦点
            // 在游戏上根本看不到，日志里也一片空白 —— 排查时线索彻底断掉。
            tracing::error!("WGC 截图失败 (pid {pid}, process {process_name}): {e}");
            screenshot::play_feedback(screenshot::FeedbackTone::Error);
            Some(ScreenshotOutcome::Failed { message: e.to_string() })
        }
    }
}

/// 注册全局截图热键并绑定处理器（on_shortcut 同时完成注册 + 绑定）。
/// 供启动时与设置变更时复用。
pub fn register_screenshot_hotkey(
    app: &tauri::AppHandle,
    hotkey: &str,
    db: Arc<Mutex<Database>>,
    tracker: Arc<Mutex<PlayTimeTracker>>,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    if hotkey.trim().is_empty() {
        return Ok(());
    }
    app.global_shortcut()
        .on_shortcut(hotkey, move |app_handle, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                // 上一张还在处理中（WGC 冷启动 + tonemap + PNG 编码可达数秒），
                // 忽略本次按键
                if SHOT_IN_FLIGHT.swap(true, Ordering::SeqCst) {
                    return;
                }
                let db = db.clone();
                let tracker = tracker.clone();
                let app_handle = app_handle.clone();
                // 【关键】独立线程执行：trigger_screenshot 里的 WGC 抓帧
                // （最长 8s 超时）+ tonemap + PNG 编码都是秒级阻塞操作，
                // 全局热键回调跑在 tauri 主事件循环线程上，直接执行会让
                // 整个应用 UI 假死。
                std::thread::spawn(move || {
                    if let Some(outcome) = trigger_screenshot(&db, &tracker) {
                        let _ = app_handle.emit("screenshot-taken", &outcome);
                    }
                    SHOT_IN_FLIGHT.store(false, Ordering::SeqCst);
                });
            }
        })
        .map_err(|e| e.to_string())
}

/// 打开某个游戏的截图文件夹（在文件管理器中）
/// 目录不存在时创建；若游戏无 exe_name 则退回截图根目录。
#[tauri::command]
pub fn open_screenshot_dir(
    db: State<'_, Arc<Mutex<Database>>>,
    app_handle: tauri::AppHandle,
    game_id: String,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let (exe_name, screenshot_dir) = {
        let db_guard = lock_or_recover(&db);
        let game = db_guard
            .get_game_by_id(&game_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "游戏不存在".to_string())?;
        let settings = Settings::load_from_db(&db_guard).map_err(|e| e.to_string())?;
        (game.exe_name, settings.screenshot_dir)
    };

    // 计算目标目录：有 exe_name 则打开 {截图目录}/{进程名}，否则打开截图根目录
    let target = match exe_name {
        Some(name) => screenshot::screenshot_dir_for_process(&screenshot_dir, &name),
        None => std::path::PathBuf::from(crate::utils::path::expand_env_vars(&screenshot_dir)),
    };

    // 目录不存在则创建（保证"打开"总能成功）
    std::fs::create_dir_all(&target).map_err(|e| format!("创建截图目录失败: {e}"))?;

    app_handle
        .opener()
        .open_path(target.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("打开截图文件夹失败: {e}"))?;

    Ok(())
}

/// 查询截图热键注册状态：返回 None 表示注册成功，Some(err) 表示注册失败原因。
/// 前端启动时调用，失败则提示用户快捷键被占用、需更换。
#[tauri::command]
pub fn get_screenshot_hotkey_status(
    hotkey_error: State<'_, Arc<Mutex<Option<String>>>>,
) -> Option<String> {
    hotkey_error
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}
