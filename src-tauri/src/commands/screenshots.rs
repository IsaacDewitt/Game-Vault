//! 截图相关 Tauri 命令：热键触发的截图入口 + 打开截图文件夹。

use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use tauri::{Emitter, State};
use windows_capture::window::Window;

use crate::core::tonemap::ToneMapPath;
use crate::core::{Database, PlayTimeTracker};
use crate::core::screenshot;
use crate::models::Game;
use crate::models::settings::Settings;
use super::lock_or_recover;

/// 截图处理防抖闸门：截图在独立线程里做（避免 WGC 最长 8s 阻塞 tauri 主事件循环
/// 导致整个应用 UI 假死），用此标志忽略处理期间的重按，与 Steam 的连按行为一致。
///
/// 它同时承担**双通道去重**：桌面场景下键盘钩子与注册热键会在同一瞬间各触发一次，
/// 先到者占闸门截图、后到者被丢弃（见 `dispatch_screenshot_async`）。
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
    /// 该游戏所属平台自带截图能力（Steam），本应用**静默退让**
    ///
    /// 刻意不发提示音：Steam 默认截图键同为 F12，若在 Steam 里按 F12 时
    /// 本应用跟着响一声，会没完没了地打扰用户。
    PlatformExcluded,
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

/// 组装某游戏的**落盘候选名**与**手账指定目录**（写入与查看两条路共用）。
///
/// 【为什么要共用，2026-09-14 实机教训】截图实际写进哪个目录（`trigger_screenshot`）与
/// 界面显示/打开哪个目录（`resolve_screenshot_dir`）必须永远同口径。此前写入侧已改成
/// 「手账指定 → 标题既有目录 → 标题新建」，而查看侧仍按 `games.exe_name` 定位 —— 于是
/// `God of War`（exe 名 `GoW.exe`）出现"图截进了 `God of War`（569 张），详情页却盯着空的
/// `GoW` 显示暂无截图"，用户再次以为没截上。
///
/// 候选顺序即命中优先级：**手账英文名 → 库内题名 → 安装程序名 → 安装目录名**。
/// 后两个是兜底：用户盘上的目录常按其一命名（`GhostOfTsushima.exe`、`Mafia The Old Country`），
/// 而库里若只有中文题名（如《死亡搁浅：导演剪辑版》），中文键永远对不上英文目录。
/// 兜底候选只在"候选目录确实含图片"时才会被采用（见 `screenshot::pick_existing_dir`），
/// 因此不会把 `GoW` 这种空壳目录重新激活。
fn capture_target_inputs(db: &Database, game: &Game) -> (Vec<String>, Option<String>) {
    let review = db
        .find_review_id_by_game_name(&game.name)
        .ok()
        .flatten()
        .and_then(|id| db.get_review_by_id(&id).ok().flatten());

    let journal_dir = review.as_ref().and_then(|r| r.screenshot_dir.clone());

    let mut candidates: Vec<String> = Vec::new();
    if let Some(en) = review
        .as_ref()
        .and_then(|r| r.name_en.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        candidates.push(en.to_string());
    }
    if !game.name.trim().is_empty() {
        candidates.push(game.name.clone());
    }
    if let Some(exe) = game.exe_name.as_deref() {
        let stem = screenshot::process_stem(exe);
        if !stem.is_empty() {
            candidates.push(stem);
        }
    }
    if let Some(install) = game.install_path.as_deref() {
        let trimmed = install.trim_end_matches(['\\', '/']);
        if let Some(base) = std::path::Path::new(trimmed)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
        {
            let base = base.trim();
            if !base.is_empty() {
                if is_generic_dir_name(base) {
                    // 安装路径末级常是 `...\Binaries\Win32` 这类技术目录名，而截图根目录下
                    // 恰有同名目录（用户库里 `Content` 123 张、`Common` 51 张），一旦命中就会把
                    // 图截进无关目录 —— 这类名字不作为候选。
                    tracing::debug!("[截图] 安装目录名 '{}' 属技术性通用名，不作为候选", base);
                } else {
                    candidates.push(base.to_string());
                }
            }
        }
    }
    (candidates, journal_dir)
}

/// 明显是技术性/通用名的目录名（安装路径的末级常见形态），不得当作截图目录候选：
/// 截图根目录是人工整理的，理论上不会有这类名字的**游戏**目录，但可能残留同名目录
/// （实机：`Content` 123 张、`Common` 51 张），误命中就会把截图落进无关目录。
fn is_generic_dir_name(name: &str) -> bool {
    const GENERIC: &[&str] = &[
        "win32", "win64", "windows", "bin", "binaries", "x64", "x86", "x86_64", "content",
        "common", "data", "game", "games", "system", "shipping", "retail", "build", "dist",
        "app", "src", "assets", "engine", "support", "installer",
    ];
    GENERIC.contains(&name.trim().to_ascii_lowercase().as_str())
}

/// 截图执行体（由 `dispatch_screenshot_async` 在独立线程里调用）。
///
/// 逻辑（与 Steam 对齐，仅限从本库启动的游戏）：
/// 1. 取前台窗口 + PID；进程名读取失败时退化为"路径的文件名"（提权进程读不到模块名）；
/// 2. 本库没有任何会话（活跃 + 待命都为 0）→ 忽略；
/// 3. 按「进程名匹配 **或** 进程路径落在安装目录下」反查游戏；活跃会话不中再试**待命**会话
///    （平台游戏进程已现身但 10 秒轮询尚未转正的窗口期，游戏就在眼前，照常截图）；
/// 4. **Steam 游戏静默退让** —— Steam 自带截图、且默认热键同为 F12，必须不响不截；
/// 5. WGC 抓屏（HDR 源 → tonemap → PNG），按**会话 exe 名**归档（与手账/详情页同口径）。
///
/// 每条返回分支都会写日志，见下方"可观测性铁律"。
pub fn trigger_screenshot(
    db: &Arc<Mutex<Database>>,
    tracker: &Arc<Mutex<PlayTimeTracker>>,
) -> Option<ScreenshotOutcome> {
    // 1. 取前台窗口 + PID + 进程名
    //
    // 【可观测性铁律】本函数每条失败分支都必须留下日志。此前 4 条分支只播个提示音就返回，
    // 而提示音在游戏里未必听得见、toast 又藏在应用窗口里，导致"按了 F9 毫无反应"类问题
    // 完全没有线索（排查 Epic 截图失效时被迫用外部探针反推）。任何新增分支同理。
    let foreground = match Window::foreground() {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!("[截图] 取前台窗口失败，放弃: {e:?}");
            screenshot::play_feedback(screenshot::FeedbackTone::Notice);
            return Some(ScreenshotOutcome::ForegroundMismatch);
        }
    };
    // PID 先行：即使后续取名失败，日志里也要有 pid 可追
    let pid = match foreground.process_id() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("[截图] 取前台窗口 PID 失败，放弃: {e:?}");
            screenshot::play_feedback(screenshot::FeedbackTone::Notice);
            return Some(ScreenshotOutcome::ForegroundMismatch);
        }
    };
    let hwnd = foreground.as_raw_hwnd() as isize;

    // 2. 识别前台游戏（取路径 + 取进程名 + 匹配活跃/待命会话，一次持锁完成）
    // 返回值：matched_game_id = 命中的游戏；archive_name = 截图归档用的 exe 名（会话 exe_name，
    // 与手账/详情页同口径），二者与"前台进程名"是三件不同的事，勿混用。
    let (matched_game_id, archive_name) = {
        let mut tracker_guard = lock_or_recover(tracker);

        // 进程完整路径：sysinfo 走 PROCESS_QUERY_LIMITED_INFORMATION，跨完整性级别也能读，
        // 是"进程名对不上"场景（Epic 启动器壳 / Steam 无 exe 名 / 提权游戏）的唯一依据。
        let full_path = tracker_guard.process_path(pid);

        // 进程名：windows-capture 走 GetModuleBaseNameW，需要 PROCESS_VM_READ ——
        // 对提权/受保护进程会返回 ACCESS_DENIED（实测本机 325 个进程里 139 个打不开）。
        // 以前这里直接放弃（静默失配），现改为退化成"路径的文件名"：路径来自 LIMITED 查询，
        // 拿到的同样是 exe 文件名，足以完成匹配与目录命名。
        let process_name = match foreground.process_name() {
            Ok(n) => n,
            Err(e) => {
                let from_path = full_path
                    .as_deref()
                    .and_then(|p| p.rsplit(['\\', '/']).next())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
                match from_path {
                    Some(name) => {
                        tracing::warn!(
                            "[截图] 读进程名失败(pid {pid}: {e:?})，改用路径文件名 {name}"
                        );
                        name
                    }
                    None => {
                        tracing::warn!(
                            "[截图] 读进程名失败且取不到进程路径 (pid {pid}: {e:?})，放弃"
                        );
                        screenshot::play_feedback(screenshot::FeedbackTone::Notice);
                        return Some(ScreenshotOutcome::ForegroundMismatch);
                    }
                }
            }
        };

        let (n_active, n_pending) = tracker_guard.session_counts();
        if n_active == 0 && n_pending == 0 {
            tracing::info!("[截图] 前台 {process_name} (pid {pid})，但本库没有任何会话，忽略");
            screenshot::play_feedback(screenshot::FeedbackTone::Notice);
            return Some(ScreenshotOutcome::NoActiveGame);
        }

        let id = match tracker_guard.find_active_game_by_process(&process_name, full_path.as_deref()) {
            Some(id) => {
                tracing::info!(
                    "[截图] 前台命中游戏 {id}（进程 {process_name} pid {pid} 路径 {full_path:?}）"
                );
                id
            }
            None => {
                // 待命期兜底：平台游戏进程已现身但 10 秒轮询尚未转正时，
                // 用户看到画面就会按截图键——此时进程确凿在跑，照常截图。
                match tracker_guard
                    .find_pending_game_by_process(&process_name, full_path.as_deref())
                {
                    Some(id) => {
                        tracing::info!(
                            "[截图] 前台命中**待命**会话 {id}（进程 {process_name} pid {pid} \
                             路径 {full_path:?}；尚未转正，先截图、不计时）"
                        );
                        id
                    }
                    None => {
                        tracing::warn!(
                            "[截图] 前台进程与会话均不匹配，忽略（进程 {process_name} pid {pid} \
                             路径 {full_path:?}，活跃 {n_active} 个 / 待命 {n_pending} 个）"
                        );
                        screenshot::play_feedback(screenshot::FeedbackTone::Notice);
                        return Some(ScreenshotOutcome::ForegroundMismatch);
                    }
                }
            }
        };

        // 归档命名：以**会话的 exe_name** 建目录（即 games.exe_name），与手账、
        // 游戏详情页定位截图目录的口径完全一致；前台进程名只用于"反查是哪款游戏"。
        // 二者在本地游戏上通常相同；平台游戏则可能不同（Epic 启动器壳场景：LaunchExecutable
        // 往往是 Launcher.exe / PlayRDR2.exe），此时按前台进程名建目录会得到 UI 永远找不到的死目录。
        let archive_name = tracker_guard
            .session_exe_name(&id)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| process_name.clone());
        if !archive_name.eq_ignore_ascii_case(&process_name) {
            tracing::info!(
                "[截图] 归档命名取会话 exe 名 {archive_name}（前台进程为 {process_name}）"
            );
        }
        (id, archive_name)
    };

    // 3. 平台分流 + 解析落盘目标（一次 DB 锁完成）
    //
    // Steam 自带截图，且其默认截图键同为 F12：本应用必须**静默退让**——
    // 不截图、也**不发提示音**，否则每次 Steam 截图都会被我们"跟一声"。
    // 判定必须显式按 platform 来，不能指望"Steam 条目没有 exe 名所以匹配不上"这种巧合——
    // 一旦将来 exe_name 被填上，就会误触发。
    // （Epic 无此冲突：其覆盖层热键是 Shift+F2，故 Epic 游戏照常支持截图。）
    let target = {
        let db_guard = lock_or_recover(db);

        let game = db_guard.get_game_by_id(&matched_game_id).ok().flatten();
        let platform = game
            .as_ref()
            .map(|g| g.platform.clone())
            .unwrap_or_default();
        if platform == crate::core::platform::PLATFORM_STEAM {
            tracing::info!(
                "前台游戏 {} 属 Steam（自带截图，默认键同为 F12），本应用静默退让",
                matched_game_id
            );
            return Some(ScreenshotOutcome::PlatformExcluded);
        }

        let settings = match Settings::load_from_db(&db_guard) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("[截图] 读取截图目录设置失败: {e}");
                screenshot::play_feedback(screenshot::FeedbackTone::Error);
                return Some(ScreenshotOutcome::Failed { message: format!("读取设置失败: {e}") });
            }
        };

        // 手账条目（含其指定的截图目录与英文题名）与候选名统一由 capture_target_inputs 组装，
        // 与「详情页显示 / 打开截图文件夹」走同一套口径。
        let (candidates, journal_dir) = match game.as_ref() {
            Some(g) => capture_target_inputs(&db_guard, g),
            None => (Vec::new(), None),
        };

        let root = std::path::PathBuf::from(crate::utils::path::expand_env_vars(
            &settings.screenshot_dir,
        ));
        let target = screenshot::resolve_capture_target(
            &root,
            &candidates,
            journal_dir.as_deref(),
            // 兜底：会话 exe 名（题名全空时才用得上）
            &archive_name,
        );
        tracing::info!(
            "[截图] 落盘 {}（命名来源：{}；候选名 {:?}）",
            target.dir.display(),
            target.source.label(),
            candidates
        );
        target
    };

    // 4. WGC 抓屏
    match screenshot::capture_and_save(hwnd, &target.dir, &target.stem) {
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
            tracing::error!("[截图] WGC 抓屏失败 (pid {pid}, 归档名 {archive_name}): {e}");
            screenshot::play_feedback(screenshot::FeedbackTone::Error);
            Some(ScreenshotOutcome::Failed { message: e.to_string() })
        }
    }
}

/// 截图触发去重闸门的复位守卫。
///
/// 用 Drop 复位而不是在末尾手写 `store(false)`：`trigger_screenshot` 一旦 panic
/// （抓帧/色调映射/PNG 编码里任何一处越界），手写复位就永远执行不到，
/// 闸门卡在"处理中"会让**两个通道一起永久失效**，用户看到的是"截图键彻底不灵了"。
/// 项目未设 `panic = "abort"`，展开会跑 Drop，故守卫足以兜住。
struct ShotInFlightGuard;

impl Drop for ShotInFlightGuard {
    fn drop(&mut self) {
        SHOT_IN_FLIGHT.store(false, Ordering::SeqCst);
    }
}

/// 截图触发的**唯一入口**（全局热键通道 + 键盘钩子通道共用）。
///
/// 【双通道去重】桌面/窗口化场景下两条通道都会在按下瞬间到达（钩子回调先于 WM_HOTKEY），
/// 由 `SHOT_IN_FLIGHT` 收口：先到者截图，后到者被忽略——效果仍是"一按一张"。
///
/// 【调用约束】`source` 只用于日志；本函数**必须保持轻量**（只做去重与投递）：
/// 键盘钩子回调同步阻塞全系统输入，绝不能在里面做日志、锁或抓屏。
pub fn dispatch_screenshot_async(
    app_handle: tauri::AppHandle,
    db: Arc<Mutex<Database>>,
    tracker: Arc<Mutex<PlayTimeTracker>>,
    source: &'static str,
) {
    if SHOT_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return;
    }
    // 【守卫必须在 spawn 之前构造】`std::thread::spawn` 在线程创建失败时会 panic：
    // 若把 `ShotInFlightGuard` 放在闭包里，闭包就永不执行、守卫永不构造 —— 闸门永久卡在
    // true，全局热键与键盘钩子**两条通道一起失效**，用户看到的是"截图键彻底不灵了"。
    // 守卫提前持有、随闭包 move 进子线程：闭包内 panic、或线程压根没起来（闭包被丢弃），
    // 两种情况都由 Drop 复位。
    let guard = ShotInFlightGuard;
    // 【关键】独立线程执行：trigger_screenshot 里的 WGC 抓帧（最长 8s 超时）
    // + tonemap + PNG 编码都是秒级阻塞操作，热键回调跑在 tauri 主事件循环线程上，
    // 直接执行会让整个应用 UI 假死。
    // 用 `Builder::spawn` 而非 `thread::spawn`：线程创建失败时前者返回 Err（可记日志），
    // 后者直接 panic，而调用方可能是键盘钩子回调 —— 绝不能在那里展开。
    let spawned = std::thread::Builder::new()
        .name("gv-screenshot".to_string())
        .spawn(move || {
            let _guard = guard;
            tracing::info!("[截图] {source} 触发");
            if let Some(outcome) = trigger_screenshot(&db, &tracker) {
                let _ = app_handle.emit("screenshot-taken", &outcome);
            }
        });
    if let Err(e) = spawned {
        // 双保险：Err 时闭包已被丢弃、守卫的 Drop 已复位过；这里再显式复位一次，
        // 保证任何未来的改法都不会让闸门卡死。
        SHOT_IN_FLIGHT.store(false, Ordering::SeqCst);
        tracing::error!("[截图] 无法创建截图线程，本次触发放弃: {e}");
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
        tracing::warn!("截图热键为空，跳过注册");
        return Ok(());
    }
    // 供回调内日志使用（闭包需 'static，故取自有 String）
    let hotkey_owned = hotkey.to_string();
    app.global_shortcut()
        .on_shortcut(hotkey, move |app_handle, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                // 【可观测性铁律】这行是第一手证据：有它 = 注册热键通道生效；
                // 游戏内按了却没有它、而下面"键盘钩子 触发"有记录 = 走的正是兜底通道。
                tracing::info!("[截图] 全局热键 {} 按下", hotkey_owned);
                dispatch_screenshot_async(
                    app_handle.clone(),
                    db.clone(),
                    tracker.clone(),
                    "全局热键",
                );
            }
        })
        .map_err(|e| e.to_string())
}

/// 截图目录信息（供前端展示路径与已有截图数量）
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScreenshotDirInfo {
    /// 解析后的目录路径（已展开环境变量；未创建也可能返回）
    pub path: String,
    /// 该目录当前是否已在磁盘上存在
    pub exists: bool,
    /// 目录下已有的图片文件数量（仅统计当前层，不递归）
    pub count: usize,
    /// 定位来源："manual"(手账手动指定) | "game"(同名游戏/该游戏的 exe) | "tombstone"(游戏已删除，借墓碑) | "root"(截图根目录)
    pub source: String,
}

/// 截图目录的定位来源（给前端标注用）
const SRC_MANUAL: &str = "manual";
const SRC_GAME: &str = "game";
const SRC_TOMBSTONE: &str = "tombstone";
const SRC_ROOT: &str = "root";

/// 解析截图目录，返回 (目录, 来源标记)。
///
/// **与截图落盘同口径**（2026-09-14 统一）：两条路都经由 `capture_target_inputs` +
/// `screenshot::resolve_capture_target`，即「手账指定（空壳除外）→ 候选名命中的既有含图目录 →
/// 按题名新建」，保证"截图写进哪个目录，界面就看哪个目录"。
///
/// - 传 `Some(game_id)`：候选名依次为 手账英文名 → 库内题名 → exe 名 → 安装目录名；
/// - 传 `Some(review_id)`：候选名依次为 手账英文名 → 手账题名 → 同名游戏 exe 名 → 同名墓碑 exe 名
///   （游戏已从游戏库删除、只留墓碑的情形靠最后一项兜底）；
///   候选名与手账指定都取不到时才返回错误，引导用户手动指定目录；
/// - 都不传：直接返回截图根目录（设置页用）。
fn resolve_screenshot_dir(
    db: &Arc<Mutex<Database>>,
    game_id: Option<&str>,
    review_id: Option<&str>,
) -> Result<(std::path::PathBuf, &'static str), String> {
    let db_guard = lock_or_recover(db);
    let settings = Settings::load_from_db(&db_guard).map_err(|e| e.to_string())?;

    let root = || {
        std::path::PathBuf::from(crate::utils::path::expand_env_vars(&settings.screenshot_dir))
    };

    if let Some(review_id) = review_id {
        let review = db_guard
            .get_review_by_id(review_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "手账条目不存在".to_string())?;

        // 候选名与"手账指定目录"按 **截图落盘同一规则** 解析（`resolve_capture_target`）。
        //
        // 旧实现是「手动指定 → 同名游戏 exe 目录 → 同名墓碑 exe 目录」直接把目录返回，
        // 与写入侧的新规则（手账指定 → 标题既有目录 → 标题新建）口径不同：手账指定的目录若已只剩
        // 空壳、或 exe 名目录是空的，界面就会盯着一个永远不会有图的目录，而新截图其实落在按标题
        // 命中的目录里 —— 又是一次"以为没截上"。现在两侧共用同一套解析。
        //
        // 候选名顺序：手账英文名 → 手账题名 → 同名游戏 exe 名 → 同名墓碑 exe 名。
        // （后两者取代了旧实现"直接返回 exe 名目录"的做法：现在必须目录确实含图才算命中。）
        let mut candidates: Vec<String> = Vec::new();
        if let Some(en) = review
            .name_en
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            candidates.push(en.to_string());
        }
        if !review.name.trim().is_empty() {
            candidates.push(review.name.clone());
        }

        let mut has_live_exe = false;
        if let Some(game) = db_guard
            .find_game_by_name(&review.name)
            .map_err(|e| e.to_string())?
        {
            if let Some(exe) = game.exe_name.as_deref() {
                has_live_exe = true;
                let stem = screenshot::process_stem(exe);
                if !stem.is_empty() {
                    candidates.push(stem);
                }
            }
        }

        // 同名活条目不存在（或它没填安装程序名）时，才借同名墓碑 —— 游戏已从库删除但留历史归档
        let mut from_tombstone = false;
        if !has_live_exe {
            if let Some(exe) = db_guard
                .find_tombstone_exe_by_name(&review.name)
                .map_err(|e| e.to_string())?
            {
                let stem = screenshot::process_stem(&exe);
                if !stem.is_empty() {
                    candidates.push(stem);
                    from_tombstone = true;
                }
            }
        }

        if candidates.is_empty() {
            return Err(format!(
                "未找到《{}》的截图目录，可在下方手动指定截图文件夹",
                review.name
            ));
        }

        let target = screenshot::resolve_capture_target(
            &root(),
            &candidates,
            review.screenshot_dir.as_deref(),
            "",
        );
        let source = match target.source {
            screenshot::CaptureDirSource::Journal => SRC_MANUAL,
            _ if from_tombstone => SRC_TOMBSTONE,
            _ => SRC_GAME,
        };
        return Ok((target.dir, source));
    }

    let Some(game_id) = game_id else {
        return Ok((root(), SRC_ROOT));
    };

    let game = db_guard
        .get_game_by_id(game_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "游戏不存在".to_string())?;

    // 【与截图写入同口径】此前这里只按 `games.exe_name` 定位，而写入侧已改成
    // 「手账指定 → 标题既有目录 → 标题新建」：两边一分叉就会出现"图截进了 `God of War`（569 张），
    // 详情页却盯着空的 `GoW` 说暂无截图"（实机案例，2026-09-14）。
    // 现在两侧共用 `capture_target_inputs` + `resolve_capture_target`，写哪里、看哪里永远一致。
    let (candidates, journal_dir) = capture_target_inputs(&db_guard, &game);
    if !candidates.is_empty() {
        let target = screenshot::resolve_capture_target(
            &root(),
            &candidates,
            journal_dir.as_deref(),
            game.exe_name.as_deref().unwrap_or_default(),
        );
        let source = match target.source {
            screenshot::CaptureDirSource::Journal => SRC_MANUAL,
            _ => SRC_GAME,
        };
        return Ok((target.dir, source));
    }

    // 题名、exe 名、安装目录都取不到（异常数据）→ 退回截图根目录
    Ok((root(), SRC_ROOT))
}

/// 打开截图文件夹（在文件管理器中）
///
/// - 传 `game_id`：打开该游戏的截图目录（无 exe_name 时退回截图根目录）；
/// - 传 `review_id`：打开手账条目的截图目录（手动指定 > 同名游戏 > 同名墓碑）；
/// - 都不传：打开截图根目录（设置页入口）。
///
/// 目录不存在时创建，保证「打开」总能成功。
#[tauri::command]
pub fn open_screenshot_dir(
    db: State<'_, Arc<Mutex<Database>>>,
    app_handle: tauri::AppHandle,
    game_id: Option<String>,
    review_id: Option<String>,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let (target, _source) = resolve_screenshot_dir(&db, game_id.as_deref(), review_id.as_deref())?;

    // 目录不存在则创建（保证"打开"总能成功）
    std::fs::create_dir_all(&target).map_err(|e| format!("创建截图目录失败: {e}"))?;

    app_handle
        .opener()
        .open_path(target.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("打开截图文件夹失败: {e}"))?;

    Ok(())
}

/// 查询截图目录信息（路径 / 是否存在 / 已有截图数量 / 定位来源），不创建目录。
/// 传 `game_id` 查该游戏目录，传 `review_id` 查手账条目目录，都不传查截图根目录。
#[tauri::command]
pub fn get_screenshot_dir(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: Option<String>,
    review_id: Option<String>,
) -> Result<ScreenshotDirInfo, String> {
    let (target, source) = resolve_screenshot_dir(&db, game_id.as_deref(), review_id.as_deref())?;
    Ok(ScreenshotDirInfo {
        exists: target.exists(),
        count: screenshot::count_images(&target),
        path: target.to_string_lossy().to_string(),
        source: source.to_string(),
    })
}

/// 截图目录候选（手账「指定截图目录」选择器用）：截图根目录下含图片的子目录
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScreenshotDirOption {
    /// 目录名（相对截图根目录）
    pub name: String,
    /// 目录内截图张数
    pub count: usize,
}

/// 列出可选的截图目录（截图根目录下的子目录，仅含有图目录，按名称排序）
#[tauri::command]
pub fn list_screenshot_dirs(
    db: State<'_, Arc<Mutex<Database>>>,
) -> Result<Vec<ScreenshotDirOption>, String> {
    // 只借锁读设置，随后立刻释放：目录枚举要 read_dir + 逐目录 stat 计数，
    // 大截图库下会让所有其它 DB 命令排队（前端整体卡顿）。
    let root = {
        let db_guard = lock_or_recover(&db);
        let settings = Settings::load_from_db(&db_guard).map_err(|e| e.to_string())?;
        std::path::PathBuf::from(crate::utils::path::expand_env_vars(&settings.screenshot_dir))
    };

    Ok(screenshot::list_image_subdirs(&root)
        .into_iter()
        .map(|(name, count)| ScreenshotDirOption { name, count })
        .collect())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Review;

    fn mem_db(screenshot_dir: &str) -> Arc<Mutex<Database>> {
        let db = Database::new(std::path::Path::new(":memory:")).expect("内存库初始化失败");
        db.set_setting("screenshot_dir", screenshot_dir).unwrap();
        Arc::new(Mutex::new(db))
    }

    fn add_review(db: &Arc<Mutex<Database>>, name: &str, screenshot_dir: Option<&str>) -> String {
        let mut review = Review::new(name.to_string());
        review.screenshot_dir = screenshot_dir.map(|s| s.to_string());
        let id = review.id.clone();
        db.lock().unwrap().insert_review(&review).unwrap();
        id
    }

    /// 测试临时目录守卫：Drop 时清理
    struct TempDirGuard(std::path::PathBuf);

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 手账截图目录解析：**与截图落盘同口径**（2026-09-14 统一）。
    ///
    /// 回归对象一：Mafia: The Old Country 从游戏库删除后（只留墓碑），手账截图库曾整块失效；
    /// 回归对象二：手账里历史自动回填的**空壳目录**会把新截图一直吸走，而界面盯着它显示"暂无截图"。
    ///
    /// 全程在临时目录里造数据，绝不触碰用户真实的 `H:\GameCapture`。
    #[test]
    fn review_screenshot_dir_resolution_priority() {
        let tmp = std::env::temp_dir().join(format!("gv_rev_shot_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _guard = TempDirGuard(tmp.clone());
        let db = mem_db(&tmp.to_string_lossy());

        let put_pngs = |name: &str, n: usize| {
            let dir = tmp.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            for i in 0..n {
                std::fs::write(dir.join(format!("s{i}.png")), b"x").unwrap();
            }
        };

        // 1) 手动指定且该目录确实有图 → 最权威
        put_pngs("MafiaTheOldCountry", 3);
        let manual = add_review(&db, "Mafia: The Old Country", Some("MafiaTheOldCountry"));
        let (path, source) = resolve_screenshot_dir(&db, None, Some(&manual)).unwrap();
        assert_eq!(source, SRC_MANUAL);
        assert_eq!(path, tmp.join("MafiaTheOldCountry"));

        // 2) 游戏已从游戏库删除、仅墓碑留档 → 借墓碑 exe 名对上既有含图目录
        {
            let guard = db.lock().unwrap();
            guard
                .insert_tombstone(
                    &uuid::Uuid::new_v4().to_string(),
                    "四海兄弟：故乡",
                    Some("MafiaTheOldCountry.exe"),
                    "2026-09-10T13:26:02Z",
                )
                .unwrap();
        }
        let tomb = add_review(&db, "四海兄弟：故乡", None);
        let (path, source) = resolve_screenshot_dir(&db, None, Some(&tomb)).unwrap();
        assert_eq!(source, SRC_TOMBSTONE);
        assert_eq!(
            path,
            tmp.join("MafiaTheOldCountry"),
            "墓碑 exe 名要能对上既有目录（去掉 .exe 后缀后比对）"
        );

        // 3) 同名活条目借 exe（优先于墓碑）；题名目录有图 → 命中它，空的 `GoW` 不得被复用
        put_pngs("God of War", 5);
        put_pngs("GoW", 0); // 历史空壳：0 张
        {
            let guard = db.lock().unwrap();
            let mut game = crate::models::Game::new("God of War".to_string());
            game.exe_name = Some("GoW.exe".to_string());
            guard.upsert_game(&game).unwrap();
        }
        let live = add_review(&db, "God of War", None);
        let (path, source) = resolve_screenshot_dir(&db, None, Some(&live)).unwrap();
        assert_eq!(source, SRC_GAME);
        assert_eq!(path, tmp.join("God of War"));

        // 4) 手账指定的目录是空壳 → 视为失效，改按标题落进有图目录（本次修复的核心行为）
        let empty_journal = add_review(&db, "God of War", Some("GoW"));
        let (path, source) = resolve_screenshot_dir(&db, None, Some(&empty_journal)).unwrap();
        assert_ne!(source, SRC_MANUAL, "空壳手账目录不得被采信");
        assert_eq!(path, tmp.join("God of War"));

        // 5) 题名与 exe 名都取不到（名字全空）→ 报错引导手动指定
        let orphan = add_review(&db, "   ", None);
        let err = resolve_screenshot_dir(&db, None, Some(&orphan)).unwrap_err();
        assert!(err.contains("手动指定"), "错误文案应可操作: {}", err);

        // 6) 手动值含路径穿越 → 非法而忽略，改按题名推断，且绝不越出截图根目录
        let evil = add_review(&db, "God of War", Some(r"..\..\Windows"));
        let (path, _) = resolve_screenshot_dir(&db, None, Some(&evil)).unwrap();
        assert!(
            path.starts_with(&tmp),
            "不得越出截图根目录: {}",
            path.display()
        );
    }

    /// 安装路径的末级常是技术性通用名，不得作为截图目录候选
    /// （用户截图根目录里确有含图的 `Content`(123) / `Common`(51)，误命中会把图截进无关目录）
    #[test]
    fn generic_install_dir_names_are_rejected() {
        for bad in [
            "Win32", "Binaries", "content", "  Common ", "Data", "Shipping", "x64", "Retail",
            "Support",
        ] {
            assert!(is_generic_dir_name(bad), "{bad} 应判为通用名");
        }
        for good in [
            "Mafia The Old Country",
            "DeathStrandingDC",
            "Red Dead Redemption",
            "H",
            "God of War",
            "MGSDelta",
        ] {
            assert!(!is_generic_dir_name(good), "{good} 不应判为通用名");
        }
    }
}
