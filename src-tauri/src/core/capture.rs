//! Windows Graphics Capture（WGC）捕获层。
//!
//! # 两条取帧路径（2026-09-22 起默认走温会话）
//!
//! - **温会话（默认）**：把 WGC 会话**常驻**（游戏会话期一直挂着），只在"有人要帧"的
//!   那一帧才拷像素。按键后等目标窗口的**下一帧**即可出图。
//!   实测（本机 RTX 5090 D v2，探针 640×360@60fps 目标窗口）：
//!   | 指标 | 温会话 | 即用即建 |
//!   |---|---|---|
//!   | 按键 → 画面到手 | **11.05 ms**（3.15~13.11） | **155.8 ms**（137~183） |
//!   | 常驻显存代价 | +4 MiB（1440p 推算 ~+56~84 MiB） | 0 |
//!   | 常驻 GPU 代价 | util 8.0% vs 基线 9.2%（噪声内） | 0 |
//!   （另见应用日志：实机 2560×1440 游戏按键→抓到帧 166~181ms，与探针的冷启动数吻合。）
//! - **即用即建（兜底）**：每次新建会话、抓到首帧即停。冷启动约 150~180ms，但它**只依赖
//!   窗口存在**：会话一启动 WGC 就会投递"当前画面"这一帧，所以画面静止时它仍拿得到。
//!
//! # 为什么温会话必须带超时兜底（改前必读，2026-09-22）
//!
//! 温会话的语义是"按键后等目标窗口的**下一次**产帧"。当游戏画面**静止**时新帧永远不会来
//! —— 暂停菜单、过场黑屏、加载画面、游戏崩溃挂起、窗口最小化，都是这种状态。
//! 探针第一版就在静止窗口上整整等了 3 秒才被超时救回。
//! 故：等待超过 `WARM_FRAME_TIMEOUT` 就退回即用即建。最坏情况只是回到旧路径的 ~170ms，
//! 绝不会出现"按了没反应"。
//!
//! 超时分两种处置（这点容易写错，改前必读）：
//! - `Stalled`（**会话本身是好的**，只是没新帧）：**保留**温会话，只把本次退回即用即建。
//!   游戏一恢复出帧，下一张立刻又是温的；若此时销毁，下一张反而要平白多付一次建会话的 ~170ms。
//! - `Broken`（结构性失效：会话已关闭 / 取帧出错 / 建立失败）：必须**先销毁**再兜底，
//!   否则失效会话会被一直复用，每次都白等一整轮超时。
//!
//! # 关键点：`ColorFormat::Rgba16F` 拿到的线性 HDR 数据，`save_as_image` 会直接拒绝，
//! 因此这里只取原始字节，交给 `tonemap` 模块处理。

use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows_capture::capture::{CaptureControl, Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

/// 温会话等"下一帧"的超时：超过即判定"目标没在产帧"，退回即用即建。
///
/// 取值权衡：探针实测正常取帧 3~13ms；给 250ms 足以吃掉偶发的合成抖动（切窗口、掉帧、
/// 加载卡顿），又不至于让"按了没反应"变得明显 —— 超时后还有兜底路径接住。
const WARM_FRAME_TIMEOUT: Duration = Duration::from_millis(250);

/// 新会话等"首帧"的超时：会话建立本身就要 150~180ms，必须给足，否则每次都白等一轮。
const WARM_FIRST_FRAME_TIMEOUT: Duration = Duration::from_millis(2000);

/// 取帧标记（`WarmShared::pending`）的"陈旧"阈值：超过这么久还没复位，就认定上一轮
/// 取帧方已经死了（panic / 线程被杀），由下一个取帧方直接接管会话，而不是推倒重建。
///
/// 取值依据：正常取帧最长只需 `WARM_FIRST_FRAME_TIMEOUT`（2s），给 5s 是 2.5 倍余量，
/// 足以排除"上一轮其实还在正常等待"的情况，避免误接管导致并发拷帧。
const PENDING_STALE: Duration = Duration::from_secs(5);

/// 捕获到的一帧 HDR 原始数据
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    /// Rgba16F 无 padding 的原始字节（每像素 8 字节：R,G,B,A 各 2 字节 f16）
    pub rgba16f: Vec<u8>,
}

/// 把 `Frame` 里的像素拷出来（两条路径共用）。
///
/// 【注意】必须用 `as_nopadding_buffer` 的**返回值**：windows-capture 2.0.1 里，
/// 帧无行 padding 时直接返回帧内部缓冲、不写入传入的 buf（写入分支只在 has_padding()
/// 为真时走）。直接用 buf 会让无 padding 帧（如 Mafia 2560x1440）抓到 0 字节。
fn copy_frame_pixels(frame: &mut Frame<'_>) -> Result<CapturedFrame, String> {
    let fb = frame.buffer().map_err(|e| e.to_string())?;
    let width = fb.width();
    let height = fb.height();
    let mut buf = Vec::new();
    let no_pad = fb.as_nopadding_buffer(&mut buf);
    Ok(CapturedFrame {
        width,
        height,
        rgba16f: no_pad.to_vec(),
    })
}

// ==================== 路径一：即用即建（如今的兜底） ====================

/// 单帧捕获 handler：在第一次帧回调时拷贝数据并停止会话
struct SingleFrameCapture {
    frame: Option<CapturedFrame>,
    error: Option<String>,
}

impl GraphicsCaptureApiHandler for SingleFrameCapture {
    type Flags = ();
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(_ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            frame: None,
            error: None,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame<'_>,
        capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        if self.frame.is_some() {
            return Ok(());
        }

        match copy_frame_pixels(frame) {
            Ok(f) => self.frame = Some(f),
            Err(e) => self.error = Some(e),
        }
        // 拿到一帧即停，不再等后续帧
        capture_control.stop();
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// 从指定 HWND 抓取一帧 FP16 HDR 原始数据（即用即建，阻塞直到拿到帧或超时）。
///
/// 只作为温会话不可用时的兜底：它慢（~170ms），但**只依赖窗口存在**，
/// 会话一建立 WGC 就会投递当前画面这一帧。
pub fn capture_window_fp16_cold(hwnd: isize) -> anyhow::Result<CapturedFrame> {
    let window = Window::from_raw_hwnd(hwnd as *mut std::ffi::c_void);

    let settings = Settings::new(
        window,
        CursorCaptureSettings::WithoutCursor,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        // 截图场景不设节流，确保拿到最新帧
        MinimumUpdateIntervalSettings::Custom(Duration::ZERO),
        DirtyRegionSettings::Default,
        ColorFormat::Rgba16F,
        (),
    );

    // start_free_threaded 不接管当前线程，返回的 CaptureControl 可通过 callback() 访问 handler
    let control = SingleFrameCapture::start_free_threaded(settings)
        .map_err(|e| anyhow::anyhow!("启动 WGC 捕获失败: {e}"))?;

    // 轮询等待帧（带 8 秒超时：覆盖 WGC 冷启动预热与系统繁忙场景）
    let deadline = Instant::now() + Duration::from_secs(8);
    let frame = loop {
        {
            let cb = control.callback();
            let guard = cb.lock();
            if let Some(f) = &guard.frame {
                break f.clone();
            }
            if let Some(err) = &guard.error {
                anyhow::bail!("捕获帧失败: {err}");
            }
        }
        if Instant::now() > deadline {
            let _ = control.stop();
            anyhow::bail!("捕获帧超时（可能是窗口已关闭或 DRM 受保护内容）");
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    // 帧已拿到，优雅停止捕获会话
    let _ = control.stop();
    Ok(frame)
}

// ==================== 路径二：温会话（默认） ====================

/// 温会话的共享状态（handler 与取帧方之间传递帧）
#[derive(Default)]
struct WarmShared {
    /// 有人正在等下一帧（只有为 `Some` 时才拷像素）。
    ///
    /// 【为什么存"开始等待的时刻"而不是 bool】置位方若在等待途中 panic 或线程被杀，
    /// 标记会永久停在 true，此后每次截图都判定"会话不可复用"→ 销毁重建，
    /// 每张白付一次建会话的 ~170ms，而界面上完全看不出异常。存时刻后，超过
    /// `PENDING_STALE` 即认定上一轮已死、由下一轮直接接管（见 `ensure_warm_session`）。
    pending: Option<Instant>,
    /// 帧已就位
    frame: Option<CapturedFrame>,
    /// 取帧失败原因
    error: Option<String>,
    /// 会话已关闭（窗口销毁 / 系统终止捕获）
    closed: bool,
    /// 会话是否已收到过至少一帧：决定下次等待用"首帧超时"还是"复用超时"
    primed: bool,
    /// **首帧预算是否已烧过一次**（预建等首帧结束，或某次取帧已用掉一次
    /// `WARM_FIRST_FRAME_TIMEOUT` 仍未出帧）。
    ///
    /// 与 `primed` 的区别很关键：只靠 `primed` 判断会漏掉"后台预建刚把会话建好、
    /// 还卡在等首帧"这段窗口期（约 150~200ms）—— 那时会话可复用但一帧都没出过，
    /// 若按 250ms 复用超时去等，就会误判 Stalled 退化成冷启动。见
    /// `capture_window_fp16_warm` 里的三档超时选择。
    first_frame_probed: bool,
    /// 累计收到的帧回调数（可观测性：确认"游戏确实在产帧"）
    frames_seen: u64,
}

/// 常驻 handler：**只在有人要帧的那一帧才拷像素**。
///
/// 没人等待时对帧什么都不做 —— 帧在回调返回后自动释放，不积压也不拷贝。
/// 这是常驻会话能"几乎零成本"挂着的关键（探针实测：常驻 3 秒收到 185 次帧回调，
/// 显存 +4 MiB，GPU 利用率与基线无差异）。
struct WarmCapture {
    shared: Arc<Mutex<WarmShared>>,
    notify: Arc<Condvar>,
}

type WarmError = Box<dyn std::error::Error + Send + Sync>;

impl GraphicsCaptureApiHandler for WarmCapture {
    type Flags = (Arc<Mutex<WarmShared>>, Arc<Condvar>);
    type Error = WarmError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let (shared, notify) = ctx.flags;
        Ok(Self { shared, notify })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame<'_>,
        _capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let mut guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        guard.frames_seen += 1;
        guard.primed = true;

        if let Some(since) = guard.pending {
            match copy_frame_pixels(frame) {
                Ok(f) => guard.frame = Some(f),
                Err(e) => guard.error = Some(e),
            }
            let waited_ms = since.elapsed().as_secs_f64() * 1000.0;
            guard.pending = None;
            drop(guard);
            tracing::trace!("[截图] 帧回调已应答等待（等待 {waited_ms:.2} ms）");
            self.notify.notify_all();
        }
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        let mut guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        guard.closed = true;
        drop(guard);
        self.notify.notify_all();
        Ok(())
    }
}

/// 一个常驻的 WGC 会话
struct WarmSession {
    hwnd: isize,
    shared: Arc<Mutex<WarmShared>>,
    notify: Arc<Condvar>,
    /// `Option` 是为了能把控制权取出来再停（`stop()` 会消费 self）
    control: Option<CaptureControl<WarmCapture, WarmError>>,
}

/// 进程内唯一的温会话：同时只可能有一个游戏在前台被截图，无需按 hwnd 建多个。
static WARM: OnceLock<Mutex<Option<WarmSession>>> = OnceLock::new();

fn warm_slot() -> &'static Mutex<Option<WarmSession>> {
    WARM.get_or_init(|| Mutex::new(None))
}

/// 目标窗口是否还在（跨窗口调用安全）
fn window_alive(hwnd: isize) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::IsWindow;
    unsafe { IsWindow(Some(HWND(hwnd as *mut std::ffi::c_void))).as_bool() }
}

/// 温会话槽位的查询结果（供后台预建判断是否已就绪）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarmTarget {
    /// **槽位正忙**：别的线程正在建立或销毁会话（持有槽位锁，约 150~180ms）。
    /// 说明已经有人在干活，本拍不该再插手投递一次预建。
    Busy,
    /// 没有温会话
    Empty,
    /// 会话挂在指定窗口上
    Window(isize),
}

/// 查询当前温会话挂载的窗口。供预建逻辑判断是否已就绪。
///
/// 【用 try_lock 而非 lock】本函数会被后台追踪循环每拍（1s）调用一次；若此刻温会话正在
/// 建立（建一次约 150~180ms）或销毁，阻塞等待会把追踪循环一起拖住，游戏退出检测跟着变慢。
/// 拿不到锁时返回 `Busy`——**刻意与「没有会话」区分开**：二者对调用方的含义完全不同
/// （后者需要建会话，前者说明正在建，本拍应直接跳过）。
///
/// 【closed 会话视同 Empty（0.8.5 审查 O1 修复）】`on_closed` 触发后（DRM 内容 / 窗口切
/// 虚拟桌面 / 系统回收捕获）槽位里的 `WarmSession` 还在、hwnd 也没变，但它已经是死会话。
/// 若照样返回 `Window(hwnd)`，前台恰好还是这个窗口时，预建循环的二级漏斗每秒都被
/// 「会话已挂在这个窗口上」短路，死会话占着槽位直到下一次按快门才走 Broken → 销毁 →
/// 冷启动自愈。视同 Empty 后，下一拍就会走「判定为游戏 → 预建」，`ensure_warm_session`
/// 的复用判定里 `!guard.closed` 不成立 → 销毁旧的建新的，无缝换掉死会话。
/// （shared 拿不到锁说明 handler 正在拷帧或有人正在等帧——那会话显然活着，返回 Busy。）
pub fn warm_session_target() -> WarmTarget {
    let Ok(slot) = warm_slot().try_lock() else {
        return WarmTarget::Busy;
    };
    let Some(s) = slot.as_ref() else {
        return WarmTarget::Empty;
    };
    // 先落到局部变量再返回：若把 match 直接作尾表达式，临时锁守卫的析构会排在 `slot`
    // 之后，借用检查器判定 `slot` 活不够长（E0597）。WarmTarget 是 Copy，无额外代价。
    let target = match s.shared.try_lock() {
        Ok(guard) if guard.closed => WarmTarget::Empty,
        Ok(_) => WarmTarget::Window(s.hwnd),
        Err(_) => WarmTarget::Busy,
    };
    target
}

/// 销毁温会话（游戏退出 / 窗口失效 / 取帧异常时调用）。
///
/// 【必须在后台线程 stop】`CaptureControl::stop()` 会投递 WM_QUIT **并 join 捕获线程**；
/// 万一那个线程没能及时退出，join 会把调用方（后台追踪循环或截图线程）一起拖住。
/// 这里一律丢进后台线程，并保证只停一次。
pub fn release_warm_session() {
    let taken = match warm_slot().lock() {
        Ok(mut slot) => slot.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    };
    let Some(mut session) = taken else {
        return;
    };
    let Some(control) = session.control.take() else {
        return;
    };
    let shared = session.shared.clone();
    let notify = session.notify.clone();
    // 先置 closed，让仍在等帧的调用方立刻醒来走兜底，不必干等到超时
    if let Ok(mut guard) = shared.lock() {
        guard.closed = true;
    }
    notify.notify_all();

    let spawned = std::thread::Builder::new()
        .name("gv-wgc-stop".to_string())
        .spawn(move || {
            let _ = control.stop();
        });
    if let Err(e) = spawned {
        // 起不了线程也要让句柄随 control 一起 Drop，绝不能把会话泄漏出作用域
        tracing::error!("[截图] 无法创建温会话停止线程: {e}");
    }
}

/// 取帧等待的超时档位选择（纯函数，便于单测）。返回 `(超时, 本轮是否属于首帧预算)`。
///
/// 【为什么不能只看"会话是不是刚建的"】"可复用"与"已出过帧"是两件事：预建线程把会话
/// 塞进槽位后就自己挂着等首帧（最长 2s），这段窗口里按快门拿到的是一个"能复用但零帧"
/// 的会话，若按 250ms 复用档去等，必然误判 Stalled 退化成冷启动（250+170ms），
/// 比不走温会话还慢。反之，已经烧过一次首帧预算仍不出帧（画面静止/最小化/DRM）时，
/// 必须回到 250ms 档，否则每张都要傻等 2s。
fn warm_wait_plan(primed: bool, first_frame_probed: bool) -> (Duration, bool) {
    let first_frame_budget = !primed && !first_frame_probed;
    let timeout = if first_frame_budget {
        WARM_FIRST_FRAME_TIMEOUT
    } else {
        WARM_FRAME_TIMEOUT
    };
    (timeout, first_frame_budget)
}

/// 判定并清理**陈旧的取帧标记**；返回是否发生了"接管"。
///
/// 上一轮取帧方若在等待途中 panic 或线程被杀，`pending` 会永久卡住，此后每次截图都判定
/// "会话不可复用"→ 销毁重建，每张白付一次建会话的 ~170ms 且界面无异常提示。这里认领它：
/// 会话本身是好的，只是标记没复位 —— 清掉残留后正常复用即可。正常路径不会触发。
fn reclaim_stale_pending(guard: &mut WarmShared) -> bool {
    let Some(since) = guard.pending else {
        return false;
    };
    if since.elapsed() < PENDING_STALE {
        return false;
    }
    let ms = since.elapsed().as_secs_f64() * 1000.0;
    tracing::warn!(
        "[截图] 发现陈旧的取帧标记（已挂起 {ms:.0} ms，超过 {}ms 阈值），\
         判定上一轮取帧方异常终止，接管会话而非重建",
        PENDING_STALE.as_millis()
    );
    guard.pending = None;
    guard.frame = None;
    guard.error = None;
    true
}

/// 确保 `hwnd` 上挂着一个可用的温会话；返回其共享状态（用于等帧）。
///
/// 返回的 `bool` 表示"这次是否新建了会话"（用于日志与预建的"已存在"判断）。
///
/// 【注意】**超时档位不能只看这个值**：可复用的会话也可能一帧都还没出过（预建刚建好、
/// 还在等首帧），真正决定档位的是 `WarmShared::primed` 与 `first_frame_probed`。
fn ensure_warm_session(hwnd: isize) -> anyhow::Result<(Arc<Mutex<WarmShared>>, Arc<Condvar>, bool)> {
    let mut slot = match warm_slot().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };

    // 复用的前提：窗口同一个、会话没关、窗口还在、且没人正在等帧（防重入）
    if let Some(existing) = slot.as_mut() {
        let reusable = {
            let mut guard = existing.shared.lock().unwrap_or_else(|e| e.into_inner());
            // 【陈旧取帧标记兜底】见 `reclaim_stale_pending`：会话本身没坏，只是上一轮的
            // 取帧标记没复位；这里接管而不是推倒重建（后者每张白付 ~170ms）。
            reclaim_stale_pending(&mut guard);
            existing.hwnd == hwnd && !guard.closed && guard.pending.is_none()
        };
        if reusable && window_alive(hwnd) {
            return Ok((existing.shared.clone(), existing.notify.clone(), false));
        }
        // 窗口换了 / 会话已关闭 / 窗口没了 → 换掉旧的
        if let Some(mut old) = slot.take() {
            if let Some(control) = old.control.take() {
                let _ = std::thread::Builder::new()
                    .name("gv-wgc-stop".to_string())
                    .spawn(move || {
                        let _ = control.stop();
                    });
            }
        }
    }

    let window = Window::from_raw_hwnd(hwnd as *mut std::ffi::c_void);
    let shared = Arc::new(Mutex::new(WarmShared::default()));
    let notify = Arc::new(Condvar::new());
    let settings = Settings::new(
        window,
        CursorCaptureSettings::WithoutCursor,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Custom(Duration::ZERO),
        DirtyRegionSettings::Default,
        ColorFormat::Rgba16F,
        (shared.clone(), notify.clone()),
    );

    let control = WarmCapture::start_free_threaded(settings)
        .map_err(|e| anyhow::anyhow!("启动 WGC 温会话失败: {e}"))?;

    *slot = Some(WarmSession {
        hwnd,
        shared: shared.clone(),
        notify: notify.clone(),
        control: Some(control),
    });
    Ok((shared, notify, true))
}

/// 同一个窗口的预建尝试冷却：避免"游戏最小化了 / 画面静止"时追踪循环每秒重建一次会话
/// （建立一次会话要 150~180ms，每秒来一次会白白吃掉一个核的 15%）。
const PRIME_COOLDOWN: Duration = Duration::from_secs(30);

static LAST_PRIME: OnceLock<Mutex<Option<(isize, Instant)>>> = OnceLock::new();

/// 预建温会话：后台建立会话并等它收到第一帧（**不拷像素**）。
///
/// 【为什么需要它】温会话的语义是"按键后等下一帧"。若等到第一次按键才建会话，那第一张
/// 截图仍要付 ~170ms 的建会话开销。老爷拍板"游戏会话存在期间就保持这个会话"，
/// 所以追踪循环在前台是被追踪游戏时会调用本函数，把会话提前挂上并等它出首帧，
/// 之后每一次按快门（包括第一张）都是温的。
///
/// 本函数设计为**可在后台线程调用**：失败静默返回，绝不抛错影响调用方。
pub fn prime_warm_session(hwnd: isize) {
    // 冷却：同一窗口 30 秒内只试一次
    {
        let slot = LAST_PRIME.get_or_init(|| Mutex::new(None));
        let mut guard = match slot.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some((h, at)) = *guard {
            if h == hwnd && at.elapsed() < PRIME_COOLDOWN {
                return;
            }
        }
        *guard = Some((hwnd, Instant::now()));
    }

    let (shared, notify, fresh) = match ensure_warm_session(hwnd) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("[截图] 预建温会话失败: {e}");
            return;
        }
    };
    if !fresh {
        return; // 已存在且可用，无需再等
    }

    // 等首帧到达即可（不拷像素：pending 保持 None，handler 不会去动帧数据）
    let deadline = Instant::now() + WARM_FIRST_FRAME_TIMEOUT;
    let mut guard = match shared.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    while !guard.primed && !guard.closed && Instant::now() < deadline {
        let remaining = deadline - Instant::now();
        if remaining.is_zero() {
            break;
        }
        let (next, _) = match notify.wait_timeout(guard, remaining) {
            Ok(v) => v,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard = next;
    }

    // 本轮"首帧探测"到此结束（无论成没成）：之后取帧一律按复用档（250ms）判，
    // 见 `capture_window_fp16_warm` 的三档说明。
    guard.first_frame_probed = true;
    if guard.primed {
        tracing::info!("[截图] 温会话已预建就绪（hwnd {hwnd:#x}）");
    } else {
        tracing::warn!("[截图] 温会话预建未等到首帧，保持会话等待下一次截图时再试");
    }
}

/// 温会话取帧的结果
enum WarmOutcome {
    Frame(CapturedFrame),
    /// 会话本身是好的，只是**等不到下一帧**（画面静止：暂停菜单 / 过场 / 最小化）。
    /// 调用方退回即用即建，但**保留温会话**：游戏一恢复出帧，下一张截图立刻又是温的；
    /// 若此时销毁，下一次截图反而要平白多付一次建会话的 ~170ms。
    Stalled(&'static str),
    /// 结构性失效（会话已关闭 / 取帧出错 / 会话建立失败）：必须先销毁再兜底，
    /// 否则失效会话会被一直复用，每次都白等一轮超时。
    Broken(&'static str),
}

/// 用温会话取一帧：挂上"我要帧"的标记，然后等目标窗口下一次产帧。
fn capture_window_fp16_warm(hwnd: isize) -> WarmOutcome {
    let (shared, notify, fresh) = match ensure_warm_session(hwnd) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("[截图] 温会话建立失败: {e}");
            return WarmOutcome::Broken("建立温会话失败");
        }
    };

    let started = Instant::now();
    let mut guard = match shared.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };

    // 【超时档位三选一 —— 不能只看 fresh】
    // `fresh` 只表示"本次调用新建了会话"，而"会话能复用"≠"会话已经出过帧"：
    //   ① primed                  → 帧流健康，走 250ms 快路径
    //   ② !primed && !probed      → 会话还没出过首帧。含最关键的那种情况：
    //      后台预建刚把会话塞进槽位、自己还挂着等首帧时按下快门（约 150~200ms 窗口），
    //      此时 fresh=false 但一帧都还没有，给 2s；否则必然误判 Stalled 退化成冷启动
    //      （250ms 白等 + ~170ms 冷启动 ≈ 420ms，反而比不温还慢）
    //   ③ !primed && probed       → 已烧过一次首帧预算仍不出帧（画面静止/最小化/DRM），
    //      按既定设计走 250ms → Stalled → 冷启动兜底
    let (timeout, first_frame_budget) = warm_wait_plan(guard.primed, guard.first_frame_probed);
    let wait_label = if fresh {
        "新会话首帧"
    } else if first_frame_budget {
        "复用·预建首帧中"
    } else {
        "复用"
    };

    // 清掉上一轮残留，本轮开始等待
    guard.frame = None;
    guard.error = None;
    guard.pending = Some(Instant::now());

    loop {
        if let Some(frame) = guard.frame.take() {
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            let seen = guard.frames_seen;
            drop(guard);
            tracing::info!(
                "[截图] 温会话取帧 {:.2} ms（{}，会话累计 {} 帧）",
                elapsed_ms,
                wait_label,
                seen
            );
            return WarmOutcome::Frame(frame);
        }
        if let Some(err) = guard.error.take() {
            guard.pending = None;
            drop(guard);
            tracing::warn!("[截图] 温会话取帧出错: {err}");
            return WarmOutcome::Broken("取帧出错");
        }
        if guard.closed {
            guard.pending = None;
            drop(guard);
            return WarmOutcome::Broken("会话已关闭");
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            guard.pending = None;
            // 已经烧掉一次首帧预算仍不出帧（画面静止/最小化）→ 记下来，
            // 之后一律走 250ms 档，不再每次傻等 2s。
            if first_frame_budget {
                guard.first_frame_probed = true;
            }
            let probed_note = if first_frame_budget {
                "（首帧预算已用尽，后续按复用超时处理）"
            } else {
                ""
            };
            drop(guard);
            tracing::warn!(
                "[截图] 温会话等下一帧超时（{:?}，{}{}）：画面可能静止（暂停/过场/最小化）",
                timeout,
                wait_label,
                probed_note
            );
            return WarmOutcome::Stalled("等不到下一帧");
        }
        let (next, _) = match notify.wait_timeout(guard, timeout - elapsed) {
            Ok(v) => v,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard = next;
    }
}

/// 从指定 HWND 抓取一帧 FP16 HDR 原始数据（**温会话优先，等不到则退回即用即建**）。
///
/// 这是截图链路唯一的取帧入口（`screenshot::capture_frame` 即由此构成，
/// 后处理与落盘在 `screenshot::save_frame`，二者已在 0.8.4 拆分）。
pub fn capture_window_fp16(hwnd: isize) -> anyhow::Result<CapturedFrame> {
    match capture_window_fp16_warm(hwnd) {
        WarmOutcome::Frame(frame) => Ok(frame),
        // 只是画面静止：保留温会话（游戏一出帧下一张又是温的），本次退回即用即建
        WarmOutcome::Stalled(reason) => {
            tracing::info!("[截图] 温会话等不到下一帧（{reason}），本次退回即用即建并保留会话");
            capture_window_fp16_cold(hwnd)
        }
        // 结构性失效：先销毁，否则失效会话会被一直复用，每次都白等一轮
        WarmOutcome::Broken(reason) => {
            tracing::warn!("[截图] 温会话失效（{reason}），销毁后退回即用即建");
            release_warm_session();
            capture_window_fp16_cold(hwnd)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 没有温会话时释放必须是安全的空操作（游戏退出、取帧兜底都会走到这里）。
    ///
    /// 只测这一条：其余温会话行为都依赖真实窗口与 WGC，无法在单元测试里稳定复现。
    /// 注意它会取走全局温会话槽位，若将来新增同样触碰该槽位的测试需串行化。
    #[test]
    fn release_warm_session_is_safe_without_session() {
        release_warm_session();
        release_warm_session();
    }

    /// 超时档位三选一：这是"预建窗口期按下快门"不退化成冷启动的回归保护。
    #[test]
    fn warm_wait_plan_picks_first_frame_budget_only_until_probed() {
        // ① 帧流健康 → 复用档
        let (t, budget) = warm_wait_plan(true, false);
        assert_eq!(t, WARM_FRAME_TIMEOUT);
        assert!(!budget);
        let (t, budget) = warm_wait_plan(true, true);
        assert_eq!(t, WARM_FRAME_TIMEOUT);
        assert!(!budget);

        // ② 会话还没出过首帧、且没烧过首帧预算（含"预建刚建好还在等首帧"）→ 首帧档
        let (t, budget) = warm_wait_plan(false, false);
        assert_eq!(t, WARM_FIRST_FRAME_TIMEOUT);
        assert!(budget, "这种情况必须给足首帧时间，否则必然误判 Stalled");

        // ③ 已烧过一次首帧预算仍不出帧（画面静止/最小化）→ 回到复用档，不再每次傻等 2s
        let (t, budget) = warm_wait_plan(false, true);
        assert_eq!(t, WARM_FRAME_TIMEOUT);
        assert!(!budget);
    }

    /// 陈旧取帧标记：超过阈值要被接管（清标记），未超过则原样保留。
    #[test]
    fn reclaim_stale_pending_only_after_threshold() {
        let mut s = WarmShared::default();

        // 没人等帧 → 不动
        assert!(!reclaim_stale_pending(&mut s));
        assert!(s.pending.is_none());

        // 刚开始等（新鲜）→ 不动，别去抢别人的等待
        s.pending = Some(Instant::now());
        assert!(!reclaim_stale_pending(&mut s));
        assert!(s.pending.is_some());

        // 上一轮早已超时仍挂着 → 接管并清空残留
        s.pending = Some(Instant::now() - PENDING_STALE - Duration::from_secs(1));
        s.frame = None;
        assert!(reclaim_stale_pending(&mut s));
        assert!(s.pending.is_none(), "接管后必须可复用，否则每次都要重建会话");
    }
}
