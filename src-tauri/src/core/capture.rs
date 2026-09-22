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
    /// 有人正在等下一帧（只有为 true 时才拷像素）
    pending: bool,
    /// 帧已就位
    frame: Option<CapturedFrame>,
    /// 取帧失败原因
    error: Option<String>,
    /// 会话已关闭（窗口销毁 / 系统终止捕获）
    closed: bool,
    /// 会话是否已收到过至少一帧：决定下次等待用"首帧超时"还是"复用超时"
    primed: bool,
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

        if guard.pending {
            match copy_frame_pixels(frame) {
                Ok(f) => guard.frame = Some(f),
                Err(e) => guard.error = Some(e),
            }
            guard.pending = false;
            drop(guard);
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

/// 当前温会话挂载的窗口句柄（没有则 None）。供预建逻辑判断是否已就绪。
///
/// 【用 try_lock 而非 lock】本函数会被后台追踪循环每拍（1s）调用一次；若此刻温会话正在
/// 建立（建一次约 150~180ms）或销毁，阻塞等待会把追踪循环一起拖住，游戏退出检测跟着变慢。
/// 拿不到锁时返回 None（等价于"这一拍先不判断"，下一拍再看），无副作用。
pub fn warm_session_hwnd() -> Option<isize> {
    warm_slot()
        .try_lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(|s| s.hwnd))
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

/// 确保 `hwnd` 上挂着一个可用的温会话；返回其共享状态（用于等帧）。
///
/// 返回的 `bool` 表示"这次是否新建了会话"：新建的会话等首帧要用长超时。
fn ensure_warm_session(hwnd: isize) -> anyhow::Result<(Arc<Mutex<WarmShared>>, Arc<Condvar>, bool)> {
    let mut slot = match warm_slot().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };

    // 复用的前提：窗口同一个、会话没关、窗口还在、且没人正在等帧（防重入）
    if let Some(existing) = slot.as_ref() {
        let reusable = {
            let guard = existing.shared.lock().unwrap_or_else(|e| e.into_inner());
            existing.hwnd == hwnd && !guard.closed && !guard.pending
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

    // 等首帧到达即可（不拷像素：pending 保持 false，handler 不会去动帧数据）
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

    let timeout = if fresh {
        WARM_FIRST_FRAME_TIMEOUT
    } else {
        WARM_FRAME_TIMEOUT
    };

    let started = Instant::now();
    let mut guard = match shared.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    // 清掉上一轮残留，本轮开始等待
    guard.frame = None;
    guard.error = None;
    guard.pending = true;

    loop {
        if let Some(frame) = guard.frame.take() {
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            let seen = guard.frames_seen;
            drop(guard);
            tracing::info!(
                "[截图] 温会话取帧 {:.2} ms（{}，会话累计 {} 帧）",
                elapsed_ms,
                if fresh { "新会话首帧" } else { "复用" },
                seen
            );
            return WarmOutcome::Frame(frame);
        }
        if let Some(err) = guard.error.take() {
            guard.pending = false;
            drop(guard);
            tracing::warn!("[截图] 温会话取帧出错: {err}");
            return WarmOutcome::Broken("取帧出错");
        }
        if guard.closed {
            guard.pending = false;
            drop(guard);
            return WarmOutcome::Broken("会话已关闭");
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            guard.pending = false;
            drop(guard);
            tracing::warn!(
                "[截图] 温会话等下一帧超时（{:?}，{}）：画面可能静止（暂停/过场/最小化）",
                timeout,
                if fresh { "新会话首帧" } else { "复用" }
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
/// 这是截图链路唯一的取帧入口（`screenshot::capture_and_save` 调用它）。
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
}
