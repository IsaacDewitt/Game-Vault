//! Steam 客户端自报的 App 运行状态（只读注册表）
//!
//! `HKCU\SOFTWARE\Valve\Steam\Apps\<appid>` 下由 Steam 自己维护三个 DWORD：
//! `Installed` / `Running` / `Updating`。相比「猜游戏目录里有没有进程」，
//! 这是客户端写下的权威状态，可以干净地区分「正在下载更新」与「游戏已开跑」。
//!
//! 【为什么不引入 winreg / 不给 windows crate 开 Win32_System_Registry feature】
//! 依赖清单一个字都不改：下面的两个 API 来自系统自带 advapi32.dll，
//! 用 `#[link]` 直接声明即可。读的是 HKCU 当前用户键，同完整性级别可读，
//! 不需要管理员权限，也不会弹出 UAC。
//!
//! 可靠性边界（StackOverflow / EpicMorg/SteamPathsLib 的公开实践）：
//! 个别游戏的 `Running` 值不可靠（如 ABZU 启动后又被置 0），因此本模块
//! **只用来判定「更新中」**，绝不单独用来判定「游戏正在运行」——
//! 后者仍以 `tracker::find_game_process` 的进程探测为准。

/// Steam 为某个 appid 写下的状态快照
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SteamAppState {
    pub running: bool,
    pub updating: bool,
}

impl SteamAppState {
    /// 是否应把「启动请求」继续按等待处理，而不是当成游戏已开跑。
    ///
    /// `Running=1` 优先于 `Updating=1`：Steam 在少数情况下会让两个值同时为 1，
    /// 此时游戏确实已经在跑，按更新阻断反而会漏计时。
    pub fn blocking_update(self) -> bool {
        self.updating && !self.running
    }
}

/// 读取指定 Steam appid 的状态；读不到（未安装 / 键不存在 / 权限异常）一律返回全 false，
/// 行为退化为旧的「固定待命窗口」，不阻断任何正常启动。
pub fn app_state(app_id: &str) -> SteamAppState {
    if app_id.is_empty() || !app_id.bytes().all(|b| b.is_ascii_digit()) {
        return SteamAppState::default();
    }
    read_app_state(app_id)
}

#[cfg(windows)]
fn read_app_state(app_id: &str) -> SteamAppState {
    let subkey = format!(r"SOFTWARE\Valve\Steam\Apps\{}", app_id);
    SteamAppState {
        running: query_dword(&subkey, "Running").unwrap_or(0) != 0,
        updating: query_dword(&subkey, "Updating").unwrap_or(0) != 0,
    }
}

/// 非 Windows 平台（本项目只出 Windows 便携版，仅为保持可编译）：
/// 全 false = 不阻断，沿用固定待命窗口。
#[cfg(not(windows))]
fn read_app_state(_app_id: &str) -> SteamAppState {
    SteamAppState::default()
}

#[cfg(windows)]
mod registry {
    use std::os::windows::ffi::OsStrExt;
    use std::ffi::OsStr;

    const HKEY_CURRENT_USER: isize = 0x8000_0001_i64 as isize;
    const KEY_READ: u32 = 0x0002_0019;
    const REG_DWORD: u32 = 4;
    const ERROR_SUCCESS: i32 = 0;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            hkey: isize,
            subkey: *const u16,
            options: u32,
            sam_desired: u32,
            result: *mut isize,
        ) -> i32;
        fn RegQueryValueExW(
            hkey: isize,
            value_name: *const u16,
            reserved: *const u32,
            value_type: *mut u32,
            data: *mut u8,
            data_len: *mut u32,
        ) -> i32;
        fn RegCloseKey(hkey: isize) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// 打开子键读一个 DWORD；任何一步失败都返回 None（保守侧）。
    pub(super) fn query_dword(subkey: &str, value_name: &str) -> Option<u32> {
        let sk = wide(subkey);
        let vn = wide(value_name);
        let mut key: isize = 0;
        // SAFETY: sk/vn 都是以 0 结尾的 UTF-16 常量串；key 仅作输出句柄，
        // 后续无论成功与否都会 RegCloseKey。
        let rc = unsafe {
            RegOpenKeyExW(HKEY_CURRENT_USER, sk.as_ptr(), 0, KEY_READ, &mut key)
        };
        if rc != ERROR_SUCCESS {
            return None;
        }

        let mut data: u32 = 0;
        let mut len: u32 = std::mem::size_of::<u32>() as u32;
        let mut value_type: u32 = 0;
        let rc = unsafe {
            RegQueryValueExW(
                key,
                vn.as_ptr(),
                std::ptr::null(),
                &mut value_type,
                &mut data as *mut u32 as *mut u8,
                &mut len,
            )
        };
        unsafe {
            RegCloseKey(key);
        }

        (rc == ERROR_SUCCESS && value_type == REG_DWORD && len == 4).then_some(data)
    }
}

#[cfg(windows)]
use registry::query_dword;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocking_update_requires_updating_without_running() {
        assert!(SteamAppState { running: false, updating: true }.blocking_update());
        // Running 优先：客户端说游戏在跑，就不能因 Updating 残留而阻断计时
        assert!(!SteamAppState { running: true, updating: true }.blocking_update());
        assert!(!SteamAppState::default().blocking_update());
    }

    #[test]
    fn invalid_app_id_never_touches_registry() {
        assert_eq!(app_state(""), SteamAppState::default());
        assert_eq!(app_state("steam://rungameid/4115450"), SteamAppState::default());
        assert_eq!(app_state("4115450;DROP"), SteamAppState::default());
    }
}
