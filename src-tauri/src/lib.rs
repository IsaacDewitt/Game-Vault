mod commands;
mod core;
mod models;
mod utils;

use std::sync::{Arc, Condvar, Mutex, atomic::{AtomicBool, Ordering}};
use tauri::{Emitter, Manager};
use tauri::tray::{TrayIconBuilder, TrayIconEvent, MouseButtonState, MouseButton};
use tauri::menu::{Menu, MenuItem};
use tauri::webview::WebviewWindowBuilder;

/// 后台轮询线程的唤醒信号。
///
/// 空闲期（无活跃会话也无待命会话）线程阻塞在此等待，`launch_game` 插入会话后
/// 调 `notify()` 立刻叫醒它——因此「点击启动 → 开始找进程」之间没有等待空窗，
/// 不必靠缩短空闲心跳来换取响应速度。
///
/// 用「标志位 + 条件变量」而不是裸 `Condvar`：通知方先置位再 signal，
/// 等待方检查标志位后才睡，这样即使 signal 早于 wait 到达（线程正在跑上一轮、
/// 尚未进入等待）也不会丢通知——裸 Condvar 的经典竞态。
pub(crate) struct PollWakeup {
    signaled: Mutex<bool>,
    cv: Condvar,
}

impl PollWakeup {
    pub(crate) fn new() -> Self {
        Self {
            signaled: Mutex::new(false),
            cv: Condvar::new(),
        }
    }

    /// 通知后台线程立即醒一轮
    pub(crate) fn notify(&self) {
        let mut guard = self.signaled.lock().unwrap_or_else(|e| e.into_inner());
        *guard = true;
        self.cv.notify_one();
    }

    /// 等待最多 `timeout`；期间被 `notify` 则立即返回
    pub(crate) fn wait(&self, timeout: std::time::Duration) {
        let mut guard = self.signaled.lock().unwrap_or_else(|e| e.into_inner());
        if !*guard {
            guard = match self.cv.wait_timeout(guard, timeout) {
                Ok((g, _)) => g,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
        *guard = false;
    }
}

/// 双写日志：同时输出到 stderr（开发时终端可见）与日志文件（正式版可排查）
struct MultiLogWriter {
    /// 日志文件句柄。为 `None` 表示文件出口不可用（目录不可写 / 磁盘满 / 权限被拦），
    /// 此时只写 stderr —— 见 `init_logging` 的降级说明。
    file: Option<Arc<std::sync::Mutex<std::fs::File>>>,
}

impl std::io::Write for MultiLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write(buf);
        match &self.file {
            Some(file) => file.lock().unwrap_or_else(|e| e.into_inner()).write(buf),
            // 没有文件出口时报"全部写入成功"，免得 tracing 反复报错刷屏
            None => Ok(buf.len()),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::stderr().flush();
        if let Some(file) = &self.file {
            file.lock().unwrap_or_else(|e| e.into_inner()).flush()?;
        }
        Ok(())
    }
}

/// 初始化日志输出到终端 + 日志文件。
///
/// **日志文件建不出来时不再 panic**（2026-09-23 改）：目录不可写、磁盘满、权限被拦，
/// 都不该让应用起不来。旧实现的 `expect("无法创建日志文件")` 会让进程直接消失，而 release
/// 版是 `windows_subsystem = "windows"`、没有控制台，用户只看到"双击没反应"。
/// 现在降级为只写 stderr，应用照常启动。
fn init_logging() {
    // 日志文件：%APPDATA%/GameVault/logs/gamevault.log
    let log_dir = utils::path::get_app_data_dir().join("logs");
    let _ = std::fs::create_dir_all(&log_dir);

    let log_path = log_dir.join("gamevault.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();

    if log_file.is_none() {
        // 此处**不能用 tracing**（subscriber 尚未 install），只能直接写 stderr
        eprintln!(
            "[GameVault] 警告：无法创建日志文件 {}（目录不可写？），本次运行日志仅输出到 stderr",
            log_path.display()
        );
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_writer(Mutex::new(MultiLogWriter {
            file: log_file.map(|f| Arc::new(Mutex::new(f))),
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

/// 启动期致命错误：尽力留痕后交由调用方决定退出。
///
/// 为什么不能用 `expect`：release 版没有控制台，而启动期的致命错误（数据目录建不出来、
/// 数据库打不开）恰恰发生在**日志系统可能尚未就绪**的时候 —— `expect` 的 panic 信息会
/// 彻底沉没，用户只看到"双击没反应"。这里同时走三条路：日志、stderr、系统消息框。
fn fatal_init_error(context: &str, detail: &str) {
    let message = format!("{context}：{detail}");
    tracing::error!("[启动失败] {message}");
    eprintln!("[GameVault 启动失败] {message}");
    show_error_message_box(&message);
}

/// 弹一个阻塞式系统错误框。
///
/// 启动早期还没有任何窗口，也谈不上前端 toast —— 只能直接走 Win32。这是"让用户至少
/// 知道发生了什么"的最后一道防线。
#[cfg(windows)]
fn show_error_message_box(message: &str) {
    use windows::core::HSTRING;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    let text = HSTRING::from(message);
    let caption = HSTRING::from("Game Vault 启动失败");
    // SAFETY: 两个 HSTRING 在本次调用期间一直存活；未指定 owner 窗口（此时尚无窗口）。
    unsafe {
        let _ = MessageBoxW(None, &text, &caption, MB_OK | MB_ICONERROR);
    }
}

#[cfg(not(windows))]
fn show_error_message_box(message: &str) {
    eprintln!("[GameVault 启动失败] {message}");
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

/// 把主窗口唤醒到最前（「二次双击 exe」/「托盘单击」/「托盘菜单显示」三处共用）。
///
/// 旧实现只有 `show() + set_focus()`，2026-09-17 实弹复现「双击 exe 全无反应」，原因有三：
///
/// 1. **`show()` 救不了最小化**：最小化不是隐藏，`ShowWindow(SW_SHOW)` 不会还原它，必须显式
///    `unminimize()`（`SW_RESTORE`）。探针实测窗口 `iconic` 恒为 True —— 用户点过最小化按钮后
///    再双击 exe，旧实现到此就断了。
/// 2. **`set_focus()` 会被 Windows 前台锁拒绝**：底层 `SetForegroundWindow` 只放行前台进程
///    （或被前台进程启动者），而执行它的是**后台已运行的老进程**，不满足条件，调用被静默拒绝
///    （返回值仍可能为 Ok，无法靠返回值判断）。窗口还原了也跳不到最前，只会让任务栏按钮闪一下。
///    兜底：置顶（TOPMOST）**不受前台锁约束**，短暂置顶即可把窗口强行推到 Z 序顶端，抢到后
///    立刻撤销，不留副作用。
/// 3. **tao 的窗口操作有「标志位缓存 + 异步投递」**（tao-0.35.3 `set_focus()` / `window_state.rs`）：
///    `set_focus` 的前置条件是 `is_visible && !is_minimized`（读 **tao 内部标志**，非 Win32 实时
///    状态），而 `set_visible()` 是 `execute_in_thread` 投递。若从非事件循环线程调用，`show()`
///    还在排队、`set_focus()` 就读到过期的「不可见」标志而被**整段跳过** —— 窗口显示了却抢不到
///    前台。故本函数把唤醒动作整体投递到主线程执行，使 `show/unminimize` 在事件循环线程上同步
///    生效，消除竞态。
///
/// 策略：常规路径零副作用（`set_focus`），以 **Win32 实际前台窗口**校验；只有确认没抢到才抖置顶，
/// 抖动前记下原置顶态、事后精确恢复。
fn focus_main_window(app: &tauri::AppHandle) {
    let handle = app.clone();
    // 见上文第 3 点：投递到主线程，保证 tao 标志位与 Win32 状态同步
    if app.run_on_main_thread(move || wake_main_window(&handle)).is_err() {
        tracing::warn!("[唤醒] 无法投递到主线程（事件循环已退出？），就地尝试");
        wake_main_window(app);
    }
}

/// 唤醒的实际动作，必须在事件循环线程上执行（由 `focus_main_window` 投递）。
fn wake_main_window(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        tracing::warn!("[唤醒] 未找到主窗口（label=main），放弃");
        return;
    };

    let mut used_on_top_fallback = false;
    // 两轮：正常一轮即够；留一轮兜住「还原动画未结束 / 刚被别的窗口抢走前台」的偶发情况
    for attempt in 0..2 {
        // ① Win32 直达还原：**不经过 tao 的标志位缓存**。
        //    tao 的 show() 走 `apply_diff`，标志无变化时直接 early return、根本不下发
        //    ShowWindow；一旦 tao 内部标志与窗口真实状态不一致（外部工具/注入式覆盖层
        //    改过窗口状态即可造成），标准的 show() 会变成空操作，窗口永远弹不出来。
        //    这里以 Win32 实时状态为准兜底，确保「可见」这一硬目标一定达成。
        force_show_window(&window);
        // ② Tauri 侧同步（同时把 tao 的标志位拉回与真实状态一致）
        let _ = window.unminimize();
        let _ = window.show();
        // ③ 常规抢前台：无副作用
        let _ = window.set_focus();
        if is_foreground(&window) {
            break;
        }

        // ④ 前台锁兜底：置顶不受 SetForegroundWindow 的前台限制约束
        used_on_top_fallback = true;
        let was_on_top = window.is_always_on_top().unwrap_or(false);
        let _ = window.set_always_on_top(true);
        let _ = window.set_focus();
        let _ = window.set_always_on_top(was_on_top);
        if is_foreground(&window) {
            break;
        }
        if attempt == 0 {
            // 仅在真失败时等一小会儿再重试（唤醒场景下阻塞几十毫秒无感）
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
    }

    tracing::info!(
        "[唤醒] 主窗口已唤醒: 前台={} 置顶兜底={} 最小化={} 可见={}",
        is_foreground(&window),
        used_on_top_fallback,
        window.is_minimized().unwrap_or(false),
        window.is_visible().unwrap_or(false),
    );
}

/// 以 Win32 实时状态为准，强制把窗口变得可见且非最小化。
///
/// 存在的理由：tao 的 `show()`/`unminimize()` 都是「先改内部标志位、再按 diff 决定是否调用
/// Win32」的写法（见 `window_state.rs` 的 `apply_diff`：`if diff == empty { return }`）。
/// 只要 tao 标志与窗口真实状态不一致——例如被外部工具改动过窗口状态——标准的 `show()` 就
/// 会变成**空操作**，窗口永远弹不出来。此处直接查 `IsIconic`/`IsWindowVisible` 真实状态并
/// 下发 `SW_RESTORE`/`SW_SHOW`，绕开标志缓存，保证「看得见」这个硬目标必达。
///
/// 非 Windows 平台无此隐患（也是 `is_foreground` 之外的唯一平台相关代码），直接返回。
#[cfg(target_os = "windows")]
fn force_show_window(window: &tauri::WebviewWindow) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible, ShowWindow, SW_RESTORE, SW_SHOW};
    let Ok(handle) = window.hwnd() else {
        return;
    };
    let hwnd = HWND(handle.0);
    unsafe {
        if IsIconic(hwnd).as_bool() {
            // 最小化：SW_RESTORE 一步还原并激活
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else if !IsWindowVisible(hwnd).as_bool() {
            // 仅被隐藏（关闭到托盘）：SW_SHOW 显示（不激活，激活交给后续 set_focus）
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        // 已可见且非最小化：什么都不做，保持零副作用
    }
}

#[cfg(not(target_os = "windows"))]
fn force_show_window(_window: &tauri::WebviewWindow) {}

/// 窗口是否就是当前前台窗口。
///
/// 刻意查 **Win32 的 `GetForegroundWindow`** 而非 tao 的 `is_focused()`：后者读的是 tao 内部
/// 标志，需等窗口处理完 `WM_ACTIVATE` 才更新，`force_window_active` 刚调用完时可能仍为 false，
/// 会造成「明明成功却误判失败」而多抖一次置顶。
#[cfg(target_os = "windows")]
fn is_foreground(window: &tauri::WebviewWindow) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    let Ok(handle) = window.hwnd() else {
        return false;
    };
    unsafe { GetForegroundWindow().0 as isize == handle.0 as isize }
}

#[cfg(not(target_os = "windows"))]
fn is_foreground(window: &tauri::WebviewWindow) -> bool {
    window.is_focused().unwrap_or(false)
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
            // 退出前释放截图温会话（常驻 WGC 会话持有捕获线程与 D3D 资源，
            // 不显式停掉会让进程退出时留下悬空线程）
            core::capture::release_warm_session();
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

/// 前端诊断信息落日志（如视口看门狗检测到 WebView2 渲染进程丢失 resize 并自愈的记录）。
///
/// 该类竞态在应用侧无任何异常痕迹，唯有此处主动落盘，事后才有据可查。
#[tauri::command]
fn log_frontend_diag(message: String) {
    tracing::warn!("[前端诊断] {message}");
}

/// 退出应用程序（优雅关闭后台线程，持久化活跃会话）
#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    graceful_exit(&app);
}

/// 初始化应用
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // panic hook **必须先于日志系统安装**（2026-09-23 调整顺序）。
    // 理由：日志初始化本身要建目录、开文件，万一在那里发生意外 panic，而 hook 尚未就绪，
    // release 版（无控制台）就会彻底静默 —— 正是我们要消除的那种"双击没反应"。
    // 先装 hook，最差情况信息也能落到 stderr 与日志（tracing 在 subscriber 未 install
    // 时是 no-op，不会因顺序而报错）。
    install_panic_hook();
    // 初始化日志输出到终端 + 日志文件（失败时降级为只写 stderr，不再 panic）
    init_logging();

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
            // 用户再次双击 game-vault.exe：不新开实例，把已有窗口唤到最前
            tracing::info!("[唤醒] 检测到第二个实例启动，唤醒已有窗口");
            focus_main_window(app);
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
            //
            // 这两步失败即无法继续，但不该以 `expect` 静默带走进程（见 fatal_init_error 注释）：
            // 记日志 + 弹系统消息框 + 把错误上抛给 Tauri run()，让用户至少知道发生了什么。
            let db_path = utils::path::get_database_path();
            let parent_dir = db_path.parent().ok_or_else(|| anyhow::anyhow!("无法获取数据库目录"))?;
            if let Err(e) = utils::path::ensure_dir_exists(parent_dir) {
                fatal_init_error(
                    "无法创建数据目录",
                    &format!("{}（{}）", parent_dir.display(), e),
                );
                return Err(format!("无法创建数据目录: {}", e).into());
            }

            let t_db = std::time::Instant::now();
            let db = match core::Database::new(&db_path) {
                Ok(db) => db,
                Err(e) => {
                    fatal_init_error(
                        "无法初始化数据库",
                        &format!("{}（{}）", db_path.display(), e),
                    );
                    return Err(format!("无法初始化数据库: {}", e).into());
                }
            };
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

            // 后台轮询线程的唤醒信号：空闲期靠它在「点击启动」时立刻醒来（见 PollWakeup）
            let wakeup = Arc::new(PollWakeup::new());

            // 注册状态
            app.manage(db.clone());
            app.manage(tracker.clone());
            app.manage(wakeup.clone());

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
            let wakeup_thread = wakeup.clone();

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
                    // 两种节奏（2026-09-19）：
                    // - 有活（活跃会话或待命会话）→ 1 秒一拍，检测要快；
                    // - 空闲 → 阻塞等待唤醒，10 秒兜底（与旧版开销持平，不因改 1s 而变差）。
                    // 待命会话（平台游戏刚发起启动请求）也必须算「有活」，
                    // 否则它永远等不到转正的机会。
                    let busy = {
                        let tracker = match tracker_arc.lock() {
                            Ok(guard) => guard,
                            Err(poisoned) => poisoned.into_inner(),
                        };
                        tracker.has_pending_work()
                    };

                    if busy {
                        std::thread::sleep(std::time::Duration::from_secs(
                            utils::constants::PROCESS_POLL_INTERVAL_SECS,
                        ));
                    } else {
                        // 空闲期睡到「点击启动」把它叫醒，或 10 秒兜底。
                        // 醒来后即使仍无活也无妨：check_active_sessions 对空状态零成本。
                        wakeup_thread.wait(std::time::Duration::from_secs(
                            utils::constants::PROCESS_POLL_IDLE_SECS,
                        ));
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
                        // 游戏已退出 → 释放截图温会话。常驻 WGC 会话的意义是"游戏会话期一直挂着"，
                        // 游戏没了就没必要留着，否则捕获线程与句柄会一直挂到下一次截图才发现失效。
                        core::capture::release_warm_session();

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

                    // 阶段 3：前台是被追踪的游戏时，后台预建截图温会话。
                    // 有了它，连"会话里的第一张截图"也是温的（~11ms），不必先付一次建会话的 ~170ms。
                    commands::screenshots::maybe_prime_warm_session(&tracker_arc);
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
                let (hotkey, gamepad_hotkey) = {
                    let db_guard = db.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    match models::settings::Settings::load_from_db(&db_guard) {
                        Ok(s) => (s.screenshot_hotkey, s.screenshot_gamepad_hotkey),
                        Err(_) => ("F12".to_string(), String::new()),
                    }
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

                // ---- 手柄截图键通道（XInput 轮询，2026-09-22）----
                // 上面两条键盘通道的前提都是"手在键盘上"；用手柄玩时够不到键盘，故再挂一条。
                // 命中后调用的是**同一个截图入口**，所以前台匹配、Steam 静默退让、快门音、
                // 落盘目录全部与键盘通道一致；差异与边界详见 core/gamepad.rs 的文件头。
                {
                    let gp_app = app.handle().clone();
                    let gp_db = db.clone();
                    let gp_tracker = tracker.clone();
                    // 省电口径（老爷 2026-09-22 定）：只在"本库有游戏会话在跑"时才读手柄。
                    // 用 has_pending_work 而不是只看活跃会话——平台游戏刚发起启动时只有待命会话。
                    let gate_tracker = tracker.clone();
                    let notice_app = app.handle().clone();

                    if let Err(e) = core::gamepad::install(
                        move || {
                            // 【约束】轮询线程上执行，只投递（该线程以 125Hz 节奏跑，不能在这里做重活）
                            commands::screenshots::dispatch_screenshot_async(
                                gp_app.clone(),
                                gp_db.clone(),
                                gp_tracker.clone(),
                                "手柄",
                            );
                        },
                        move || {
                            gate_tracker
                                .lock()
                                .map(|t| t.has_pending_work())
                                .unwrap_or(false)
                        },
                        move |notice| {
                            // 录制结果回给设置页：成功带着组合键、失败带着原因
                            let (event, payload) = match notice {
                                core::gamepad::Notice::Recorded(spec) => {
                                    ("gamepad-hotkey-recorded", spec)
                                }
                                core::gamepad::Notice::Failed(reason) => {
                                    ("gamepad-hotkey-record-failed", reason)
                                }
                            };
                            let _ = notice_app.emit(event, payload);
                        },
                    ) {
                        tracing::error!("启动手柄截图通道失败: {e}（键盘截图不受影响）");
                    }

                    core::gamepad::set_spec(core::gamepad::parse_hotkey(&gamepad_hotkey));
                    match core::gamepad::current_spec() {
                        Some(mask) => tracing::info!(
                            "手柄截图键已就绪: {}（掩码 0x{mask:04X}）",
                            core::gamepad::format_hotkey(mask)
                        ),
                        None if gamepad_hotkey.trim().is_empty() => {
                            tracing::info!("手柄截图键未设置，手柄通道待命（无游戏时不读手柄）")
                        }
                        None => tracing::warn!(
                            "手柄截图键 {gamepad_hotkey} 无法解析为键位，手柄通道停用"
                        ),
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
                        "show" => focus_main_window(app),
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
                        focus_main_window(tray.app_handle());
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
            commands::games::preview_saves_backup,
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
            commands::screenshots::start_gamepad_hotkey_recording,
            commands::screenshots::cancel_gamepad_hotkey_recording,
            // 应用
            quit_app,
            frontend_ready,
            log_frontend_diag,
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
