//! 手柄截图键（XInput 轮询通道）。
//!
//! # 为什么另开一条通道
//!
//! 键盘侧已有两条通道（注册全局热键 + `hotkey_hook` 低级键盘钩子），但它们的前提是
//! 「手在键盘上」。用手柄玩时够不到键盘，故再挂一条 XInput 轮询通道。
//!
//! **它只管"按下"这件事**：命中组合后调用的仍是键盘通道那个唯一入口
//! （`commands::screenshots::dispatch_screenshot_async`），所以前台窗口匹配、
//! Steam 静默退让、快门音、落盘目录解析全部与键盘通道逐字一致。
//!
//! # 与键盘侧的差异（改前必读，2026-09-22 实测/拍板）
//!
//! - **只能轮询、不能注册**：操作系统没有"手柄全局热键"这种机制，只能按帧读手柄状态。
//! - **吞不掉按键**：XInput 是只读的，组合键会照常传给游戏（Steam Input 亦然），
//!   所以组合要挑游戏里用不到的（例如 `Back+A`）。
//! - **不依赖前台窗口与消息投递**：全屏独占游戏里反而比注册热键更可靠 —— 这正是键盘侧
//!   不得不挂低级钩子的那个痛点，手柄通道天生没有。
//! - **只在「本库有游戏会话在跑」时轮询**（老爷 2026-09-22 定的口径）：没有游戏会话时
//!   一个字节的 XInput 都不读，线程只睡。**录制**手柄键是用户的显式操作，不受此限。
//! - **Xbox 指南键（中间的 Logo）读不到**：XInput 根本不上报它（被系统 Xbox Game Bar 拿走），
//!   故键表里没有它。要支持它得改用 RawInput 解析 HID 报告，是另一个量级的工作。
//! - **只认 XInput 类设备**：Xbox 手柄原生可用；DS4/DS5 需经 Steam Input 或 DS4Windows
//!   转译成 XInput 才看得到（老爷明确不要求支持裸连的 DS 系列）。
//! - **Steam 游戏不响应**：不是本模块的特例，而是截图入口既有的 Steam 静默退让
//!   （Steam 自带截图、默认热键同为 F12）——手柄通道走同一入口，行为自然一致。
//!
//! # 匹配语义
//!
//! **子集匹配**：配置的键全部按住即触发（同时按着别的键不影响，避免"按着 RT 瞄准时
//! 组合键哑掉"）。触发后必须把配置里的键**全部松开**才允许下一次（自锁，等价键盘侧的
//! `MOD_NOREPEAT`）。扳机 LT/RT 是 0-255 模拟量，超过 [`TRIGGER_THRESHOLD`] 才算按下。

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::Win32::UI::Input::XboxController::{XInputGetState, XINPUT_GAMEPAD, XINPUT_STATE, XUSER_MAX_COUNT};

/// 轮询间隔：8ms ≈ 125Hz。手柄"按下 → 触发"的固有延迟上限就是这个值加上抓帧耗时（~11ms），
/// 对截图这种操作已经完全无感。
const POLL_INTERVAL_MS: u64 = 8;
/// 无游戏、无录制时的复查节拍：不是轮询手柄，只是看看"游戏是否开始了"。
const IDLE_INTERVAL_MS: u64 = 200;
/// 游戏运行状态的缓存有效期：轮询期内多久向追踪器问一次"现在有游戏在跑吗"。
/// 不每拍都问，是为了不与 1 秒一拍的主循环、以及截图路径抢追踪器锁。
const GATE_TTL: Duration = Duration::from_millis(200);
/// 当前没有手柄连接时的全槽复查周期。
///
/// **为什么要有这个节拍**（2026-09-22 实测）：本机量得单次 `XInputGetState` 约 **10.5µs**，
/// 且与槽位号无关（代价在调用本身，不是在槽位索引上）。若每拍都扫 4 个槽，
/// 125Hz × 4 = 500 次/秒 ≈ **5.3ms/s，占单核 0.53%** —— 手柄没插、游戏在跑时纯属白烧。
/// 改成"只轮已知连接的槽位、没有手柄时 500ms 才复查一次"后：
/// 有手柄时约 0.13% 单核（125 次/秒），无手柄时约 0.008%（8 次/秒）。
const RESCAN_WHEN_EMPTY: Duration = Duration::from_millis(500);
/// 已连接手柄时的全槽复查周期：只为兜住"又插了一个手柄"这种热插拔，
/// 代价 4 次调用 / 2 秒（约 0.002% 单核），可以忽略。
const RESCAN_PERIODIC: Duration = Duration::from_secs(2);
/// 录制超时：超过这么久还没走完"按下 → 全部松开"，按已观察到的结果收尾。
const RECORD_TIMEOUT: Duration = Duration::from_secs(10);

/// 扳机（LT/RT）按下阈值：0-255 的模拟量。
/// 取 128（约半程）而不是微软 `XINPUT_GAMEPAD_TRIGGER_THRESHOLD` 的 30：
/// 30 是给"死区"用的，手指搭在扳机上就会越过，做组合键会疯狂误触发。
const TRIGGER_THRESHOLD: u8 = 128;

/// 左扳机在掩码里的位（扳机不在 `wButtons` 里，只能另占高位）
pub const TRIG_LT: u32 = 1 << 16;
/// 右扳机在掩码里的位
pub const TRIG_RT: u32 = 1 << 17;

/// 键位表：**唯一的键名来源**，同时决定掩码位与展示顺序。
///
/// 低 16 位原样沿用 XInput `wButtons` 的位序（不做重排，少一层出错的机会）；
/// 高两位是模拟扳机。**必须与设置页展示的键名一一对应**（大小写不敏感）。
const KEY_TABLE: &[(&str, u32)] = &[
    // 肩键
    ("LB", 0x0100),
    ("RB", 0x0200),
    // 模拟扳机
    ("LT", TRIG_LT),
    ("RT", TRIG_RT),
    // 面键
    ("A", 0x1000),
    ("B", 0x2000),
    ("X", 0x4000),
    ("Y", 0x8000),
    // 十字键
    ("Up", 0x0001),
    ("Down", 0x0002),
    ("Left", 0x0004),
    ("Right", 0x0008),
    // 中央键
    ("Start", 0x0010),
    ("Back", 0x0020),
    // 摇杆按下
    ("LS", 0x0040),
    ("RS", 0x0080),
];

/// 当前生效的组合掩码（`0` = 未启用）。轮询线程无锁读取，故用原子整型承载。
static SPEC: AtomicU32 = AtomicU32::new(0);
/// 本轮组合是否已经触发过 —— 置位后必须等配置键全部松开才允许再次触发。
static LATCHED: AtomicBool = AtomicBool::new(false);
/// 触发回调（进程内只装一次）
static TRIGGER: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
/// 游戏运行状态的查询口（由 `lib.rs` 接进时长追踪器）
static GATE: OnceLock<Box<dyn Fn() -> bool + Send + Sync>> = OnceLock::new();
/// 录制结果通知口（由 `lib.rs` 转成事件发给前端）
static NOTICE: OnceLock<Box<dyn Fn(Notice) + Send + Sync>> = OnceLock::new();
/// 轮询线程是否已启动（防重复安装叠出多条线程）
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// 录制结果（发给前端）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// 录制成功：规范化的组合键字符串（如 `LB+A`）
    Recorded(String),
    /// 录制失败及原因
    Failed(String),
}

/// 键名 → 掩码位。未知键名返回 `None`（整个组合**整体作废**，不做"忽略未知键"的宽容：
/// 半截生效比失效更难查）。
fn key_name_to_bit(name: &str) -> Option<u32> {
    let upper = name.trim().to_ascii_uppercase();
    if let Some((_, bit)) = KEY_TABLE.iter().find(|(n, _)| n.to_ascii_uppercase() == upper) {
        return Some(*bit);
    }
    // 别名：只为宽容手输/历史配置，不作为展示名
    Some(match upper.as_str() {
        "L1" => 0x0100,
        "R1" => 0x0200,
        "L2" => TRIG_LT,
        "R2" => TRIG_RT,
        "L3" => 0x0040,
        "R3" => 0x0080,
        "SELECT" => 0x0020,
        _ => return None,
    })
}

/// 掩码 → 规范字符串。**录制结果与设置页展示都走它**，保证"显示什么就是存了什么"。
pub fn format_hotkey(mask: u32) -> String {
    KEY_TABLE
        .iter()
        .filter(|(_, bit)| mask & bit != 0)
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join("+")
}

/// 解析设置里的组合键（形如 `LB+RB` / `Back+A`）。
/// 空串或含未知键名返回 `None` —— 调用方据此停用手柄通道。
pub fn parse_hotkey(spec: &str) -> Option<u32> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut mask = 0u32;
    for part in trimmed.split('+') {
        mask |= key_name_to_bit(part)?;
    }
    if mask == 0 {
        None
    } else {
        Some(mask)
    }
}

/// 子集匹配：配置的键是否**全部**按住（`sample` 里多按的键不影响判定）
pub(crate) fn combo_satisfied(spec: u32, sample: u32) -> bool {
    spec != 0 && sample & spec == spec
}

/// 更新当前生效的组合键（`None` = 停用手柄通道）
pub fn set_spec(spec: Option<u32>) {
    // 与键盘侧同理：换键瞬间旧键很可能正被按住，若不复位自锁，新键的第一次按下会被吞掉
    LATCHED.store(false, Ordering::Release);
    SPEC.store(spec.unwrap_or(0), Ordering::Release);
}

/// 当前生效的组合键掩码（供日志/自检使用）
pub fn current_spec() -> Option<u32> {
    let raw = SPEC.load(Ordering::Acquire);
    if raw == 0 {
        None
    } else {
        Some(raw)
    }
}

/// 开始录制手柄键：等待"一次完整的按压序列"（从无键按下 → 按键 → 全部松开），
/// 取序列中**同时按下键数最多**的那一拍作为结果，经 [`Notice`] 回报。
///
/// 取"最多的一拍"而不是"按下过的键的并集"：并集会把"先按 A、松开、再按 B"这种
/// 依次操作凑成一个用户从没做过的组合。
pub fn start_recording() -> Result<(), String> {
    if !INSTALLED.load(Ordering::Acquire) {
        return Err("手柄通道未就绪".to_string());
    }
    {
        let mut guard = RECORDING.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(Recorder {
            best: 0,
            saw_press: false,
            deadline: Instant::now() + RECORD_TIMEOUT,
        });
    }
    tracing::info!("手柄截图键录制开始（{} 秒内完成一次按压）", RECORD_TIMEOUT.as_secs());
    Ok(())
}

/// 取消录制（用户中断）。不产生任何通知。
pub fn cancel_recording() {
    let mut guard = RECORDING.lock().unwrap_or_else(|e| e.into_inner());
    if guard.take().is_some() {
        tracing::info!("手柄截图键录制已取消");
    }
}

/// 录制是否进行中
fn recording_active() -> bool {
    RECORDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some()
}

struct Recorder {
    /// 观察到的"同时按下键数最多"的那一拍
    best: u32,
    /// 是否见过按键（用来区分"超时"和"压根没按"）
    saw_press: bool,
    deadline: Instant,
}

static RECORDING: Mutex<Option<Recorder>> = Mutex::new(None);

/// 把一拍采样喂给录制器。返回 `Some` 表示录制结束（成功或失败）。
fn feed_recorder(sample: u32) -> Option<Notice> {
    let mut guard = RECORDING.lock().unwrap_or_else(|e| e.into_inner());
    let rec = guard.as_mut()?;

    if sample != 0 {
        rec.saw_press = true;
        if sample.count_ones() > rec.best.count_ones() {
            rec.best = sample;
        }
    }
    // 收尾条件：走完一次完整按压序列（见过按下 + 此刻全松），或超时
    let finished = rec.saw_press && sample == 0;
    let expired = Instant::now() >= rec.deadline;
    if !finished && !expired {
        return None;
    }

    let best = rec.best;
    let saw_press = rec.saw_press;
    *guard = None;
    drop(guard);

    if !saw_press || best == 0 {
        return Some(Notice::Failed(
            "没有读到任何手柄按键（请确认手柄已连接、用的是 XInput 手柄，并在提示期间按一下按键）"
                .to_string(),
        ));
    }
    Some(Notice::Recorded(format_hotkey(best)))
}

/// 读一次手柄按键掩码。**只轮已知连接的槽位**，全槽扫描按两种节拍进行：
/// 没有手柄时 [`RESCAN_WHEN_EMPTY`]（500ms，为省电）、有手柄时 [`RESCAN_PERIODIC`]（2s，为热插拔）。
///
/// **为什么要缓存槽位**（2026-09-22 实测）：本机量得单次 `XInputGetState` 约 10.5µs，
/// 且与槽位号无关（代价在调用本身）。每拍扫 4 槽 = 500 次/秒 ≈ 0.53% 单核，
/// 手柄没插、游戏在跑时纯属白烧。缓存后：有手柄约 0.13%、无手柄约 0.008%。
///
/// 取"按下键数最多的那一个手柄"的采样，而不是跨手柄求并集：并集会把两个手柄各自按住的键
/// 凑成一个"组合"，那是用户从未做过的操作。
fn read_sample(known: &mut Vec<u32>, last_scan: &mut Option<Instant>) -> u32 {
    let scan_due = match *last_scan {
        None => true, // 从没扫过（含手柄全掉线后的复位）
        Some(t) if known.is_empty() => t.elapsed() >= RESCAN_WHEN_EMPTY,
        Some(t) => t.elapsed() >= RESCAN_PERIODIC,
    };

    if scan_due {
        *last_scan = Some(Instant::now());
        known.clear();
        let mut best = 0u32;
        for index in 0..XUSER_MAX_COUNT {
            if let Some(sample) = read_slot(index) {
                known.push(index);
                if sample.count_ones() > best.count_ones() {
                    best = sample;
                }
            }
        }
        return best;
    }

    if known.is_empty() {
        // 没手柄、又没到复查时刻 → 这一拍一个字节的 XInput 都不读
        return 0;
    }

    // 只轮已知槽位；掉线的剔出缓存（等下一个全扫周期/缓存清空后再去发现新插上的手柄）
    let mut best = 0u32;
    known.retain(|index| {
        if let Some(sample) = read_slot(*index) {
            if sample.count_ones() > best.count_ones() {
                best = sample;
            }
            true
        } else {
            false
        }
    });
    if known.is_empty() {
        // 手柄刚拔掉：允许下一拍立刻全扫，重新插上时能马上认出来
        *last_scan = None;
    }
    best
}

/// 读单个槽位：`Some(掩码)` = 该槽位有手柄；`None` = 无手柄或读失败
fn read_slot(index: u32) -> Option<u32> {
    let mut state = XINPUT_STATE::default();
    // SAFETY: state 是本栈上的合法可写指针；index 由调用方限定在 `XUSER_MAX_COUNT` 之内。
    // 返回非 0（通常是 ERROR_DEVICE_NOT_CONNECTED）即该槽位没手柄。
    let ret = unsafe { XInputGetState(index, &mut state) };
    if ret != 0 {
        return None;
    }
    Some(to_mask(&state.Gamepad))
}

/// XInput 手柄状态 → 掩码（按键位原样 + 两个扳机位）
fn to_mask(pad: &XINPUT_GAMEPAD) -> u32 {
    let mut mask = pad.wButtons.0 as u32;
    if pad.bLeftTrigger >= TRIGGER_THRESHOLD {
        mask |= TRIG_LT;
    }
    if pad.bRightTrigger >= TRIGGER_THRESHOLD {
        mask |= TRIG_RT;
    }
    mask
}

/// 安装手柄轮询通道（进程内只装一次）。
///
/// - `on_trigger`：命中组合时调用，**在轮询线程上**执行，必须极轻（只投递截图任务）；
/// - `is_game_running`：向时长追踪器问"本库是否有游戏会话在跑"，本模块据此决定要不要读手柄；
/// - `on_notice`：录制结果回调（转成前端事件）。
pub fn install(
    on_trigger: impl Fn() + Send + Sync + 'static,
    is_game_running: impl Fn() -> bool + Send + Sync + 'static,
    on_notice: impl Fn(Notice) + Send + Sync + 'static,
) -> Result<(), String> {
    // 幂等：`OnceLock` 会把第二次的闭包静默丢掉，但真去重装就会多出一条轮询线程
    if INSTALLED.swap(true, Ordering::SeqCst) {
        tracing::warn!("手柄通道已安装过，忽略重复安装请求");
        return Ok(());
    }
    let _ = TRIGGER.set(Box::new(on_trigger));
    let _ = GATE.set(Box::new(is_game_running));
    let _ = NOTICE.set(Box::new(on_notice));

    match std::thread::Builder::new()
        .name("gv-gamepad".to_string())
        .spawn(poll_loop)
    {
        Ok(_) => Ok(()),
        Err(e) => {
            // 线程没起来 → 复位，允许以后重试
            INSTALLED.store(false, Ordering::SeqCst);
            Err(format!("启动手柄轮询线程失败: {e}"))
        }
    }
}

/// 轮询主循环。**线程随进程存续**，不做退出：应用"关闭"只是隐藏窗口（`CloseRequested`
/// 被拦下），此时游戏可能仍在运行、手柄通道还得继续工作，没有安全的收尾时机。
fn poll_loop() {
    let mut game_running = false;
    // None = 还没查过 → 首拍强制问一次追踪器
    let mut gate_checked_at: Option<Instant> = None;
    // 已知连接的手柄槽位与上次全扫时刻（跨拍缓存，见 read_sample）
    let mut known_slots: Vec<u32> = Vec::new();
    let mut last_scan: Option<Instant> = None;

    loop {
        let fresh_gate = match gate_checked_at {
            Some(t) => t.elapsed() >= GATE_TTL,
            None => true,
        };
        if fresh_gate {
            game_running = GATE.get().map(|probe| probe()).unwrap_or(false);
            gate_checked_at = Some(Instant::now());
        }

        let recording = recording_active();
        if !game_running && !recording {
            // 老爷定的口径：不玩游戏就不碰手柄，一个字节的 XInput 都不读
            std::thread::sleep(Duration::from_millis(IDLE_INTERVAL_MS));
            continue;
        }

        let sample = read_sample(&mut known_slots, &mut last_scan);

        if recording {
            // 录制期间不触发截图：用户此刻是在"设置键位"，不是在"截图"
            if let Some(notice) = feed_recorder(sample) {
                match &notice {
                    Notice::Recorded(spec) => {
                        tracing::info!("手柄截图键录制完成: {spec}")
                    }
                    Notice::Failed(reason) => tracing::warn!("手柄截图键录制失败: {reason}"),
                }
                if let Some(cb) = NOTICE.get() {
                    cb(notice);
                }
            }
        } else if game_running {
            let spec = SPEC.load(Ordering::Acquire);
            if spec != 0 {
                if !combo_satisfied(spec, sample) {
                    // 组合不完整（含已松开）→ 重新上膛
                    LATCHED.store(false, Ordering::Release);
                } else if !LATCHED.swap(true, Ordering::AcqRel) {
                    // 第一次命中才触发；保持按住不会连发（等价 MOD_NOREPEAT）
                    if let Some(trigger) = TRIGGER.get() {
                        trigger();
                    }
                }
            }
        }

        std::thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 键表里每个键名都必须能解析回自己的位——设置页展示/存储只走这一张表
    #[test]
    fn parses_every_key_name_in_the_table() {
        for (name, bit) in KEY_TABLE {
            assert_eq!(parse_hotkey(name), Some(*bit), "{name} 应可解析");
        }
    }

    /// 组合解析：子集/大小写/空白容忍，未知键名整体作废
    #[test]
    fn parses_combinations_and_rejects_junk() {
        assert_eq!(parse_hotkey("LB+RB"), Some(0x0100 | 0x0200));
        assert_eq!(parse_hotkey(" lb + a "), Some(0x0100 | 0x1000));
        assert_eq!(parse_hotkey("LT+RT"), Some(TRIG_LT | TRIG_RT));
        assert_eq!(parse_hotkey("Back+A"), Some(0x0020 | 0x1000));
        assert_eq!(parse_hotkey("A+A"), Some(0x1000), "重复键退化为单个位");

        assert_eq!(parse_hotkey(""), None);
        assert_eq!(parse_hotkey("   "), None);
        assert_eq!(parse_hotkey("Guide"), None, "指南键 XInput 读不到，必须拒绝");
        assert_eq!(parse_hotkey("LB+"), None, "残缺组合整体作废");
        assert_eq!(parse_hotkey("LB+Nope"), None, "含未知键 → 整体作废");
    }

    /// 掩码 → 字符串必须可逆（录制结果经它落盘，回读还得一样）
    #[test]
    fn format_roundtrips_and_is_canonical() {
        for mask in [0x0100u32, 0x1000, TRIG_LT, 0x0100 | 0x2000, 0x0020 | 0x1000] {
            let text = format_hotkey(mask);
            assert_eq!(parse_hotkey(&text), Some(mask), "{text} 应可逆");
        }
        // 顺序固定：与 KEY_TABLE 一致，不是按按下顺序
        assert_eq!(format_hotkey(0x1000 | 0x0100), "LB+A");
        assert_eq!(format_hotkey(0x1000 | 0x0020), "A+Back");
    }

    /// 子集匹配：多按别的键不影响，缺一个就不算命中
    #[test]
    fn combo_matching_is_subset_based() {
        let spec = parse_hotkey("LB+A").unwrap();
        assert!(combo_satisfied(spec, 0x0100 | 0x1000), "恰好按全");
        assert!(
            combo_satisfied(spec, 0x0100 | 0x1000 | 0x0200),
            "多按了 RB 也算命中（按着 RT 瞄准时不该哑掉）"
        );
        assert!(!combo_satisfied(spec, 0x0100), "只按一半不算");
        assert!(!combo_satisfied(spec, 0x1000), "只按一半不算");
        assert!(!combo_satisfied(0, 0xFFFF), "未启用时永不命中");
    }

    /// 扳机位不能与 `wButtons` 的位撞车（撞了会让"按住 RT"变成"按下 A"这种鬼事）
    #[test]
    fn trigger_bits_do_not_collide_with_button_bits() {
        for (name, bit) in KEY_TABLE {
            if *name == "LT" || *name == "RT" {
                continue;
            }
            assert_eq!(*bit & 0xFFFF_0000, 0, "{name} 不该占高位");
        }
        assert_eq!(TRIG_LT & 0xFFFF, 0);
        assert_eq!(TRIG_RT & 0xFFFF, 0);
        assert_ne!(TRIG_LT, TRIG_RT);
    }

    /// 自锁语义：命中置位 → 保持按住不再触发；配置键松开后重新上膛
    #[test]
    fn latch_blocks_repeat_until_release() {
        let spec = parse_hotkey("LB+A").unwrap();
        set_spec(Some(spec));

        let held = 0x0100 | 0x1000;
        assert!(combo_satisfied(spec, held));
        assert!(!LATCHED.swap(true, Ordering::AcqRel), "第一次命中应触发");
        assert!(LATCHED.swap(true, Ordering::AcqRel), "保持按住不应再触发");

        // 松开配置键 → 重新上膛
        if !combo_satisfied(spec, held & !0x1000) {
            LATCHED.store(false, Ordering::Release);
        }
        assert!(!LATCHED.load(Ordering::Acquire));

        set_spec(None);
        assert_eq!(current_spec(), None, "停用后不应有生效键位");
        assert!(!LATCHED.load(Ordering::Acquire), "换键/停用必须复位自锁");
        set_spec(Some(spec));
        assert_eq!(current_spec(), Some(spec));
        set_spec(None);
    }

    /// 空缓存 + 刚全扫过 → 本拍不该再碰 XInput。
    /// 守的是省电口径：没手柄时每拍扫 4 槽要白烧约 0.53% 单核（实测单次 10.5µs）。
    /// 本机是否真的插着手柄不影响本断言（缓存空 + 未到期时直接返回 0，不调用 API）。
    #[test]
    fn empty_slot_cache_does_not_rescan_every_tick() {
        let mut known: Vec<u32> = Vec::new();
        let mut last_scan = Some(Instant::now());
        assert_eq!(read_sample(&mut known, &mut last_scan), 0);
        assert!(known.is_empty(), "未到期时不该去探测槽位");
    }

    /// 放一个全新录制器进去（绕过 `start_recording` 的 INSTALLED 前置检查，
    /// 让录制状态机可以脱离轮询线程单测）
    fn arm_recorder(timeout: Duration) {
        *RECORDING.lock().unwrap() = Some(Recorder {
            best: 0,
            saw_press: false,
            deadline: Instant::now() + timeout,
        });
    }

    /// 录制器状态机（**两个用例刻意合成一个测试函数**：它们共用 `RECORDING` 这个全局，
    /// 拆成两个函数后 Rust 会并行跑，互相清空对方的录制态，变成随机失败）
    #[test]
    fn recorder_state_machine() {
        assert!(RECORDING.lock().unwrap().is_none(), "测试间不应残留录制态");

        // ---- 用例 1：取"同时按下最多"的一拍，而不是按下过的键的并集 ----
        arm_recorder(Duration::from_secs(30));
        let lb = 0x0100u32;
        let a = 0x1000u32;
        assert_eq!(feed_recorder(lb), None, "先按 LB：还没松手，继续录");
        assert_eq!(feed_recorder(lb | a), None, "再按 A：同时按下数变多");
        assert_eq!(feed_recorder(a), None, "松开 LB：峰值仍是 LB+A");
        // 全部松开 → 结束，取峰值
        assert_eq!(feed_recorder(0), Some(Notice::Recorded("LB+A".to_string())));
        assert!(RECORDING.lock().unwrap().is_none(), "结束后必须清空录制态");

        // ---- 用例 2：全程无按键 → 超时后明确报"没读到" ----
        arm_recorder(Duration::ZERO); // 立刻过期
        match feed_recorder(0) {
            Some(Notice::Failed(reason)) => {
                assert!(reason.contains("没有读到"), "原因要指明没读到: {reason}")
            }
            other => panic!("应报失败，实际 {other:?}"),
        }
        assert!(RECORDING.lock().unwrap().is_none(), "失败后同样要清空录制态");
    }
}
