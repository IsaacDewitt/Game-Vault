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
//! 2. **SDR 内容零损失**：亮度 ≤ SDR 白的部分恒等直通——截图与视频不同，
//!    视频可把 SDR 白压到中灰（BT.2408 reference white），截图观感必须
//!    "原样"，只有真正的 HDR 高光需要处理。原裸 Reinhard L/(1+L) 会把
//!    SDR 白压到 0.5、中灰压暗一成，正是被推翻的旧实现。
//! 3. **高光走 knee 肩部压缩**：libplacebo 默认的 spline/BT.2390 均为
//!    "低区恒等 + 肩部 roll-off"结构；本模块实现其线性域简化版
//!    （smoothstep 肩部），并把肩部最亮点压至 1-0.35 而非硬裁剪。
//!    OBS/hdrfix 用的 extended Reinhard 属同类单 knee 曲线，但会把
//!    SDR 白也压暗，不适合截图语义，故取其结构弃其曲线。
//! 4. **越界色保亮度收敛**：宽色域（Rec.2020/P3）内容在 scRGB 中会出现
//!    负分量；超白高光会出现 >1 分量。按 libplacebo 的 desaturate 模式
//!    （保 CIE 亮度向灰轴收敛）单步精确求解收缩系数。
//!
//! 注入方案用的 PQ 解码 / R10G10B10A2 解码 / 线性帧入口同样存于 injection 分支
//! （且其 PQ 值域与白电平语义不一致，恢复前需先修正），master 不保留。

use half::f16;

/// 肩部最大下压深度：最强高光被压到 1-0.35=0.65 线性亮度（sRGB 约 211/255）。
/// 取值权衡：过小则高光无层次（近似硬裁剪），过大则高光发灰。
const SHOULDER_DEPTH: f32 = 0.35;

/// 三级判定结果，决定走哪条编码路径
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToneMapPath {
    /// 纯 SDR 内容且白电平为标准 1.0，零变换直接 sRGB 编码，与 Steam 完全一致
    DirectSrgb,
    /// SDR 内容整体随系统 SDR 白电平线性缩放，除回白电平系数即无损还原
    DivideWhiteLevel,
    /// 存在 HDR 高光（> SDR 白），恒等段 + 肩部压缩
    Reinhard,
}

/// 一帧 HDR 数据的亮度统计（第一遍扫描得到）
#[derive(Debug, Clone, Copy)]
struct LuminanceStats {
    /// 最大线性亮度
    max: f32,
}

impl LuminanceStats {
    fn from_frame(rgba: &[f32]) -> Self {
        let mut max = 0.0f32;
        for px in rgba.chunks_exact(4) {
            let l = luminance(px[0], px[1], px[2]);
            if l > max {
                max = l;
            }
        }
        Self { max }
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
    let stats = LuminanceStats::from_frame(&linear);

    let ws = if white_level_scale > 0.0 { white_level_scale } else { 1.0 };

    // 三级判定：ws 是帧内 SDR 白的真实线性值
    let path = if stats.max <= ws {
        if (ws - 1.0).abs() < 1e-6 {
            ToneMapPath::DirectSrgb
        } else {
            ToneMapPath::DivideWhiteLevel
        }
    } else {
        ToneMapPath::Reinhard
    };

    // 肩部白点：帧内最亮像素（归一化到 SDR 白=1.0 的域）。
    // 取 max 而非 p999：截图语义下高光占比小（太阳/灯光常 <1% 像素），
    // p999 抓不到会把肩部预算浪费在 [1,1] 区间、高光全压到肩底发灰；
    // 取 max 则肩部覆盖全部高光范围，个别超界坏点由 clamp 兜底。
    let shoulder_w = (stats.max / ws).max(1.0 + 1e-6);

    let mut out: Vec<u8> = Vec::with_capacity(parsed_pixels * 4);

    for px in linear.chunks_exact(4) {
        let (mut r, mut g, mut b, a) = (px[0], px[1], px[2], px[3]);

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
            ToneMapPath::Reinhard => {
                // 归一化到 SDR 白 = 1.0
                r /= ws;
                g /= ws;
                b /= ws;
                // 恒等段 + 肩部：亮度 ≤ 1（SDR 内容）完全不动；
                // (1, W] 的 HDR 高光经 smoothstep 肩部平滑压入 (1-d, 1]。
                let n = luminance(r, g, b);
                if n > 1.0 {
                    let t = ((n - 1.0) / (shoulder_w - 1.0)).clamp(0.0, 1.0);
                    let h = 3.0 * t * t - 2.0 * t * t * t;
                    let scale = (1.0 - SHOULDER_DEPTH * h) / n;
                    r *= scale;
                    g *= scale;
                    b *= scale;
                }
            }
        }

        // 公共出口：宽色域负分量 / 超界分量的保亮度收敛
        // （DirectSrgb/DivideWhiteLevel 路径的正常像素此处为 no-op）
        let (dr, dg, db) = desaturate(r, g, b);
        r = dr;
        g = dg;
        b = db;

        // sRGB 编码 + 8bit 量化
        let enc = |c: f32| -> u8 {
            let s = linear_to_srgb(c.clamp(0.0, 1.0));
            (s * 255.0).round().clamp(0.0, 255.0) as u8
        };
        out.push(enc(r));
        out.push(enc(g));
        out.push(enc(b));
        out.push((a.clamp(0.0, 1.0) * 255.0).round().clamp(0.0, 255.0) as u8);
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

    /// 核心恒等性：混合帧中亮度 ≤ SDR 白的像素，输出必须与纯 SDR 帧逐字节一致
    /// （旧裸 Reinhard 会把中灰压暗一成、SDR 白压半，此测试防回归）
    #[test]
    fn test_reinhard_keeps_sdr_pixels_identical() {
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
        assert_eq!(path_mixed, ToneMapPath::Reinhard);
        // 前 3 像素（0.18/0.5/1.0）逐字节相等
        for i in 0..3 {
            assert_eq!(
                &out_mixed[i * 4..i * 4 + 4],
                &out_sdr[i * 4..i * 4 + 4],
                "像素 {i} 在 HDR 混合帧中被意外改动"
            );
        }
        // 高光像素被压缩但不裁剪成死白
        assert!(out_mixed[12] < 255, "高光不应顶格死白");
        assert!(out_mixed[12] > 150, "高光不应被压到过暗，实际 {}", out_mixed[12]);
    }

    /// 肩部压缩落点：单强高光（10.0 线性 = 800nits）应压到肩底
    /// 1-0.35=0.65 线性 ≈ sRGB 211，而非裁剪 255 或旧裸 Reinhard 的 118
    #[test]
    fn test_reinhard_shoulder_compresses_peak() {
        let src = fp16_frame(&[[10.0, 10.0, 10.0, 1.0]]);
        let (out, path) = tonemap_rgba16f_to_srgb(&src, 1, 1, 1.0);
        assert_eq!(path, ToneMapPath::Reinhard);
        assert_eq!(out.len(), 4, "1x1 帧应输出 4 字节 RGBA");
        // 0.65 线性 → sRGB(0.65) ≈ 0.827 → 211；留容差防精度漂移
        assert!(
            (195..=225).contains(&out[0]),
            "肩底落点应在 195~225，实际 {}",
            out[0]
        );
    }

    /// 原生 HDR 极值（4000nits = 50.0 线性）：中灰恒等 + 高光有层次不 NaN
    #[test]
    fn test_high_peak_4000nits() {
        let src = fp16_frame(&[[0.18, 0.18, 0.18, 1.0], [50.0, 50.0, 50.0, 1.0]]);
        let sdr = fp16_frame(&[[0.18, 0.18, 0.18, 1.0], [1.0, 1.0, 1.0, 1.0]]);
        let (out_hdr, _) = tonemap_rgba16f_to_srgb(&src, 2, 1, 1.0);
        let (out_sdr, _) = tonemap_rgba16f_to_srgb(&sdr, 2, 1, 1.0);
        // 中灰在 4000nits 高光旁保持零损失
        assert_eq!(out_hdr[0], out_sdr[0], "中灰不应被极端高光拖动");
        // 高光压入肩部区间
        assert!((150..=254).contains(&out_hdr[4]), "高光落点异常: {}", out_hdr[4]);
    }

    /// 宽色域负分量（Rec.2020 高饱和色在 scRGB 中 R<0）：保亮度精确收敛
    #[test]
    fn test_desaturate_negative_wide_gamut() {
        let (r, g, b) = desaturate(-0.2, 0.1, 0.3);
        // 亮度 l=0.0507 不变；k=l/(l-(-0.2)) 使 R 恰好归零
        let l = luminance(r, g, b);
        assert!((l - 0.0507).abs() < 1e-3, "亮度应保持不变，实际 {l}");
        assert!(r >= 0.0 && r < 1e-4, "负分量应精确收敛到 0，实际 {r}");
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

    /// 域内像素必须是严格 no-op（DirectSrgb 路径的逐像素开销仅一次比较）
    #[test]
    fn test_desaturate_in_gamut_noop() {
        assert_eq!(desaturate(0.18, 0.5, 0.9), (0.18, 0.5, 0.9));
        assert_eq!(desaturate(0.0, 0.0, 0.0), (0.0, 0.0, 0.0));
        assert_eq!(desaturate(1.0, 1.0, 1.0), (1.0, 1.0, 1.0));
    }

    // PQ 逆运算测试与 R10G10B10A2 解码测试随注入路径一并移至 injection 分支
}
