//! 截图管理器：抓帧 → 色调映射 → 命名归档 → 保存。
//!
//! 命名规则（与 NVIDIA App / Steam 保持一致的目录组织）：
//!   `{截图目录}/{进程名}/{进程名}_{日期}_{时间}.png`
//!
//! 两条取数路径共用同一套「tonemap → 编码 → 归档」：
//! - WGC（方案 A）：抓 FP16 scRGB → `tonemap_rgba16f_to_srgb`
//! - 注入（方案 C）：抓 backbuffer（R10G10B10A2+PQ 或 R8G8B8A8）→ PQ 解码 → `tonemap_linear_rgba_to_srgb`

use crate::core::tonemap::{self, ToneMapPath};

/// DXGI_FORMAT 常量（与 hook DLL 约定一致）
pub const DXGI_FORMAT_R16G16B16A16_FLOAT: u32 = 10; // scRGB HDR，D3D12 常见
pub const DXGI_FORMAT_R10G10B10A2_UNORM: u32 = 24; // HDR10 (PQ)
pub const DXGI_FORMAT_R8G8B8A8_UNORM: u32 = 28; // SDR
pub const DXGI_FORMAT_R8G8B8A8_UNORM_SRGB: u32 = 29; // SDR（gamma 已编码）
pub const DXGI_FORMAT_B8G8R8A8_UNORM: u32 = 87; // SDR（BGRA 内存序，部分老引擎）

/// 一次截图的最终结果
#[derive(Debug, Clone)]
pub struct ScreenshotResult {
    pub path: String,
    pub process_name: String,
    pub width: u32,
    pub height: u32,
    pub tone_map_path: ToneMapPath,
}

/// 从进程名中提取文件 stem（去掉 .exe 扩展名），用于目录与文件命名
pub fn process_stem(exe_name: &str) -> String {
    let name = exe_name.trim();
    let lower = name.to_lowercase();
    let stem = if lower.ends_with(".exe") {
        &name[..name.len() - 4]
    } else {
        name
    };
    let cleaned: String = stem
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect();
    cleaned.trim().to_string()
}

/// 计算某个游戏（进程）的截图目录：`{screenshot_dir}/{process_stem}`
pub fn screenshot_dir_for_process(screenshot_dir: &str, exe_name: &str) -> std::path::PathBuf {
    let expanded = crate::utils::path::expand_env_vars(screenshot_dir);
    std::path::PathBuf::from(expanded).join(process_stem(exe_name))
}

/// 生成一个不重名的截图文件路径（同一秒多次截图自动加序号）
fn build_unique_path(dir: &std::path::Path, stem: &str) -> std::path::PathBuf {
    let now = chrono::Local::now();
    let base = now.format("%Y-%m-%d_%H-%M-%S").to_string();
    let mut candidate = dir.join(format!("{}_{}.png", stem, base));
    let mut i = 1u32;
    while candidate.exists() {
        candidate = dir.join(format!("{}_{}_{}.png", stem, base, i));
        i += 1;
    }
    candidate
}

/// 通用保存：把已 tonemap 好的 sRGB RGBA8 帧编码为 PNG 并归档。
/// WGC 与注入两条路径共用。
pub fn save_srgb_png(
    srgb: &[u8],
    width: u32,
    height: u32,
    process_name: &str,
    screenshot_dir: &str,
    tone_path: ToneMapPath,
) -> anyhow::Result<ScreenshotResult> {
    let stem = process_stem(process_name);
    let dir = screenshot_dir_for_process(screenshot_dir, process_name);
    std::fs::create_dir_all(&dir)
        .map_err(|e| anyhow::anyhow!("创建截图目录失败 {}: {e}", dir.display()))?;
    let path = build_unique_path(&dir, &stem);

    tonemap::encode_png(srgb, width, height, &path)?;

    tracing::info!("截图已保存: {} (tonemap: {:?})", path.display(), tone_path);
    Ok(ScreenshotResult {
        path: path.to_string_lossy().to_string(),
        process_name: stem,
        width,
        height,
        tone_map_path: tone_path,
    })
}

/// 方案 A：从指定 HWND 抓取一帧并保存（WGC 路径）。
pub fn capture_and_save(
    hwnd: isize,
    process_name: &str,
    screenshot_dir: &str,
) -> anyhow::Result<ScreenshotResult> {
    let frame = crate::core::capture::capture_window_fp16(hwnd)?;
    tracing::info!(
        "WGC 抓到帧 {}x{} ({} bytes Rgba16F)",
        frame.width,
        frame.height,
        frame.rgba16f.len()
    );

    let white_level_scale = 1.0f32;
    let exposure = 1.0f32;
    let (srgb, tone_path) = tonemap::tonemap_rgba16f_to_srgb(
        &frame.rgba16f,
        frame.width,
        frame.height,
        white_level_scale,
        exposure,
    );

    save_srgb_png(&srgb, frame.width, frame.height, process_name, screenshot_dir, tone_path)
}

/// 截图反馈音效（内嵌合成 wav，经 winmm PlaySound 播放，零外部文件依赖）。
///
/// 之前用 `MessageBeep` 播系统提示音：Success=Asterisk、Error=Critical Stop、
/// Notice=Exclamation —— 全是 Windows 报错腔，游戏内听感刺耳且三者区分度差。
/// 现改为三段内嵌音效（`include_bytes!` 静态存储，SND_ASYNC 异步播放不阻塞热键线程）：
/// - Success: 相机快门「咔嚓」（两次机械 click，Steam 截图同风格）
/// - Error:   柔和低音「咚」（180Hz 指数衰减，不再用刺耳的 Critical Stop）
/// - Notice:  轻 UI tick（短促高频），提示无目标/前台不匹配
#[derive(Debug, Clone, Copy)]
pub enum FeedbackTone {
    /// 截图成功：快门音
    Success,
    /// 截图失败/出错：低音提示
    Error,
    /// 收到按键但无可截图目标（无活跃游戏 / 前台窗口不匹配）
    Notice,
}

/// SND_MEMORY 播放的内存映像必须是 WAV 文件格式字节流；
/// include_bytes! 得到的 &'static [u8] 常驻只读段，满足 SND_ASYNC 的
/// 「播放期间缓冲必须持续有效」要求，无生命周期问题。
static SOUND_SHUTTER: &[u8] = include_bytes!("../../assets/sounds/shutter.wav");
static SOUND_ERROR: &[u8] = include_bytes!("../../assets/sounds/error.wav");
static SOUND_NOTICE: &[u8] = include_bytes!("../../assets/sounds/notice.wav");

/// 播放截图反馈音效。
///
/// SND_ASYNC：异步，热键回调线程立即返回；SND_NODEFAULT：声卡设备异常时
/// 静默失败，不回落播放系统默认提示音（避免再次出现"Windows 报错音"）。
/// 连续两次 play_feedback 时后者自动打断前者（PlaySound 语义），符合
/// "Notice(受理) → Error(失败)" 的连播场景。
pub fn play_feedback(tone: FeedbackTone) {
    use windows::core::PCWSTR;
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

    let data = match tone {
        FeedbackTone::Success => SOUND_SHUTTER,
        FeedbackTone::Error => SOUND_ERROR,
        FeedbackTone::Notice => SOUND_NOTICE,
    };

    unsafe {
        // hmod 在 SND_MEMORY 模式下被忽略，传默认空句柄
        let _ = PlaySoundW(
            PCWSTR(data.as_ptr() as *const u16),
            None,
            SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
        );
    }
}

/// 逐行提取像素，剥离行 padding（staging Map 返回的 RowPitch 可能 > width * bpp）。
fn extract_rows(raw: &[u8], width: u32, height: u32, row_pitch: u32, bpp: u32) -> Vec<u8> {
    let row_bytes = (width * bpp) as usize;
    let mut out = Vec::with_capacity(row_bytes * height as usize);
    for row in 0..height {
        let start = (row * row_pitch) as usize;
        let end = start.saturating_add(row_bytes);
        if end <= raw.len() {
            out.extend_from_slice(&raw[start..end]);
        } else {
            break;
        }
    }
    out
}

/// 方案 C：处理注入回传的 backbuffer 原始帧并保存。
///
/// 参数：
/// - `raw`: 按 RowPitch 排布的原始字节（`row_pitch * height` 字节）
/// - `format`: DXGI_FORMAT
///
/// 格式分派表（D3D11/D3D12 backbuffer 的全部常见变体）：
/// | 格式 | 场景 | 处理 |
/// |---|---|---|
/// | R10G10B10A2_UNORM (24) | HDR10 | PQ 解码 → tonemap |
/// | R16G16B16A16_FLOAT (10) | scRGB HDR | f16 → f32 线性 → tonemap |
/// | R8G8B8A8_UNORM (28) | SDR | 直接保存 |
/// | R8G8B8A8_UNORM_SRGB (29) | SDR | 直接保存 |
/// | B8G8R8A8_UNORM (87) | SDR（BGRA） | 通道交换后保存 |
pub fn process_and_save_raw_frame(
    raw: &[u8],
    width: u32,
    height: u32,
    format: u32,
    row_pitch: u32,
    process_name: &str,
    screenshot_dir: &str,
) -> anyhow::Result<ScreenshotResult> {
    match format {
        DXGI_FORMAT_R10G10B10A2_UNORM => {
            let packed = extract_rows(raw, width, height, row_pitch, 4);
            let linear = tonemap::decode_r10g10b10a2_pq(&packed, width, height);
            let (srgb, tone_path) = tonemap::tonemap_linear_rgba_to_srgb(&linear, 1.0, 1.0);
            save_srgb_png(&srgb, width, height, process_name, screenshot_dir, tone_path)
        }
        DXGI_FORMAT_R16G16B16A16_FLOAT => {
            // scRGB HDR：线性域，1.0 = SDR 白（80 nits）。
            // 与 WGC 的 Rgba16F 路径完全同构（见 capture_and_save）。
            let packed = extract_rows(raw, width, height, row_pitch, 8);
            let linear = tonemap::decode_r16g16b16a16_scrgb(&packed, width, height);
            let (srgb, tone_path) = tonemap::tonemap_linear_rgba_to_srgb(&linear, 1.0, 1.0);
            save_srgb_png(&srgb, width, height, process_name, screenshot_dir, tone_path)
        }
        DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => {
            // SDR 游戏：R8G8B8A8 已接近显示就绪，直接保存
            let packed = extract_rows(raw, width, height, row_pitch, 4);
            save_srgb_png(
                &packed,
                width,
                height,
                process_name,
                screenshot_dir,
                ToneMapPath::DirectSrgb,
            )
        }
        DXGI_FORMAT_B8G8R8A8_UNORM => {
            // BGRA 内存序：交换 R/B 通道后按 RGBA 保存
            let packed = extract_rows(raw, width, height, row_pitch, 4);
            let rgba = bgra_to_rgba(&packed);
            save_srgb_png(
                &rgba,
                width,
                height,
                process_name,
                screenshot_dir,
                ToneMapPath::DirectSrgb,
            )
        }
        other => anyhow::bail!(
            "不支持的 DXGI_FORMAT: {other}（期待 10/24/28/29/87 之一）"
        ),
    }
}

/// BGRA → RGBA 通道交换（原地语义的纯函数版）
fn bgra_to_rgba(bgra: &[u8]) -> Vec<u8> {
    let mut out = bgra.to_vec();
    for px in out.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    out
}
