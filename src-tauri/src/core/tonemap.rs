//! 色调映射：把 WGC 拿到的 HDR 原始帧压回 SDR sRGB。
//!
//! 这是本功能画质的胜负手。核心结论（源自调研）：
//! - windows-capture 的 `save_as_image` 对 `Rgba16F` 直接拒绝，本模块就是补上这一步。
//! - 拿到线性 HDR 后先做三级判定，只对真正需要压缩的高光动手，把偏差压缩到最小。
//! - Reinhard 走亮度域（而非逐通道），超 sRGB 高光去饱和（而非压暗）。
//!
//! 注入方案用的 PQ 解码 / R10G10B10A2 解码 / 线性帧入口同样存于 injection 分支
//! （且其 PQ 值域与三级判定的 SDR 白语义不一致，恢复前需先修正），master 不保留。

use half::f16;

/// 三级判定结果，决定走哪条编码路径
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToneMapPath {
    /// 纯 SDR 内容（max ≤ 1.0），零变换直接 sRGB 编码，与 Steam 完全一致
    DirectSrgb,
    /// SDR 内容被系统白电平线性提亮，除回白电平系数即无损还原
    DivideWhiteLevel,
    /// 真正存在 HDR 高光，需 Reinhard 压缩
    Reinhard,
}

/// 一帧 HDR 数据的亮度统计（第一遍扫描得到）
#[derive(Debug, Clone, Copy)]
struct LuminanceStats {
    /// 最大线性亮度
    max: f32,
    /// 99.9 百分位亮度（抗噪，避免个别坏点干扰自动曝光）
    p999: f32,
}

impl LuminanceStats {
    fn from_frame(rgba: &[f32]) -> Self {
        let mut max = 0.0f32;
        let mut lumas: Vec<f32> = Vec::with_capacity(rgba.len() / 4);
        for px in rgba.chunks_exact(4) {
            let l = luminance(px[0], px[1], px[2]);
            if l > max {
                max = l;
            }
            lumas.push(l);
        }
        // 99.9 百分位
        let p999 = percentile(&mut lumas, 0.999);
        Self { max, p999 }
    }
}

/// 计算 Rec.709 亮度（线性域）
#[inline]
fn luminance(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// 百分位采样上限：4K 帧有 829 万个亮度值，全排序约 1.5~3s；
/// 采样 6.5 万个点后排序降至 ~5ms，对 p99.9 的精度影响可忽略。
const PERCENTILE_SAMPLE_CAP: usize = 65_536;

/// 近似百分位：超过采样上限时按等步长抽样后排序（注释曾承诺"改用采样近似"，
/// 但此前实现仍是全排序，此处兑现）
fn percentile(data: &mut [f32], q: f32) -> f32 {
    if data.is_empty() {
        return 0.0;
    }
    if data.len() <= PERCENTILE_SAMPLE_CAP {
        return percentile_sorted(data, q);
    }
    let step = data.len() / PERCENTILE_SAMPLE_CAP;
    let mut sampled: Vec<f32> = data.iter().copied().step_by(step).collect();
    percentile_sorted(&mut sampled, q)
}

fn percentile_sorted(data: &mut [f32], q: f32) -> f32 {
    data.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    data[((data.len() - 1) as f32 * q) as usize]
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

/// 去饱和：把超 sRGB 色域的高光向亮度轴收敛，保住高光形状与对比
/// 参考 hdrfix 的思路：压暗会让立体感塌掉，正确做法是降低饱和度、保住亮度
#[inline]
fn desaturate(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    // 若三通道都 ≤ 1.0，已落在色域内，无需处理
    if r <= 1.0 && g <= 1.0 && b <= 1.0 {
        return (r, g, b);
    }
    // 亮度不变，向灰轴（等亮度）收敛
    let l = luminance(r, g, b);
    // 对超界通道做一次向灰轴的线性插值，插值系数按最大越界幅度决定
    let max_over = (r.max(g).max(b) - 1.0).max(0.0);
    let t = (max_over / (max_over + 1.0)).clamp(0.0, 0.9); // 越界越多，去饱和越强
    let desat = |c: f32| l + (c - l) * (1.0 - t);
    (desat(r), desat(g), desat(b))
}

/// 单帧 tonemap 核心。
///
/// 参数：
/// - `src`: Rgba16F 无 padding 的原始字节（每像素 8 字节，R,G,B,A 各 2 字节 f16）
/// - `width` / `height`: 帧尺寸
/// - `white_level_scale`: SDR 白电平归一化系数（`SdrWhiteLevelInNits / 80`），
///   默认 1.0 表示 80 nits；用于抵消 DWM 对 SDR 内容的线性提亮。
/// - `exposure`: 手动曝光档位，默认 1.0。
///
/// 返回 sRGB RGBA8 字节（每像素 4 字节，无 padding），以及本次实际采用的编码路径。
pub fn tonemap_rgba16f_to_srgb(
    src: &[u8],
    width: u32,
    height: u32,
    white_level_scale: f32,
    exposure: f32,
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

    // 第一遍统计亮度（不含曝光，用原始线性值判定）
    let stats = LuminanceStats::from_frame(&linear);

    let ws = if white_level_scale > 0.0 { white_level_scale } else { 1.0 };
    let expo = if exposure > 0.0 { exposure } else { 1.0 };

    // 三级判定
    let path = if stats.max <= 1.0 {
        ToneMapPath::DirectSrgb
    } else if stats.max / ws <= 1.0 {
        ToneMapPath::DivideWhiteLevel
    } else {
        ToneMapPath::Reinhard
    };

    // 自动曝光：按 99.9 百分位反推，避免个别极端高光把整张图拖暗
    // （仅 Reinhard 路径启用；DirectSrgb / DivideWhiteLevel 属无损还原，不做曝光）
    let auto_exposure = if path == ToneMapPath::Reinhard && stats.p999 > 0.0 {
        // 目标是让 99.9 百分位落在中灰附近，取温和系数
        (1.0 / stats.p999.max(1.0)).clamp(0.5, 1.5)
    } else {
        1.0
    };

    let mut out: Vec<u8> = Vec::with_capacity(parsed_pixels * 4);

    for px in linear.chunks_exact(4) {
        let (mut r, mut g, mut b, a) = (px[0], px[1], px[2], px[3]);

        match path {
            ToneMapPath::DirectSrgb => {
                // 零变换：值域未越界，直接编码
            }
            ToneMapPath::DivideWhiteLevel => {
                // 线性提亮可无损还原：除回白电平系数
                r /= ws;
                g /= ws;
                b /= ws;
            }
            ToneMapPath::Reinhard => {
                // 先归一化白电平，再应用自动曝光
                r = r / ws * auto_exposure * expo;
                g = g / ws * auto_exposure * expo;
                b = b / ws * auto_exposure * expo;
                // 亮度域 Reinhard：L' = L / (1 + L)，保持色相不变
                let l = luminance(r, g, b);
                let l_tm = l / (1.0 + l);
                let scale = if l > 1e-6 { l_tm / l } else { 1.0 };
                r *= scale;
                g *= scale;
                b *= scale;
                // 超 sRGB 高光去饱和
                let (dr, dg, db) = desaturate(r, g, b);
                r = dr;
                g = dg;
                b = db;
            }
        }

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

    /// 纯 SDR 内容（线性 ≤ 1.0）应走 DirectSrgb 且零变换
    #[test]
    fn test_direct_srgb_zero_transform() {
        // 2x1 像素：中灰 (0.5, 0.5, 0.5) 和 纯白 (1.0, 1.0, 1.0)
        let mut src = Vec::new();
        for &v in &[0.5f32, 0.5, 0.5, 1.0, 1.0, 1.0, 1.0, 1.0] {
            src.extend_from_slice(&f16::from_f32(v).to_bits().to_le_bytes());
        }
        let (out, path) = tonemap_rgba16f_to_srgb(&src, 2, 1, 1.0, 1.0);
        assert_eq!(path, ToneMapPath::DirectSrgb);
        // 中灰 0.5 → sRGB ≈ 0.7354 → 188
        assert_eq!(out[0], 188);
        // 纯白 → 255
        assert_eq!(out[4], 255);
    }

    /// 越界高光应走 Reinhard，且不产生 NaN / 越界
    #[test]
    fn test_reinhard_no_nan() {
        // 1x1 像素，亮度 10.0（远超 1.0）
        let mut src = Vec::new();
        for &v in &[10.0f32, 10.0, 10.0, 1.0] {
            src.extend_from_slice(&f16::from_f32(v).to_bits().to_le_bytes());
        }
        let (out, path) = tonemap_rgba16f_to_srgb(&src, 1, 1, 1.0, 1.0);
        assert_eq!(path, ToneMapPath::Reinhard);
        // 高光被压缩到接近白但未过曝；输出为 u8，NaN 已在 f32 阶段被 clamp 处理
        assert!(out[0] < 255 && out[0] > 200, "压缩后高光应在 200~254 之间，实际 {}", out[0]);
        assert_eq!(out.len(), 4, "1x1 帧应输出 4 字节 RGBA");
    }

    // PQ 逆运算测试与 R10G10B10A2 解码测试随注入路径一并移至 injection 分支
}
