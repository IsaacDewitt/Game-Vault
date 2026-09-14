mod commands;
mod core;
mod models;
mod utils;

use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use tauri::{Emitter, Manager};
use tauri::tray::{TrayIconBuilder, TrayIconEvent, MouseButtonState, MouseButton};
use tauri::menu::{Menu, MenuItem};
use tauri::webview::WebviewWindowBuilder;

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

/// 安装 panic hook：把 panic 落进日志文件。
///
/// release 版 `windows_subsystem = "windows"` 没有控制台，panic 默认只写向已失效的
/// stderr —— 例如 WebView 创建失败时 Tauri 抛的 `Failed to setup app: WebView2 error: ...`
/// 会彻底沉没，事后完全无从查证。这里把它落到日志里，让「闪退 / 黑屏」类问题有据可依。
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".to_string());
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<非字符串 panic 负载>".to_string()
        };
        tracing::error!("PANIC @ {} | {}", location, payload);
        default_hook(info);
    }));
}

/// 手动创建主窗口。
///
/// `tauri.conf.json` 中主窗口已设 `create: false`，改由这里显式创建，目的是让
/// WebView 创建失败（WebView2 `0x8007139F`）变成**可捕获的 `Err`**：
/// Tauri 内部 setup 遇到同样情况会 `panic!("Failed to setup app")` 直接带走整个进程，
/// 应用没有任何自愈余地。捕获后交由启动看门狗超时重启。
fn create_main_window(app: &tauri::AppHandle) -> Result<(), String> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .cloned()
        .ok_or_else(|| "未在 tauri.conf.json 中找到 label=main 的窗口配置".to_string())?;

    WebviewWindowBuilder::from_config(app, &config)
        .map_err(|e| format!("构建窗口失败: {e}"))?
        .build()
        .map_err(|e| format!("创建 WebView 失败: {e}"))?;

    Ok(())
}

/// 优雅退出：通知后台线程、持久化活跃会话、清除启动标记、退出进程
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
    // 正常退出，清除启动标记：下次启动便不会误判为「异常退出」而清理残留锁
    core::boot_guard::mark_clean_exit();
    app.exit(0);
}

/// 前端挂载成功的报到入口（黑屏看门狗据此确认 WebView 正常）。
///
/// 前端在 `app.mount()` 成功后调用一次；后端超时未收到即判定 WebView 创建失败并自动重启。
/// 报到成功同时清零重启计数，保证后续偶发黑屏仍有完整的重启额度。
#[tauri::command]
fn frontend_ready() {
    core::boot_guard::mark_frontend_ready();
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
    // 让 panic（尤其 Tauri 内部 setup 失败）不再静默沉没
    install_panic_hook();

    let context = tauri::generate_context!();

    // 注意：启动守卫（清理上次异常退出残留的 WebView2 锁）**不能**放在这里——单实例
    // 插件是在 Builder::build() 内部的插件 setup 里判定的，晚于本行；若在此清理，
    // 用户重复双击启动时，第二个进程会先把**正在运行实例**的 Chromium 单例锁删掉，
    // 再被单实例插件拦截退出。故移至应用 setup 首行（见下方 ---- 0. ----）。
    let app = tauri::Builder::default()
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
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            let t_setup = std::time::Instant::now();

            // ---- 0. 启动守卫：识别上次异常退出 → 清 WebView2 残留锁 → 写本次运行标记 ----
            // 时序要点（2026-09-12 修正）：本回调执行于 Builder::build() 之后的插件 setup
            // 之后（tauri app.rs：插件 initialize 在 build 内，应用 setup 在 run 内），
            // 因此第二个实例已在插件处 process::exit，**不会走到这里**——重复双击启动
            // 不再误删运行中实例的单例锁。仍早于下面的窗口创建，语义不变。
            core::boot_guard::prepare_launch(&app.config().identifier);

            // ---- 1. 数据库（同步必需：前端所有命令都依赖它，必须最先就绪）----
            let db_path = utils::path::get_database_path();
            let parent_dir = db_path.parent().ok_or_else(|| anyhow::anyhow!("无法获取数据库目录"))?;
            utils::path::ensure_dir_exists(parent_dir)
                .expect("无法创建数据目录");

            let t_db = std::time::Instant::now();
            let db = core::Database::new(&db_path)
                .expect("无法初始化数据库");
            tracing::info!("数据库初始化耗时 {} ms", t_db.elapsed().as_millis());

            let db = Arc::new(Mutex::new(db));

            // ---- 2. 手动创建主窗口 ----
            // create:false + 显式创建，让 WebView 创建失败成为可捕获的错误而非 panic
            match create_main_window(app.handle()) {
                Ok(()) => tracing::info!("主窗口创建成功"),
                Err(e) => tracing::error!("主窗口创建失败: {e}（看门狗将在超时后自动重启）"),
            }

            // ---- 3. 启动黑屏看门狗 ----
            // 置于窗口创建之后：只有窗口存在了才有「黑屏」可言，也避免数据库初始化
            // 偏慢时被误判。看门狗不依赖任何 state，仅凭 AppHandle 即可工作。
            core::boot_guard::start_watchdog(
                app.handle().clone(),
                app.config().identifier.clone(),
            );

            // 初始化时长追踪器
            let tracker = core::PlayTimeTracker::new();
            let tracker = Arc::new(Mutex::new(tracker));

            // 注册状态
            app.manage(db.clone());
            app.manage(tracker.clone());

            // 成就系统：启动结算存量数据（静默，不弹通知）
            // 挪到后台线程并稍作延迟 —— 该结算需独占 DB 锁，留在 setup 里既推迟
            // 窗口/托盘/热键就绪，也容易与前端首屏查询抢锁。
            {
                let ach_db = db.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    let db_guard = match ach_db.lock() {
                        Ok(g) => g,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    match core::AchievementEngine::evaluate(&db_guard) {
                        Ok(events) => {
                            if !events.is_empty() {
                                tracing::info!("成就系统：启动结算解锁 {} 条历史成就", events.len());
                            }
                        }
                        Err(e) => tracing::error!("成就系统启动结算失败: {}", e),
                    }
                });
            }

            // 封面体系迁移（2026-09-10，一次性，settings 标记防重跑）：
            // 把现有游戏/手账封面登记进 covers 索引表并补生成缩略图；已移除条目
            // 没图时按同名从手账回挂一张（"删游戏不删图"的历史欠账）。后台跑，不阻塞启动。
            {
                let cover_db = db.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(2500));
                    let db_guard = match cover_db.lock() {
                        Ok(g) => g,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    match core::cover_store::migrate(&db_guard) {
                        Ok(r) if r.indexed + r.relinked > 0 => tracing::info!(
                            "封面索引迁移：登记 {}，回挂 {}，缺文件 {}，失败 {}（{} → {} 字节）",
                            r.indexed, r.relinked, r.missing_file, r.failed, r.bytes_before, r.bytes_after
                        ),
                        Ok(_) => tracing::debug!("封面索引迁移：无需处理"),
                        Err(e) => tracing::error!("封面索引迁移失败: {}", e),
                    }
                });
            }

            // 启动后台进程监控（支持优雅退出）
            let app_handle = app.handle().clone();
            let running = Arc::new(AtomicBool::new(true));
            let running_clone = running.clone();

            // 启动后延迟清理过期游玩明细（一次性后台任务，不阻塞启动）
            // 清理只删明细行，日/时段汇总表与 games 聚合字段不受影响
            {
                let cleanup_db = db.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(
                        utils::constants::SESSION_CLEANUP_DELAY_SECS,
                    ));
                    if let Ok(db_guard) = cleanup_db.lock() {
                        match db_guard.cleanup_expired_sessions(
                            utils::constants::SESSION_RETENTION_DAYS,
                        ) {
                            Ok(n) => {
                                if n > 0 {
                                    tracing::info!("已清理 {} 条超过保留期的游玩明细", n);
                                }
                            }
                            Err(e) => tracing::error!("清理过期游玩明细失败: {}", e),
                        }
                    }
                });
            }

            // 预先克隆 Arc 引用，避免线程内每 10 秒查找一次 state
            let tracker_arc: Arc<Mutex<core::PlayTimeTracker>> = app.state::<Arc<Mutex<core::PlayTimeTracker>>>().inner().clone();
            let db_arc: Arc<Mutex<core::Database>> = app.state::<Arc<Mutex<core::Database>>>().inner().clone();

            std::thread::spawn(move || {
                while running_clone.load(Ordering::Acquire) {
                    std::thread::sleep(std::time::Duration::from_secs(utils::constants::PROCESS_POLL_INTERVAL_SECS));

                    // 快速检查是否有待处理工作（活跃会话或待命会话）；
                    // 两者皆空则跳过进程扫描，避免空转 CPU 开销。
                    // 注意：待命会话（平台游戏刚发起启动请求）也必须纳入判断，
                    // 否则它永远等不到转正的机会。
                    {
                        let tracker = match tracker_arc.lock() {
                            Ok(guard) => guard,
                            Err(poisoned) => poisoned.into_inner(),
                        };
                        if !tracker.has_pending_work() {
                            continue;
                        }
                    }

                    // 阶段 1：检查会话，收集本轮产出，然后释放 Tracker 锁
                    let tick = {
                        match tracker_arc.lock() {
                            Ok(mut tracker) => tracker.check_active_sessions(),
                            Err(poisoned) => {
                                // Mutex 中毒：恢复锁而非放弃
                                let mut tracker = poisoned.into_inner();
                                tracker.check_active_sessions()
                            }
                        }
                    };
                    // Tracker 锁已释放

                    // 阶段 2：持久化已结束的会话到数据库（独立获取 DB 锁）
                    if !tick.finished.is_empty() {
                        core::PlayTimeTracker::persist_finished_sessions(&db_arc, &tick.finished);

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

                    // 平台游戏：待命转正 / 超时未启动，通知前端以便给出反馈
                    if !tick.activated.is_empty() {
                        let _ = app_handle.emit("platform-game-started", &tick.activated);
                    }
                    for game_id in &tick.arm_timeouts {
                        let _ = app_handle.emit("platform-launch-timeout", game_id);
                    }

                    // 通知前端
                    for session in &tick.finished {
                        let _ = app_handle.emit("game-stopped", &session.game_id);
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
                    // 边界检查：保存的窗口尺寸可能来自更大的显示器，防止窗口超出屏幕。
                    // 尺寸语义为逻辑像素（与设置页/tauri.conf.json 同口径），
                    // work_area 是物理像素，需除以显示器缩放比再 clamp。
                    let clamped = match window.current_monitor() {
                        Ok(Some(monitor)) => {
                            let sf = monitor.scale_factor();
                            let wa = monitor.work_area().size;
                            let max_w = ((wa.width as f64) / sf) as i32;
                            let max_h = ((wa.height as f64) / sf) as i32;
                            let w = (settings.window_width as i32).clamp(900, max_w.max(900)) as f64;
                            let h = (settings.window_height as i32).clamp(600, max_h.max(600)) as f64;
                            tauri::LogicalSize::new(w, h)
                        }
                        _ => tauri::LogicalSize::new(
                            settings.window_width as f64,
                            settings.window_height as f64,
                        ),
                    };
                    let _ = window.set_size(tauri::Size::Logical(clamped));
                }
            }

            // 注册全局截图热键（默认 F12，与 Steam 一致；仅从本库启动的游戏运行时生效）
            {
                let hotkey = {
                    let db_guard = db.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    models::settings::Settings::load_from_db(&db_guard)
                        .map(|s| s.screenshot_hotkey)
                        .unwrap_or_else(|_| "F12".to_string())
                };

                // 记录当前热键（供设置修改时重新注册）
                let hotkey_state = Arc::new(Mutex::new(hotkey.clone()));
                app.manage(hotkey_state);

                // 记录热键注册错误（None=成功；Some(err)=失败原因，供前端启动时检测提示）
                let hotkey_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
                app.manage(hotkey_error.clone());

                let mut hotkey_registered = true;
                if let Err(e) = commands::screenshots::register_screenshot_hotkey(
                    app.handle(),
                    &hotkey,
                    db.clone(),
                    tracker.clone(),
                ) {
                    tracing::error!("注册全局截图热键失败: {e}");
                    *hotkey_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
                    hotkey_registered = false;
                } else {
                    tracing::info!("全局截图热键已注册: {hotkey}");
                }

                // ---- 键盘钩子兜底通道（2026-09-14 实测必需）----
                // 全屏游戏在前台时，上面的注册热键**收不到 WM_HOTKEY**（同进程对照热键 F7 同样收不到，
                // 而 WH_KEYBOARD_LL 钩子能稳定看到按键）。故截图必须再挂一条键盘钩子通道，
                // 两条通道由 SHOT_IN_FLIGHT 去重；详见 core/hotkey_hook.rs 的实测记录。
                {
                    let hook_app = app.handle().clone();
                    let hook_db = db.clone();
                    let hook_tracker = tracker.clone();
                    if let Err(e) = core::hotkey_hook::install(move || {
                        // 【约束】钩子回调内只投递，不做日志/抓屏（它同步阻塞全系统输入）
                        commands::screenshots::dispatch_screenshot_async(
                            hook_app.clone(),
                            hook_db.clone(),
                            hook_tracker.clone(),
                            "键盘钩子",
                        );
                    }) {
                        tracing::error!(
                            "安装截图键盘钩子失败: {e}（游戏内截图将不可用；桌面场景不受影响）"
                        );
                    }

                    // 只有注册热键成功（= 该键确实是我们的）才启用钩子键位：
                    // 否则会在"快捷键被别的程序占用"时替别人响应按键（例如 F12 被 Steam 占用）。
                    match (hotkey_registered, core::hotkey_hook::parse_key_spec(&hotkey)) {
                        (true, Some(spec)) => {
                            core::hotkey_hook::set_spec(Some(spec));
                            tracing::info!(
                                "截图键盘钩子已就绪: vk=0x{:02X} ctrl={} shift={} alt={} win={}",
                                spec.vk,
                                spec.ctrl,
                                spec.shift,
                                spec.alt,
                                spec.win
                            );
                        }
                        (true, None) => {
                            core::hotkey_hook::set_spec(None);
                            tracing::warn!(
                                "截图热键 {hotkey} 无法解析为钩子键位，仅注册热键通道生效"
                            );
                        }
                        (false, _) => {
                            core::hotkey_hook::set_spec(None);
                            tracing::warn!(
                                "截图热键 {hotkey} 注册失败（多半被其他程序占用），钩子通道一并停用，\
                                 避免替别的程序响应按键；请在设置里换一个快捷键"
                            );
                        }
                    }
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

            tracing::info!("应用初始化完成，耗时 {} ms", t_setup.elapsed().as_millis());
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
            // 平台（Steam / Epic）扫描与导入
            commands::platform::scan_platform_games,
            commands::platform::import_platform_games,
            // 鉴赏相关
            commands::reviews::get_reviews,
            commands::reviews::get_review_detail,
            commands::reviews::add_review,
            commands::reviews::refresh_review_info,
            commands::reviews::update_review_meta,
            commands::reviews::set_review_rating,
            commands::reviews::set_review_review,
            commands::reviews::set_review_status,
            commands::reviews::set_review_screenshot_dir,
            commands::reviews::delete_review,
            commands::reviews::fetch_review_cover_options,
            commands::reviews::set_review_cover_from_url,
            commands::reviews::import_review_from_game,
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
            commands::settings::save_settings_partial,
            commands::settings::get_autostart_enabled,
            commands::settings::set_autostart_enabled,
            commands::settings::set_window_size,
            // 截图相关
            commands::screenshots::open_screenshot_dir,
            commands::screenshots::get_screenshot_dir,
            commands::screenshots::list_screenshot_dirs,
            commands::screenshots::get_screenshot_hotkey_status,
            // 应用
            quit_app,
            frontend_ready,
        ])
        .build(context)
        .expect("error while building tauri application");

    // 显式接管退出事件：正常退出（含看门狗重启前的退出）清除启动标记，
    // 使下次启动不再把本次判为「异常退出」。强杀/崩溃时标记残留，正好触发清理。
    app.run(|_handle, event| {
        if let tauri::RunEvent::Exit = event {
            core::boot_guard::mark_clean_exit();
        }
    });
}
