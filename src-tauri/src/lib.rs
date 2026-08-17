mod commands;
mod core;
mod models;
mod utils;

use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use tauri::{Emitter, Manager};
use tauri::tray::{TrayIconBuilder, TrayIconEvent, MouseButtonState, MouseButton};
use tauri::menu::{Menu, MenuItem};

/// 双写日志：同时输出到 stderr（开发时终端可见）与日志文件（正式版可排查）
struct MultiLogWriter {
    file: Arc<std::sync::Mutex<std::fs::File>>,
}

impl std::io::Write for MultiLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write(buf);
        self.file.lock().unwrap_or_else(|e| e.into_inner()).write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::stderr().flush();
        self.file.lock().unwrap_or_else(|e| e.into_inner()).flush()
    }
}

/// 初始化日志输出到终端 + 日志文件
fn init_logging() {
    // 日志文件：%APPDATA%/GameVault/logs/gamevault.log
    let log_dir = utils::path::get_app_data_dir().join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("gamevault.log"))
        .expect("无法创建日志文件");

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_writer(Mutex::new(MultiLogWriter {
            file: Arc::new(Mutex::new(log_file)),
        }))
        .with_ansi(false)
        .init();
}

/// 优雅退出：通知后台线程、持久化活跃会话、退出进程
fn graceful_exit(app: &tauri::AppHandle) {
    // 通知后台监控线程退出（Release 保证写入对后台线程可见）
    {
        let running = app.state::<Arc<AtomicBool>>();
        running.store(false, Ordering::Release);
    }
    // 直接持久化活跃会话，不依赖后台线程（后台线程可能正在 10s sleep 中）
    if let Some(tracker) = app.try_state::<Arc<Mutex<core::PlayTimeTracker>>>() {
        if let Some(db) = app.try_state::<Arc<Mutex<core::Database>>>() {
            let mut tracker_guard = match tracker.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let finished = tracker_guard.force_finish_all();
            drop(tracker_guard);
            if !finished.is_empty() {
                core::PlayTimeTracker::persist_finished_sessions(&db, &finished);
                // 结算最后一批会话对应的成就（静默，退出时不弹通知）
                if let Ok(db_guard) = db.lock() {
                    let _ = core::AchievementEngine::evaluate(&db_guard);
                }
            }
        }
    }
    app.exit(0);
}

/// 退出应用程序（优雅关闭后台线程，持久化活跃会话）
#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    graceful_exit(&app);
}

/// 初始化应用
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 初始化日志输出到终端 + 日志文件
    init_logging();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![]),
        ))
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // 当用户尝试打开第二个实例时，将已有窗口显示到前台
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .setup(|app| {
            // 初始化数据库
            let db_path = utils::path::get_database_path();
            let parent_dir = db_path.parent().ok_or_else(|| anyhow::anyhow!("无法获取数据库目录"))?;
            utils::path::ensure_dir_exists(parent_dir)
                .expect("无法创建数据目录");

            let db = core::Database::new(&db_path)
                .expect("无法初始化数据库");

            let db = Arc::new(Mutex::new(db));

            // 初始化时长追踪器
            let tracker = core::PlayTimeTracker::new();
            let tracker = Arc::new(Mutex::new(tracker));

            // 注册状态
            app.manage(db.clone());
            app.manage(tracker);

            // 成就系统：启动时立即结算存量数据（静默，不弹通知）
            {
                let db_guard = db.lock().unwrap_or_else(|e| e.into_inner());
                match core::AchievementEngine::evaluate(&db_guard) {
                    Ok(events) => {
                        if !events.is_empty() {
                            tracing::info!("成就系统：启动结算解锁 {} 条历史成就", events.len());
                        }
                    }
                    Err(e) => tracing::error!("成就系统启动结算失败: {}", e),
                }
            }

            // 启动后台进程监控（支持优雅退出）
            let app_handle = app.handle().clone();
            let running = Arc::new(AtomicBool::new(true));
            let running_clone = running.clone();

            // 预先克隆 Arc 引用，避免线程内每 10 秒查找一次 state
            let tracker_arc: Arc<Mutex<core::PlayTimeTracker>> = app.state::<Arc<Mutex<core::PlayTimeTracker>>>().inner().clone();
            let db_arc: Arc<Mutex<core::Database>> = app.state::<Arc<Mutex<core::Database>>>().inner().clone();

            std::thread::spawn(move || {
                while running_clone.load(Ordering::Acquire) {
                    std::thread::sleep(std::time::Duration::from_secs(utils::constants::PROCESS_POLL_INTERVAL_SECS));

                    // 快速检查是否有活跃会话；没有则跳过进程扫描，避免空转 CPU 开销
                    {
                        let tracker = match tracker_arc.lock() {
                            Ok(guard) => guard,
                            Err(poisoned) => poisoned.into_inner(),
                        };
                        if tracker.get_active_games().is_empty() {
                            continue;
                        }
                    }

                    // 阶段 1：检查活跃会话，收集已结束的会话数据，然后释放 Tracker 锁
                    let (finished, active) = {
                        match tracker_arc.lock() {
                            Ok(mut tracker) => {
                                let finished = tracker.check_active_sessions();
                                let active = tracker.get_active_games();
                                (finished, active)
                            }
                            Err(poisoned) => {
                                // Mutex 中毒：恢复锁而非放弃
                                let mut tracker = poisoned.into_inner();
                                let finished = tracker.check_active_sessions();
                                let active = tracker.get_active_games();
                                (finished, active)
                            }
                        }
                    };
                    // Tracker 锁已释放

                    // 阶段 2：持久化已结束的会话到数据库（独立获取 DB 锁）
                    if !finished.is_empty() {
                        core::PlayTimeTracker::persist_finished_sessions(&db_arc, &finished);

                        // 会话结束后检测成就（时长/次数类成就），新解锁通过事件通知前端
                        let mut new_unlocks = Vec::new();
                        if let Ok(db_guard) = db_arc.lock() {
                            match core::AchievementEngine::evaluate(&db_guard) {
                                Ok(events) => new_unlocks = events,
                                Err(e) => tracing::error!("成就检测失败: {}", e),
                            }
                        }
                        if !new_unlocks.is_empty() {
                            let _ = app_handle.emit("achievement-unlocked", &new_unlocks);
                        }
                    }

                    // 通知前端
                    for session in &finished {
                        let _ = app_handle.emit("game-stopped", &session.game_id);
                    }

                    if !active.is_empty() {
                        let _ = app_handle.emit("active-games-updated", &active);
                    }
                }
            });

            // 保存 running 标记以便退出时清理
            app.manage(running);

            // 应用保存的窗口大小
            {
                let db_guard = db.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let settings = models::settings::Settings::load_from_db(&db_guard)
                    .unwrap_or_default();
                drop(db_guard);

                if let Some(window) = app.get_webview_window("main") {
                    // 边界检查：保存的窗口尺寸可能来自更大的显示器，防止窗口超出屏幕
                    let clamped = match window.current_monitor() {
                        Ok(Some(monitor)) => {
                            let wa = monitor.work_area();
                            let w = (settings.window_width as i32)
                                .clamp(900, wa.size.width.max(900) as i32) as u32;
                            let h = (settings.window_height as i32)
                                .clamp(600, wa.size.height.max(600) as i32) as u32;
                            tauri::PhysicalSize::new(w, h)
                        }
                        _ => tauri::PhysicalSize::new(settings.window_width, settings.window_height),
                    };
                    let _ = window.set_size(tauri::Size::Physical(clamped));
                }
            }

            // 创建系统托盘
            let show_item = MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().expect("未配置默认窗口图标"))
                .tooltip("Game Vault")
                .menu(&menu)
                .on_menu_event(|app, event| {
                    match event.id.as_ref() {
                        "show" => {
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                        "quit" => {
                            graceful_exit(app);
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            // 拦截窗口关闭事件，通知前端弹出确认对话框
            if let Some(window) = app.get_webview_window("main") {
                let window_clone = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = window_clone.emit("close-requested", ());
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // 游戏相关
            commands::games::get_games,
            commands::games::get_game_detail,
            commands::games::launch_game,
            commands::games::toggle_favorite,
            commands::games::delete_game,
            commands::games::add_game_manual,
            commands::games::refresh_exe_versions,
            commands::games::set_game_cover,
            commands::games::remove_game_cover,
            commands::games::get_all_covers,
            commands::games::fetch_missing_covers,
            commands::games::fetch_missing_game_info,
            commands::games::fetch_cover_options,
            commands::games::set_game_cover_from_url,
            commands::games::fetch_game_info_llm,
            commands::games::read_cover_as_base64,
            commands::games::read_covers_batch_as_base64,
            commands::games::rename_game,
            commands::games::update_exe_path,
            commands::games::export_game_data,
            commands::games::set_game_status,
            commands::games::import_game_data,
            commands::games::get_all_genres,
            commands::games::open_save_path,
            commands::games::update_save_paths,
            commands::games::check_save_paths,
            commands::games::check_save_paths_for_game,
            commands::games::update_game_meta,
            commands::games::export_saves_backup,
            commands::games::import_saves_backup,
            // 统计相关
            commands::stats::get_play_stats,
            commands::stats::get_daily_stats,
            commands::stats::get_overview_stats,
            commands::stats::get_genre_stats,
            commands::stats::get_heatmap_stats,
            commands::stats::get_hourly_stats,
            commands::stats::get_status_stats,
            commands::stats::get_play_sessions,
            // 成就相关
            commands::achievements::get_achievements,
            commands::achievements::check_achievements,
            // 设置相关
            commands::settings::get_settings,
            commands::settings::save_settings,
            commands::settings::get_autostart_enabled,
            commands::settings::set_autostart_enabled,
            commands::settings::set_window_size,
            // 应用
            quit_app,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
