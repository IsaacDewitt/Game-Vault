mod commands;
mod core;
mod models;
mod utils;

use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use tauri::{Emitter, Manager};

/// 初始化应用
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 初始化日志输出到终端
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
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
            app.manage(db);
            app.manage(tracker);

            // 启动后台进程监控（支持优雅退出）
            let app_handle = app.handle().clone();
            let running = Arc::new(AtomicBool::new(true));
            let running_clone = running.clone();

            std::thread::spawn(move || {
                while running_clone.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_secs(10));

                    let tracker_arc = app_handle.state::<Arc<Mutex<core::PlayTimeTracker>>>().inner().clone();
                    let db_arc = app_handle.state::<Arc<Mutex<core::Database>>>().inner().clone();

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
            commands::games::set_game_cover,
            commands::games::get_all_covers,
            commands::games::fetch_missing_covers,
            commands::games::fetch_game_info_llm,
            commands::games::read_cover_as_base64,
            commands::games::read_covers_batch_as_base64,
            // 统计相关
            commands::stats::get_play_stats,
            commands::stats::get_daily_stats,
            commands::stats::get_overview_stats,
            // 设置相关
            commands::settings::get_settings,
            commands::settings::save_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
