//! 全局低级键盘钩子（WH_KEYBOARD_LL）：截图热键在游戏内失效时的**必备**触发通道。
//!
//! # 为什么需要它（2026-09-14 实测，不要删）
//!
//! 全屏游戏窗口处于前台时，`RegisterHotKey` 注册成功却收不到 `WM_HOTKEY`：
//! - 同一进程内注册的对照热键（F7）在游戏内注入与物理按键**均不触发**；
//! - 同一时刻 WH_KEYBOARD_LL 钩子能稳定看到这些按键（`vk=0x78 F9`，注入/物理都记录到）；
//! - 对照：窗口化应用（GameVault 自身、播放器）在前台时注册热键工作正常。
//!
//! 即"注册热键"这条通道在游戏内会被系统整体吞掉，与是否提权、是否 GameVault 无关。
//! AutoHotkey 等工具以键盘钩子为主通道，原因正在于此。
//!
//! # 设计取舍
//!
//! - **双通道并存**：注册热键照旧（窗口化/桌面场景 + 设置页的"键被占用"提示依赖它），
//!   钩子负责游戏内。同一次按键两条通道都会到达，由调用方 `SHOT_IN_FLIGHT` 去重（先到者赢）。
//! - **回调内零阻塞**：钩子回调同步阻塞全系统输入，所以里面**只做原子判断 + 投递新线程**，
//!   连日志都不写（日志的 Mutex + 文件写会拖慢全系统按键）。
//! - **不吞键**：返回 `CallNextHookEx` 的结果，按键照常传给游戏，不改变游戏内按键语义。
//! - **已知边界**：低级钩子收不到"提权窗口"的按键（UIPI 限制），此时两条通道都无效；
//!   本机实测游戏进程为 `asInvoker` 非提权，故正常工作。

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::OnceLock;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, SetWindowsHookExW, UnhookWindowsHookEx, KBDLLHOOKSTRUCT, MSG,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

/// 解析后的截图键：目标虚拟键码 + 必须按住的修饰键
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeySpec {
    pub vk: u32,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub win: bool,
}

impl KeySpec {
    /// 打包成单个 u32（`0` 表示"未启用"）：钩子回调里必须无锁读取，故用原子整型承载
    fn encode(self) -> u32 {
        (self.vk << 8)
            | (self.ctrl as u32)
            | ((self.shift as u32) << 1)
            | ((self.alt as u32) << 2)
            | ((self.win as u32) << 3)
    }

    fn decode(raw: u32) -> Self {
        Self {
            vk: raw >> 8,
            ctrl: raw & 0x01 != 0,
            shift: raw & 0x02 != 0,
            alt: raw & 0x04 != 0,
            win: raw & 0x08 != 0,
        }
    }
}

/// 当前生效的键（0 = 未启用）
static SPEC: AtomicU32 = AtomicU32::new(0);
/// 目标键是否处于"已按下"状态：用于过滤自动重复（等价注册热键的 MOD_NOREPEAT）
static KEY_DOWN: AtomicBool = AtomicBool::new(false);
/// 触发回调（进程内只装一次）
static TRIGGER: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// 键名 → 虚拟键码。**只覆盖设置页录制器会产出的键名**
/// （见 `SettingsView.vue::normalizeHotkeyKey`），未知键名返回 None：
/// 此时不装钩子通道（仅退化为注册热键通道），并把原因记进日志。
fn key_name_to_vk(name: &str) -> Option<u32> {
    let upper = name.to_ascii_uppercase();
    // F1-F24
    if let Some(rest) = upper.strip_prefix('F') {
        if let Ok(n) = rest.parse::<u32>() {
            if (1..=24).contains(&n) {
                return Some(0x70 + n - 1);
            }
        }
    }
    // A-Z / 0-9
    if upper.len() == 1 {
        let c = upper.as_bytes()[0];
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(c as u32);
        }
    }
    Some(match upper.as_str() {
        "SPACE" => 0x20,
        "TAB" => 0x09,
        "ENTER" | "RETURN" => 0x0D,
        "ESC" | "ESCAPE" => 0x1B,
        "BACKSPACE" => 0x08,
        "DELETE" | "DEL" => 0x2E,
        "INSERT" | "INS" => 0x2D,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGEUP" | "PGUP" => 0x21,
        "PAGEDOWN" | "PGDN" => 0x22,
        "UP" => 0x26,
        "DOWN" => 0x28,
        "LEFT" => 0x25,
        "RIGHT" => 0x27,
        "PRINTSCREEN" | "PRTSC" => 0x2C,
        "SCROLLLOCK" => 0x91,
        "PAUSE" => 0x13,
        "CAPSLOCK" => 0x14,
        "NUMLOCK" => 0x90,
        "MINUS" => 0xBD,
        "EQUAL" => 0xBB,
        "BRACKETLEFT" => 0xDB,
        "BRACKETRIGHT" => 0xDD,
        "BACKSLASH" => 0xDC,
        "SEMICOLON" => 0xBA,
        "QUOTE" => 0xDE,
        "COMMA" => 0xBC,
        "PERIOD" => 0xBE,
        "SLASH" => 0xBF,
        "BACKQUOTE" => 0xC0,
        _ => return None,
    })
}

/// 解析设置页保存的热键字符串（形如 `F9` / `Ctrl+Shift+S` / `Alt+PrintScreen`）。
/// 无法解析（空串、未知键名）返回 None —— 调用方据此停用钩子通道。
pub fn parse_key_spec(spec: &str) -> Option<KeySpec> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut out = KeySpec {
        vk: 0,
        ctrl: false,
        shift: false,
        alt: false,
        win: false,
    };
    for (idx, part) in trimmed.split('+').enumerate() {
        let part = part.trim();
        if part.is_empty() {
            return None;
        }
        match part.to_ascii_uppercase().as_str() {
            "CTRL" | "CONTROL" => out.ctrl = true,
            "SHIFT" => out.shift = true,
            "ALT" => out.alt = true,
            "SUPER" | "WIN" | "META" | "CMD" | "COMMAND" => out.win = true,
            _ => {
                // 主键必须且只能出现一次，且必须排在修饰键之后（与设置页 join 顺序一致）
                if idx + 1 != trimmed.split('+').count() || out.vk != 0 {
                    return None;
                }
                out.vk = key_name_to_vk(part)?;
            }
        }
    }
    if out.vk == 0 {
        return None;
    }
    Some(out)
}

/// 安装键盘钩子（进程内只装一次；重复调用只更新触发回调）。
///
/// `on_trigger` 会在**钩子回调线程**上被调用，必须极轻（只投递，不做日志/IO/锁）。
pub fn install(on_trigger: impl Fn() + Send + Sync + 'static) -> Result<(), String> {
    let _ = TRIGGER.set(Box::new(on_trigger));

    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    std::thread::Builder::new()
        .name("gv-key-hook".to_string())
        .spawn(move || {
            // SAFETY: 钩子过程在本线程消息循环中回调；线程存续进程全程（故不卸载）
            let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0) };
            match hook {
                Ok(h) => {
                    let _ = tx.send(Ok(()));
                    let mut msg = MSG::default();
                    // 低级钩子必须有消息循环才会被系统回调
                    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {}
                    let _ = unsafe { UnhookWindowsHookEx(h) };
                }
                Err(e) => {
                    let _ = tx.send(Err(format!("安装键盘钩子失败: {e}")));
                }
            }
        })
        .map_err(|e| format!("启动键盘钩子线程失败: {e}"))?;

    rx.recv().map_err(|e| e.to_string())?
}

/// 更新当前生效的截图键（`None` = 停用钩子通道，例如用户清空了快捷键）
pub fn set_spec(spec: Option<KeySpec>) {
    SPEC.store(spec.map(KeySpec::encode).unwrap_or(0), Ordering::Release);
}

/// 当前生效的键（供日志/自检使用）
pub fn current_spec() -> Option<KeySpec> {
    let raw = SPEC.load(Ordering::Acquire);
    if raw == 0 {
        None
    } else {
        Some(KeySpec::decode(raw))
    }
}

/// 修饰键是否与期望一致（用 GetAsyncKeyState 查全局键态）
fn modifiers_match(spec: &KeySpec) -> bool {
    let down = |vk: i32| unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000 != 0;
    down(VK_CONTROL.0 as i32) == spec.ctrl
        && down(VK_SHIFT.0 as i32) == spec.shift
        && down(VK_MENU.0 as i32) == spec.alt
        && (down(VK_LWIN.0 as i32) || down(VK_RWIN.0 as i32)) == spec.win
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // 【约束】这里同步阻塞全系统输入：只允许原子量比较 + 投递，禁止日志/锁/文件 IO
    if code >= 0 {
        let raw = SPEC.load(Ordering::Acquire);
        if raw != 0 {
            let spec = KeySpec::decode(raw);
            let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            if kb.vkCode == spec.vk {
                let msg = wparam.0 as u32;
                match msg {
                    WM_KEYDOWN | WM_SYSKEYDOWN => {
                        // swap 返回 true 说明这是自动重复，忽略（等价 MOD_NOREPEAT）
                        if !KEY_DOWN.swap(true, Ordering::AcqRel) && modifiers_match(&spec) {
                            if let Some(trigger) = TRIGGER.get() {
                                trigger();
                            }
                        }
                    }
                    WM_KEYUP | WM_SYSKEYUP => {
                        KEY_DOWN.store(false, Ordering::Release);
                    }
                    _ => {}
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 设置页录制器产出的键名必须全部可解析（与前端 normalizeHotkeyKey 一一对应）
    #[test]
    fn parses_every_key_name_the_recorder_can_produce() {
        for (spec, vk) in [
            ("F9", 0x78u32),
            ("F1", 0x70),
            ("F24", 0x87),
            ("A", 0x41),
            ("z", 0x5A),
            ("0", 0x30),
            ("Space", 0x20),
            ("Esc", 0x1B),
            ("Enter", 0x0D),
            ("Backspace", 0x08),
            ("Delete", 0x2E),
            ("Tab", 0x09),
            ("Up", 0x26),
            ("PageDown", 0x22),
            ("PrintScreen", 0x2C),
            ("Minus", 0xBD),
            ("Slash", 0xBF),
            ("Backquote", 0xC0),
        ] {
            let parsed = parse_key_spec(spec).unwrap_or_else(|| panic!("{spec} 应可解析"));
            assert_eq!(parsed.vk, vk, "{spec} 的虚拟键码不对");
            assert!(!parsed.ctrl && !parsed.shift && !parsed.alt && !parsed.win);
        }
    }

    /// 修饰键组合（顺序与设置页 join 顺序一致：Ctrl → Shift → Alt → Super）
    #[test]
    fn parses_modifier_combinations() {
        let spec = parse_key_spec("Ctrl+Shift+S").unwrap();
        assert_eq!(spec.vk, 0x53);
        assert!(spec.ctrl && spec.shift && !spec.alt && !spec.win);

        let spec = parse_key_spec("Super+F12").unwrap();
        assert_eq!(spec.vk, 0x7B);
        assert!(spec.win && !spec.ctrl);
    }

    /// 空串 / 未知键名 / 畸形组合一律返回 None（退化到注册热键通道，不装钩子）
    #[test]
    fn rejects_unparsable_specs() {
        assert_eq!(parse_key_spec(""), None);
        assert_eq!(parse_key_spec("   "), None);
        assert_eq!(parse_key_spec("F25"), None);
        assert_eq!(parse_key_spec("Mute"), None);
        assert_eq!(parse_key_spec("Ctrl+"), None);
        assert_eq!(parse_key_spec("Ctrl"), None, "只有修饰键不算热键");
        assert_eq!(parse_key_spec("A+B"), None, "不允许两个主键");
    }

    /// 编码/解码必须可逆（钩子回调靠原子整型传键位，0 表示停用）
    #[test]
    fn spec_encoding_roundtrip() {
        let spec = parse_key_spec("Ctrl+Alt+F9").unwrap();
        let raw = spec.encode();
        assert_ne!(raw, 0, "启用的键编码不能为 0（0 被当作停用）");
        assert_eq!(KeySpec::decode(raw), spec);

        set_spec(Some(spec));
        assert_eq!(current_spec(), Some(spec));
        set_spec(None);
        assert_eq!(current_spec(), None);
    }
}
