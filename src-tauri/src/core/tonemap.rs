//! 色调映射：把 WGC 拿到的 HDR 原始帧压回 SDR sRGB。
//!
//! 这是本功能画质的胜负手。设计源自对三个工业级开源实现的对齐调研：
//! - OBS Studio 官方 `hdr-tonemap-filter`（同为"桌面合成内容→SDR"场景）；
//! - libplacebo/mpv（HDR→SDR 色彩科学权威，knee 型 EETF + 亮度域 + 去饱和）；
//! - hdrfix（Brion Vibber 的 HDR 截图转 SDR 工具，同为 Rust）。
//!
//! 三者共识与本模块的对应实现：
//! 1. **白电平必须参数化**：scRGB 帧里 SDR 白 = 系统滑块 nits/80（80~480），
//!    由 `sdr_white` 模块查询传入，绝不硬编码（OBS 的 sdr_white_level 滑块同款）。
//! 2. **SDR 内容尽量零损失**：亮度低于 SDR 白的部分恒等直通——截图与视频不同，
//!    视频可把 SDR 白压到中灰（BT.2408 reference white），截图观感必须
//!    "原样"，只有真正的 HDR 高光需要处理。原裸 Reinhard L/(1+L) 会把
//!    SDR 白压到 0.5、中灰压暗一成，正是被推翻的旧实现。
//! 3. **高光走单调压缩**（2026-10-24 重做，来源见下）：改用 Khronos PBR Neutral
//!    的压缩支（略去 F90 toe 以保黑位）。归一化到 SDR 白 = 1.0 后，0.76 以下
//!    逐字节恒等；0.76 以上按 max(R,G,B) 单调压向 1.0，越亮越向白去饱和。
//!    保色相、C¹ 连续、无硬裁剪、解析可逆（Blender 4.2 / three.js / Filament 同款）。
//!
//!    【血案，勿回退】此前的实现是「恒等段 + 反向下压的肩部」：
//!    `scale = (1 - 0.35*h)/n` ⇒ 输出亮度 = `1 - 0.35*h`，而 h 随 n 单调递增，
//!    于是帧内越亮输出越暗：360nit→255、480nit→251、640nit→238、1000nit→211。
//!    那不是 tone curve，是高光倒挂 —— SDR 白与峰值之间的像素全被压到比白墙更闷。
//!    由 `test_extended_range_is_monotonic` 钉死，改前先跑它。
//!
//!    【有意的代价】含 HDR 高光的帧里，SDR 白从 255 让到 241（线性 0.88），
//!    腾出的 14 个码阶给高光分层。8bit 只有 256 格，"完美锁 SDR"与"高光有层次"
//!    数学上互斥；纯 SDR 帧（max ≤ SDR 白）不进这条路径，仍逐字节不变。
//! 4. **越界色保亮度收敛**：宽色域（Rec.2020/P3）内容在 scRGB 中会出现
//!    负分量；超白高光会出现 >1 分量。按 libplacebo 的 desaturate 模式
//!    （保 CIE 亮度向灰轴收敛）单步精确求解收缩系数。
//!
//! 注入方案用的 PQ 解码 / R10G10B10A2 解码 / 线性帧入口同样存于 injection 分支
//! （且其 PQ 值域与白电平语义不一致，恢复前需先修正），master 不保留。

use std::sync::OnceLock;

use half::f16;

/// PBR Neutral 压缩起点（官方 Ks − F90 = 0.8 − 0.04）。
/// 归一化到「SDR 白 = 1.0」之后，线性亮度低于此值的像素逐字节恒等。
const PBR_START: f32 = 0.8 - 0.04;

/// PBR Neutral 去饱和速度（官方 Kd）：峰值处按此速度向白收敛，
/// 让"更亮"在高光段仍有可感知的方向，而不是靠裁剪。
const PBR_DESAT: f32 = 0.15;

/// 三级判定结果，决定走哪条编码路径
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToneMapPath {
    /// 纯 SDR 内容且白电平为标准 1.0，零变换直接 sRGB 编码，与 Steam 完全一致
    DirectSrgb,
    /// SDR 内容整体随系统 SDR 白电平线性缩放，除回白电平系数即无损还原
    DivideWhiteLevel,
    /// 存在超过 SDR 白的内容（HDR / AutoHDR 扩展范围），走 PBR Neutral 单调压缩
    ExtendedRange,
}

/// 一帧 HDR 数据的亮度统计（第一遍扫描得到）
#[derive(Debug, Clone, Copy)]
struct LuminanceStats {
    /// 最大线性亮度
    max: f32,
    /// 亮度超过 SDR 白的像素数（诊断用：区分"整帧真 HDR"与"个别高光点"）
    over_white: usize,
}

impl LuminanceStats {
    fn from_frame(rgba: &[f32], ws: f32) -> Self {
        let mut max = 0.0f32;
        let mut over_white = 0usize;
        for px in rgba.chunks_exact(4) {
            let l = luminance(px[0], px[1], px[2]);
            if l > max {
                max = l;
            }
            if l > ws {
                over_white += 1;
            }
        }
        Self { max, over_white }
    }
}

/// 计算 Rec.709 亮度（线性域）
#[inline]
fn luminance(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// 线性 → sRGB（标准分段 gamma）
#[inline]
fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

// ==================== sRGB 编码查找表（2026-09-22：替掉逐像素 powf） ====================
//
// 【为什么】线性→sRGB 编码里的 `powf(1/2.4)` 是整条截图链路最贵的单项：2560×1440 一帧要走
// 1106 万次 powf，实测单线程 101.3 ms。而这一步是**纯函数**（linear 闭区间 [0,1] → u8），
// 完全可以查表。用 65536 项的表把 [0,1] 量化到 16 位，量化步长 1/131070 远细于 8bit 输出的
// 1/255，实测结果与逐像素 powf 只在极少数取值上差 1 个 LSB（见下方一致性测试）。
//
// 选 65536 而不是更小 + 插值：实测 4096 项 + 线性插值反而更慢（47.9 ms，插值本身要算），
// 65536 直接查表只要 26.4 ms；而 64 KiB 的表能常驻 L2，代价可忽略。

/// linear → sRGB 8bit 的编码表（惰性构建一次）
static SRGB_LUT: OnceLock<Box<[u8; 65536]>> = OnceLock::new();

fn srgb_lut() -> &'static [u8; 65536] {
    SRGB_LUT.get_or_init(|| {
        let mut table = Box::new([0u8; 65536]);
        for (i, slot) in table.iter_mut().enumerate() {
            // 与逐像素实现同一条公式，保证"表里每个点 = 该点的精确值"
            let s = linear_to_srgb(i as f32 / 65535.0);
            *slot = (s * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        table
    })
}

/// sRGB 编码（LUT 版）：把线性值量化到 16 位后查表。
///
/// 量化用 `round` 而不是截断：截断会让表内每个采样点的误差单向偏移，四舍五入则正负抵消。
/// 越界值与 NaN 的处理与旧实现完全一致（`as usize` 是饱和转换：负数与 NaN 都落到 0）。
#[inline]
fn encode_srgb(c: f32) -> u8 {
    let q = (c.clamp(0.0, 1.0) * 65535.0).round();
    srgb_lut()[(q as usize).min(65535)]
}

/// alpha 编码：线性直通，无 gamma（与旧实现同式）
#[inline]
fn encode_alpha(a: f32) -> u8 {
    (a.clamp(0.0, 1.0) * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Khronos PBR Neutral 压缩支（略去 F90 toe，保黑位）。
///
/// 输入已归一化到「SDR 白 = 1.0」。低于 `PBR_START` 原样返回；以上按
/// `max(R,G,B)` 单调压向 1.0，并按 `PBR_DESAT` 向白去饱和。三个通道同乘一个
/// 标量（再向白插值），故色相不偏；曲线一阶导在拐点连续，不会出色带。
///
/// 数学性质（Khronos 规范）：非负输入必得 [0,1] 输出，且解析可逆。
#[inline]
fn compress_extended(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let peak = r.max(g).max(b);
    if peak < PBR_START {
        return (r, g, b);
    }
    let d = 1.0 - PBR_START;
    // 单调递增、渐近于 1.0：peak = PBR_START 时等于 PBR_START，peak → ∞ 时 → 1
    let new_peak = 1.0 - d * d / (peak + d - PBR_START);
    let scale = new_peak / peak;
    // 去饱和权重：峰值与压缩后白点差得越多越向白靠（官方 Kd 公式）
    let keep = 1.0 / (PBR_DESAT * (peak - new_peak) + 1.0);
    let f = |c: f32| new_peak + (c * scale - new_peak) * keep;
    (f(r), f(g), f(b))
}

/// 单像素核心：路径分派 → 越界色收敛 → sRGB 编码。返回该像素的 RGBA8 四字节。
#[inline]
fn tonemap_pixel(px: &[f32], path: ToneMapPath, ws: f32) -> [u8; 4] {
    let (mut r, mut g, mut b) = (px[0], px[1], px[2]);

    match path {
        ToneMapPath::DirectSrgb => {
            // 零变换：值域未越界且白电平标准，直接编码
        }
        ToneMapPath::DivideWhiteLevel => {
            // 系统把 SDR 内容按 nits/80 线性缩放，除回即无损还原
            r /= ws;
            g /= ws;
            b /= ws;
        }
        ToneMapPath::ExtendedRange => {
            // 归一化到 SDR 白 = 1.0，再走单调压缩：
            // 0.76 以下（SDR 内容主体）逐字节恒等，以上按亮度分层
            let (cr, cg, cb) = compress_extended(r / ws, g / ws, b / ws);
            r = cr;
            g = cg;
            b = cb;
        }
    }

    // 公共出口：宽色域负分量 / 超界分量的保亮度收敛
    // （DirectSrgb/DivideWhiteLevel 路径的正常像素此处为 no-op）
    let (dr, dg, db) = desaturate(r, g, b);
    [
        encode_srgb(dr),
        encode_srgb(dg),
        encode_srgb(db),
        encode_alpha(px[3]),
    ]
}

/// 越界色收敛：保亮度把色域外颜色（任一通道 <0 的宽色域负分量，或 >1 的
/// 超白分量）向灰轴精确收缩回 sRGB 色域（libplacebo desaturate 模式同思路）。
///
/// 单步精确解：收缩 c' = l + (c-l)*k，解 k 使 max 通道恰好回到 1.0 或
/// min 通道恰好回到 0.0，亮度 l 严格不变。特例：
/// - 亮度非正（如极暗宽色域色）：无法保亮度，直接置黑；
/// - 亮度 ≥ 1（纯白高光整体超域）：保亮度不可能（输出上限 1.0），
///   退化为整体等比缩放至白点，色相保持。
#[inline]
fn desaturate(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max_c = r.max(g).max(b);
    let min_c = r.min(g).min(b);
    if max_c <= 1.0 && min_c >= 0.0 {
        return (r, g, b);
    }
    let l = luminance(r, g, b);
    if l <= 1e-9 {
        return (0.0, 0.0, 0.0);
    }
    if l >= 1.0 {
        let k = 1.0 / max_c;
        return (r * k, g * k, b * k);
    }
    let mut k = 1.0f32;
    if max_c > 1.0 {
        k = k.min((1.0 - l) / (max_c - l));
    }
    if min_c < 0.0 {
        k = k.min(l / (l - min_c));
    }
    (
        l + (r - l) * k,
        l + (g - l) * k,
        l + (b - l) * k,
    )
}

/// 单帧 tonemap 核心。
///
/// 参数：
/// - `src`: Rgba16F 无 padding 的原始字节（每像素 8 字节，R,G,B,A 各 2 字节 f16）
/// - `width` / `height`: 帧尺寸
/// - `white_level_scale`: 帧内 SDR 白的线性值（`SDRWhiteLevel/1000` = nits/80，
///   由 `sdr_white::sdr_white_scale_for_window` 查询传入；SDR 屏恒为 1.0）。
///
/// 返回 sRGB RGBA8 字节（每像素 4 字节，无 padding），以及本次实际采用的编码路径。
pub fn tonemap_rgba16f_to_srgb(
    src: &[u8],
    width: u32,
    height: u32,
    white_level_scale: f32,
) -> (Vec<u8>, ToneMapPath) {
    let pixel_count = (width as usize) * (height as usize);
    let mut linear: Vec<f32> = Vec::with_capacity(pixel_count * 4);

    // 第一遍：FP16 解码为 f32 线性（半精度转单精度）
    for px in src.chunks_exact(8) {
        for i in 0..4 {
            let lo = px[i * 2];
            let hi = px[i * 2 + 1];
            let bits = u16::from_le_bytes([lo, hi]);
            linear.push(f16::from_bits(bits).to_f32());
        }
    }

    // 若字节数不足（padding 未剥净等异常），按已解析像素继续
    let parsed_pixels = linear.len() / 4;

    // 第一遍统计亮度（用原始线性值判定）
    let ws = if white_level_scale > 0.0 { white_level_scale } else { 1.0 };
    let stats = LuminanceStats::from_frame(&linear, ws);

    // 三级判定：ws 是帧内 SDR 白的真实线性值
    // 容差不是随手加的：WGC 帧是 Rgba16F，SDR 白在帧里就是 f16(ws)。
    // SDRWhiteLevel/1000 有近一半档位不能被 f16 精确表示（如 2.4 → 2.400390625），
    // 此时 stats.max 会比 ws 大半个 ulp —— 无容差判定会把**纯 SDR 帧**判成扩展范围，
    // 全帧白从 255 让到 241、0.76×ws 以上的颜色被去饱和。
    // f16 相对舍入上限 2^-11 ≈ 4.88e-4，取 1.001 覆盖它，代价是超过白电平 0.1% 以内的
    // 内容按 SDR 处理（最多差 1 个码阶）。
    let sdr_tolerance = 1.001;
    let path = if stats.max <= ws * sdr_tolerance {
        if (ws - 1.0).abs() < 1e-6 {
            ToneMapPath::DirectSrgb
        } else {
            ToneMapPath::DivideWhiteLevel
        }
    } else {
        ToneMapPath::ExtendedRange
    };

    // 诊断：扩展范围帧打出 max/ws 与超白像素占比。
    // 【为什么要有这两项】2026-10-23 排查"地铁 2033 截图过曝"时，日志里只有
    // tonemap 路径名，无法区分"整帧真 HDR""AutoHDR 抬亮"还是"一两个热点击穿"，
    // 白白多花了一轮取证。有这两个数就能一眼定位。
    if path == ToneMapPath::ExtendedRange {
        let total = parsed_pixels.max(1);
        tracing::info!(
            "[tonemap] 扩展范围帧：max={:.3}（≈{:.0} nit），SDR 白={:.3}（≈{:.0} nit），\
             max/ws={:.2}，超白像素 {:.3}%",
            stats.max,
            stats.max * 80.0,
            ws,
            ws * 80.0,
            stats.max / ws,
            stats.over_white as f64 / total as f64 * 100.0
        );
    }

    // 第二遍：逐像素映射 + 编码（见 `tonemap_pixel`：数学与重构前逐字一致，
    // 唯一变化是 sRGB 编码改走 65536 项查找表，替掉每像素 3 次 powf）。
    //
    // 【等价性前提】必须**先**完成整帧亮度统计、**再**逐像素处理 —— 路径判定依赖全帧最大亮度，
    // 任何"一边处理一边定路径"的融合优化都会改变输出，不可做。
    let mut out: Vec<u8> = vec![0u8; parsed_pixels * 4];
    for (dst, px) in out.chunks_exact_mut(4).zip(linear.chunks_exact(4)) {
        dst.copy_from_slice(&tonemap_pixel(px, path, ws));
    }

    (out, path)
}

/// 将 sRGB RGBA8 字节编码为 PNG 文件。
///
/// backbuffer / DWM 合成帧的 alpha 恒为 255（截图场景无意义），
/// 剥掉 alpha 按 Rgb 编码：同画质体积省约 25%，编码耗时同步下降。
pub fn encode_png(srgb_rgba8: &[u8], width: u32, height: u32, path: &std::path::Path) -> anyhow::Result<()> {
    let mut rgb: Vec<u8> = Vec::with_capacity(srgb_rgba8.len() / 4 * 3);
    for px in srgb_rgba8.chunks_exact(4) {
        rgb.extend_from_slice(&px[..3]);
    }
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    // 截图场景体积换速度：4K 帧 Default 压缩明显更慢，画质无差异
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&rgb)?;
    // 【关键】显式 finish：写 IEND + flush 并返回 Result。
    // 只靠 Drop 收尾时，磁盘满 / I/O 错误会被静默吞掉 —— 文件截断却返回 Ok。
    writer
        .finish()
        .map_err(|e| anyhow::anyhow!("PNG 收尾失败 {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造 Rgba16F 原始字节（每像素 4 个 f32 值）
    fn fp16_frame(pixels: &[[f32; 4]]) -> Vec<u8> {
        let mut src = Vec::new();
        for px in pixels {
            for v in px {
                src.extend_from_slice(&f16::from_f32(*v).to_bits().to_le_bytes());
            }
        }
        src
    }

    /// 【2026-10-28】纯 SDR 帧在「白电平无法被 f16 精确表示」的档位上必须仍保持 255。
    ///
    /// 1000~6000 逐档实测：49.6% 的 SDRWhiteLevel 档位会被 f16 向上取整
    /// （如 1.4 → 1.400390625），帧内白像素的 max 就比 ws 大半个 ulp。
    /// 路径判定没有容差时，整帧会被误判成 ExtendedRange：SDR 白掉到 241、
    /// 0.76×ws 以上的颜色被去饱和。这条测试钉住那个容差。
    #[test]
    fn test_pure_sdr_frame_with_lossy_f16_white_level_stays_255() {
        let ws = 1.4_f32; // f16(1.4) = 1.400390625 > 1.4
        assert!(
            f16::from_f32(ws).to_f32() > ws,
            "该档位应被 f16 向上取整，否则这条测试失去意义"
        );
        let src = fp16_frame(&[[ws, ws, ws, 1.0], [0.5, 0.5, 0.5, 1.0]]);
        let (out, path) = tonemap_rgba16f_to_srgb(&src, 2, 1, ws);
        assert_eq!(
            path,
            ToneMapPath::DivideWhiteLevel,
            "纯 SDR 帧不得被 f16 舍入误判成扩展范围"
        );
        assert_eq!(out[0], 255, "SDR 白必须仍是 255，实际 {}", out[0]);
    }

    /// 纯 SDR 内容（线性 ≤ 1.0）应走 DirectSrgb 且零变换
    #[test]
    fn test_direct_srgb_zero_transform() {
        let src = fp16_frame(&[[0.5, 0.5, 0.5, 1.0], [1.0, 1.0, 1.0, 1.0]]);
        let (out, path) = tonemap_rgba16f_to_srgb(&src, 2, 1, 1.0);
        assert_eq!(path, ToneMapPath::DirectSrgb);
        // 中灰 0.5 → sRGB ≈ 0.7354 → 188
        assert_eq!(out[0], 188);
        // 纯白 → 255
        assert_eq!(out[4], 255);
    }

    /// 白电平 < 1（系统 SDR 滑块 40nits=0.5）时，被压暗的 SDR 帧应走
    /// DivideWhiteLevel 且精确还原为标准 sRGB 亮度
    #[test]
    fn test_divide_white_level_rescues_dimmed_sdr() {
        // 系统滑块 40nits：SDR 白在帧里是 0.5，中灰 0.25
        let src = fp16_frame(&[[0.25, 0.25, 0.25, 1.0], [0.5, 0.5, 0.5, 1.0]]);
        let (out, path) = tonemap_rgba16f_to_srgb(&src, 2, 1, 0.5);
        assert_eq!(path, ToneMapPath::DivideWhiteLevel);
        // 0.25/0.5=0.5 → 188；SDR 白 0.5/0.5=1.0 → 255，与标准帧完全一致
        assert_eq!(out[0], 188);
        assert_eq!(out[4], 255);
    }

    /// 白电平 > 1（滑块 160nits=2.0）时，SDR 帧不得被误判为 HDR 而压灰
    #[test]
    fn test_white_level_above_one_not_misrouted() {
        let src = fp16_frame(&[[2.0, 2.0, 2.0, 1.0], [1.0, 1.0, 1.0, 1.0]]);
        let (out, path) = tonemap_rgba16f_to_srgb(&src, 2, 1, 2.0);
        assert_eq!(path, ToneMapPath::DivideWhiteLevel);
        // 2.0/2.0=1.0 → 255；1.0/2.0=0.5 → 188
        assert_eq!(out[0], 255);
        assert_eq!(out[4], 188);
    }

    /// SDR 主体在扩展范围帧中仍逐字节恒等：亮度低于压缩起点（0.76×ws）的像素一律不动
    /// —— 这是"截图要原样"的底线（旧裸 Reinhard 会把中灰压暗一成，此测试防回归）。
    ///
    /// 同时钉住那笔**有意的代价**：SDR 白（1.0）在含高光的帧里让到 241，
    /// 腾出的码阶给高光分层；纯 SDR 帧仍是 255。
    #[test]
    fn test_extended_range_keeps_sub_knee_identical() {
        let mixed = fp16_frame(&[
            [0.18, 0.18, 0.18, 1.0],
            [0.5, 0.5, 0.5, 1.0],
            [1.0, 1.0, 1.0, 1.0],
            [3.0, 3.0, 3.0, 1.0],
        ]);
        let sdr_only = fp16_frame(&[
            [0.18, 0.18, 0.18, 1.0],
            [0.5, 0.5, 0.5, 1.0],
            [1.0, 1.0, 1.0, 1.0],
            [1.0, 1.0, 1.0, 1.0],
        ]);
        let (out_mixed, path_mixed) = tonemap_rgba16f_to_srgb(&mixed, 4, 1, 1.0);
        let (out_sdr, _) = tonemap_rgba16f_to_srgb(&sdr_only, 4, 1, 1.0);
        assert_eq!(path_mixed, ToneMapPath::ExtendedRange);
        // 压缩起点以下（0.18 / 0.5）逐字节相等
        for i in 0..2 {
            assert_eq!(
                &out_mixed[i * 4..i * 4 + 4],
                &out_sdr[i * 4..i * 4 + 4],
                "像素 {i} 在 HDR 混合帧中被意外改动"
            );
        }
        // SDR 白：混帧让到 241，纯 SDR 帧保持 255
        assert_eq!(out_mixed[8], 241, "SDR 白应让到 241");
        assert_eq!(out_sdr[8], 255, "纯 SDR 帧不得被压");
        // 3.0 线性（240nit）：单调地高于 SDR 白，且未顶格
        assert!(
            out_mixed[12] > 241 && out_mixed[12] < 255,
            "高光应落在 SDR 白之上且留余量，实际 {}",
            out_mixed[12]
        );
    }

    /// 【血案防线，2026-10-24】扩展范围路径必须**单调不减**：帧内越亮，输出越亮。
    ///
    /// 旧肩部 `scale = (1 - 0.35h)/n` 使输出亮度 = `1 - 0.35h`、随 n 递减，
    /// 实测 360nit→255、480nit→251、640nit→238、1000nit→211 —— 高光倒挂。
    /// 这条测试是这个 bug 的唯一防线：旧测试只用单像素帧，永远踩在曲线终点。
    #[test]
    fn test_extended_range_is_monotonic() {
        let ws = 4.5; // 老爷机器上的真实 SDR 白（360nits）
        let ladder: [f32; 10] = [3.0, 4.5, 4.6, 5.0, 6.0, 8.0, 12.5, 20.0, 25.0, 50.0];
        let pixels: Vec<[f32; 4]> = ladder.iter().map(|v| [*v, *v, *v, 1.0]).collect();
        let src = fp16_frame(&pixels);
        let (out, path) = tonemap_rgba16f_to_srgb(&src, ladder.len() as u32, 1, ws);
        assert_eq!(path, ToneMapPath::ExtendedRange);

        let codes: Vec<u8> = (0..ladder.len()).map(|i| out[i * 4]).collect();
        for (i, w) in codes.windows(2).enumerate() {
            assert!(
                w[1] >= w[0],
                "亮度阶梯在 {:.2}→{:.2} 线性处倒挂：sRGB {}→{}（全梯 {:?}）",
                ladder[i],
                ladder[i + 1],
                w[0],
                w[1],
                codes
            );
        }
        // 峰值必须比 SDR 白落点更亮 —— 否则就是倒挂又回来了
        let peak = *codes.last().unwrap();
        assert!(peak > 241, "峰值应高于 SDR 白（241），实际 {peak}");
    }

    /// 扩展范围峰值应逼近白点：10.0 线性（800nit）落在 250~255，
    /// 而非旧实现的 211（肩底）或裁剪成死白。
    #[test]
    fn test_extended_range_peak_approaches_white() {
        let src = fp16_frame(&[[10.0, 10.0, 10.0, 1.0]]);
        let (out, path) = tonemap_rgba16f_to_srgb(&src, 1, 1, 1.0);
        assert_eq!(path, ToneMapPath::ExtendedRange);
        assert_eq!(out.len(), 4, "1x1 帧应输出 4 字节 RGBA");
        assert!(
            (250..=255).contains(&out[0]),
            "峰值应逼近白点，实际 {}",
            out[0]
        );
    }

    /// 高饱和高光保色相：单通道超白不得被裁成偏色死块。
    /// 线性 (0.5, 3.0, 20.0) 是霓虹蓝，直接裁 255 会得到死蓝 (62,118,255)；
    /// PBR Neutral 按 max 通道等比缩放 + 向白插值，暗通道会被抬起来。
    #[test]
    fn test_extended_range_preserves_hue() {
        let src = fp16_frame(&[[0.5, 3.0, 20.0, 1.0]]);
        let (out, _) = tonemap_rgba16f_to_srgb(&src, 1, 1, 1.0);
        assert!(
            out[2] > out[1] && out[1] > out[0],
            "通道序应保持 B>G>R，实际 {out:?}"
        );
        assert!(out[2] >= 250, "主通道应接近白，实际 {out:?}");
        assert!(out[0] > 62, "暗通道应被去饱和抬起（硬裁会得到 62），实际 {out:?}");
    }

    /// 原生 HDR 极值（4000nits = 50.0 线性）：中灰恒等 + 峰值贴白不 NaN
    #[test]
    fn test_high_peak_4000nits() {
        let src = fp16_frame(&[[0.18, 0.18, 0.18, 1.0], [50.0, 50.0, 50.0, 1.0]]);
        let sdr = fp16_frame(&[[0.18, 0.18, 0.18, 1.0], [1.0, 1.0, 1.0, 1.0]]);
        let (out_hdr, _) = tonemap_rgba16f_to_srgb(&src, 2, 1, 1.0);
        let (out_sdr, _) = tonemap_rgba16f_to_srgb(&sdr, 2, 1, 1.0);
        // 中灰在 4000nits 高光旁保持零损失
        assert_eq!(out_hdr[0], out_sdr[0], "中灰不应被极端高光拖动");
        // 4000nit 峰值贴到白点（254），不得被压到中灰
        assert!(
            (253..=255).contains(&out_hdr[4]),
            "峰值落点异常: {}",
            out_hdr[4]
        );
    }

    /// 宽色域负分量（Rec.2020 高饱和色在 scRGB 中 R<0）：保亮度精确收敛
    #[test]
    fn test_desaturate_negative_wide_gamut() {
        let (r, g, b) = desaturate(-0.2, 0.1, 0.3);
        // 亮度 l=0.0507 不变；k=l/(l-(-0.2)) 使 R 恰好归零
        let l = luminance(r, g, b);
        assert!((l - 0.0507).abs() < 1e-3, "亮度应保持不变，实际 {l}");
        assert!((0.0..1e-4).contains(&r), "负分量应精确收敛到 0，实际 {r}");
        assert!((0.0..=1.0).contains(&g) && (0.0..=1.0).contains(&b));
    }

    /// 超白高饱和色（如 1.2 线性的暖白）：max 通道精确回到 1.0，亮度不变
    #[test]
    fn test_desaturate_overwhite_keeps_luminance() {
        let (r, g, b) = desaturate(1.2, 0.9, 0.1);
        let l_in = 0.2126 * 1.2 + 0.7152 * 0.9 + 0.0722 * 0.1;
        let l_out = luminance(r, g, b);
        assert!((l_in - l_out).abs() < 1e-4, "亮度应不变 {l_in} vs {l_out}");
        assert!(r <= 1.0 + 1e-4 && (r - 1.0).abs() < 1e-3, "max 通道应回到 1.0，实际 {r}");
        assert!((0.0..=1.0).contains(&g) && (0.0..=1.0).contains(&b));
    }

    /// 纯白超亮（亮度 ≥1）：退化为等比缩放至白点，无 NaN；NaN 输入不 panic
    #[test]
    fn test_desaturate_full_white_scales_to_white() {
        let (r, g, b) = desaturate(1.5, 1.5, 1.5);
        assert!((r - 1.0).abs() < 1e-6 && (g - 1.0).abs() < 1e-6 && (b - 1.0).abs() < 1e-6);
        // NaN 通道：比较全为 false → 视为域内原样通过，enc 的 clamp 兜底，不 panic
        let _ = desaturate(f32::NAN, 0.5, 0.5);
    }

    /// LUT 编码 vs 逐像素 powf 精确实现的一致性 —— C 方案（2026-09-22）的回归防线。
    ///
    /// 用伪随机通道值模拟真实帧（不是表的采样点本身），断言：
    /// 最大差异 ≤1 个 8bit 台阶，且出现差异的通道比例 ≤1%。
    #[test]
    fn lut_encoding_matches_exact_powf_within_one_lsb() {
        let mut state: u64 = 0x2545F4914F6CDD1D;
        let mut worst = 0i32;
        let mut differing = 0usize;
        const N: usize = 1_000_000;
        for _ in 0..N {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let c = ((state >> 40) as u32) as f32 / (1u32 << 24) as f32; // [0,1)
            let exact = {
                let s = linear_to_srgb(c);
                (s * 255.0).round().clamp(0.0, 255.0) as u8
            };
            let d = (exact as i32 - encode_srgb(c) as i32).abs();
            if d > 0 {
                differing += 1;
            }
            worst = worst.max(d);
        }
        // 打印实测分布（cargo test 时加 --nocapture 可见）：给"画质有没有变"留一份证据
        println!(
            "[tonemap LUT] 取样 {N} 个通道：最大差异 {worst} 个色阶；有差异的通道 {differing} 个（{:.3}%）",
            differing as f64 / N as f64 * 100.0
        );
        assert!(
            worst <= 1,
            "LUT 与精确实现的最大差异不得超过 1 LSB，实际 {worst}"
        );
        assert!(
            differing * 1000 / N <= 10,
            "差异通道比例应 ≤1%，实际 {differing}/{N}"
        );
    }

    /// 越界值与 NaN 的编码行为与旧实现一致：旧实现靠 `as u8` 饱和转换，负数与 NaN 都落到 0
    #[test]
    fn lut_encoding_clamps_and_handles_nan() {
        assert_eq!(encode_srgb(-1.0), 0);
        assert_eq!(encode_srgb(0.0), 0);
        assert_eq!(encode_srgb(1.0), 255);
        assert_eq!(encode_srgb(5.0), 255);
        assert_eq!(encode_srgb(f32::NAN), 0, "NaN 必须落 0 且不得 panic");
    }

    /// 域内像素必须是严格 no-op（DirectSrgb 路径的逐像素开销仅一次比较）
    #[test]
    fn test_desaturate_in_gamut_noop() {
        assert_eq!(desaturate(0.18, 0.5, 0.9), (0.18, 0.5, 0.9));
        assert_eq!(desaturate(0.0, 0.0, 0.0), (0.0, 0.0, 0.0));
        assert_eq!(desaturate(1.0, 1.0, 1.0), (1.0, 1.0, 1.0));
    }

    // PQ 逆运算测试与 R10G10B10A2 解码测试随注入路径一并移至 injection 分支
}
