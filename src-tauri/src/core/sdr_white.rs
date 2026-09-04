//! 显示器 SDR 白电平查询（HDR→SDR 归一化系数）。
//!
//! 背景：HDR 开启时，Windows「SDR 内容亮度」滑块允许把 SDR 白设在 80~480 nits
//! （默认 80，见 OBS Studio hdr-tonemap-filter 的同名参数范围 80-480）。
//! WGC 抓到的 scRGB 帧里，SDR 白的线性值 = nits/80，若按 1.0 硬编码：
//! - 滑块 <80nits（常见下调）→ 截图整体偏暗；
//! - 滑块 >80nits → SDR 内容亮度超过 1.0，被 tonemap 误判为 HDR 高光而压灰。
//!
//! API 语义（微软文档）：`DISPLAYCONFIG_SDR_WHITE_LEVEL.SDRWhiteLevel` 的单位是
//! 「1/1000 × 80nits」，即 1000 = 80nits。换算：
//! ```text
//! nits          = SDRWhiteLevel * 80 / 1000
//! scRGB 白电平  = nits / 80 = SDRWhiteLevel / 1000
//! ```
//!
//! 流程：窗口 → HMONITOR → GDI 设备名 → 匹配 QueryDisplayConfig 活动路径 →
//! 对该路径的 source 查 SDR 白电平。任何失败一律回落 1.0（80nits 标准语义），
//! 与旧行为完全一致，保证 SDR 屏与查询异常时零回归。

use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SDR_WHITE_LEVEL, DISPLAYCONFIG_SOURCE_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};

/// 查询窗口所在显示器的 SDR 白电平（scRGB 归一化系数 = nits/80）。
///
/// HDR 滑块在默认 80nits 时返回 1.0；滑块 40nits 返回 0.5；480nits 返回 6.0。
/// 查询失败回落 1.0。
pub fn sdr_white_scale_for_window(hwnd: isize) -> f32 {
    let scale = query_sdr_white_scale(hwnd);
    if scale > 0.0 {
        scale
    } else {
        1.0
    }
}

fn query_sdr_white_scale(hwnd: isize) -> f32 {
    unsafe {
        // 1. 窗口 -> 所在显示器 -> GDI 设备名（如 \\.\DISPLAY1）
        let hmon = MonitorFromWindow(
            HWND(hwnd as *mut core::ffi::c_void),
            MONITOR_DEFAULTTONEAREST,
        );
        if hmon.is_invalid() {
            return 0.0;
        }
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        // GetMonitorInfoW 接受 MONITORINFO 指针，按 cbSize 识别实际为 EXW 结构
        if !GetMonitorInfoW(hmon, &mut mi as *mut MONITORINFOEXW as *mut _).as_bool() {
            return 0.0;
        }
        let device = mi.szDevice;

        // 2. 枚举当前活动的显示路径
        let (mut num_paths, mut num_modes) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut num_paths, &mut num_modes)
            .is_err()
        {
            return 0.0;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); num_paths as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); num_modes as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut num_paths,
            paths.as_mut_ptr(),
            &mut num_modes,
            modes.as_mut_ptr(),
            None,
        )
        .is_err()
        {
            return 0.0;
        }

        // 3. 按 GDI 设备名匹配路径，再查该路径的 SDR 白电平
        for path in &paths[..num_paths as usize] {
            let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            src.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: path.sourceInfo.adapterId,
                id: path.sourceInfo.id,
            };
            if DisplayConfigGetDeviceInfo(&mut src.header) != 0 {
                continue;
            }
            if !wide_eq(&device, &src.viewGdiDeviceName) {
                continue;
            }

            let mut white = DISPLAYCONFIG_SDR_WHITE_LEVEL::default();
            white.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
                size: std::mem::size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32,
                adapterId: path.sourceInfo.adapterId,
                id: path.sourceInfo.id,
            };
            if DisplayConfigGetDeviceInfo(&mut white.header) != 0 {
                continue;
            }
            // SDRWhiteLevel 单位 = 1/1000 × 80nits → nits/80 = 值/1000
            return white.SDRWhiteLevel as f32 / 1000.0;
        }
        0.0
    }
}

/// 按 NUL 结尾语义比较两个 UTF-16 定长设备名数组
/// （MONITORINFO 的 szDevice 与 DISPLAYCONFIG 的 viewGdiDeviceName
/// 均保证 NUL 结尾，但尾部未用空间不保证清零，不能整组直接比较）。
fn wide_eq(a: &[u16; 32], b: &[u16; 32]) -> bool {
    let len_a = a.iter().position(|&c| c == 0).unwrap_or(a.len());
    let len_b = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    len_a == len_b && a[..len_a] == b[..len_b]
}
