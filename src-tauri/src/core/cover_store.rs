//! 封面存储：`covers` 索引表 + `covers` 目录的权威读写层（2026-09-10 立）
//!
//! ## 目录布局（单一根目录，按用途分桶，不散落）
//! ```text
//! %APPDATA%/GameVault/covers/
//!   <owner_id>.<ext>            主图（游戏/手账共用；ext 视源格式而定）
//!   thumb/<owner_id>.<ext>      缩略图（长边 ≤ `THUMB_MAX_EDGE`，卡片网格/统计排行使用）
//!   archive/<owner_id>.<ext>    已移除条目的留档主图（删除不再删图）
//!   archive/thumb/<owner_id>.<ext>
//! ```
//!
//! ## 三条不变式（改这个模块前先读）
//! 1. **文件只在 covers 根目录内**；索引 `rel_path` 一律相对该根目录（换盘/换机不失效）。
//! 2. **同一 sha256 可被多个主体引用**（手账从游戏导入即共享物理文件）；
//!    任何删除动作必须先查引用计数，归零才动文件——根治"删游戏顺手毁了手账封面"。
//! 3. **落盘一律 写 .tmp → 校验 → 原子替换**，绝不留半张图；
//!    索引写失败也不删源文件（宁可留孤儿，不可丢图）。
//!
//! ## 编码策略（为什么不直接全转 WebP）
//! `image` 0.25 的 WebP 编码器**只有无损模式**（VP8L），对照片类封面写出来往往
//! 比原 JPEG 还大；有损 WebP 需引入 libwebp（C 依赖）。故：
//! - 主图：源是**不透明 PNG** → 转 JPEG q92（省体积且无视觉损失）；其余原样保留（已压过）。
//! - 缩略图：不透明 → JPEG q82；含真透明 → 无损 WebP（保留 alpha，512px 下体积可控）。
//!
//! ## 缩略图规格 = 渲染预算（改 `THUMB_MAX_EDGE` 前必读，2026-09-27）
//! 缩略图**只服务卡片网格/最近条带/时长排行**这类小尺寸场景，而它的长边预算必须同时
//! 盖住三件事，否则界面就是"糊"的：
//! 1. **卡片宽度随窗口变**：`.game-grid` 用 `auto-fill + minmax(180px, 220px)`，
//!    卡片宽 180~220 CSS px（高 = 宽×4/3，即 240~293 px）；
//! 2. **系统缩放（DPI）**：125%/150%/200% 时同样的卡片要 1.25~2 倍的物理像素；
//! 3. **源图长宽比**：缩略图按**长边**封顶，卡片却是固定 3:4 + `object-fit: cover`——
//!    16:9 的源封顶后只有 512×288，短边若太小，卡片就得放大补足（旧 256 规格下横图
//!    要放大 2.0 倍，肉眼就是"糊"）。抓来的封面恒为 2:3，本地手选的图长宽比任意，
//!    所以"只有自己上传的封面才糊"。
//! 结论：256 只够 100% 缩放下的小卡片；512 覆盖到 150% 缩放与任意长宽比。
//! **改动此常量后必须让老缩略图重生**（见 [`upgrade_thumbs`]），否则老图不会自动刷新。

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::core::Database;
use crate::models::cover::*;
use crate::utils::path;

/// 缩略图长边像素上限（渲染预算，见模块头注释；改动后必须走 [`upgrade_thumbs`]）
pub const THUMB_MAX_EDGE: u32 = 512;
/// 不透明主图转 JPEG 的质量（仅 PNG→JPEG 时使用）
const MAIN_JPEG_QUALITY: u8 = 92;
/// 缩略图 JPEG 质量
const THUMB_JPEG_QUALITY: u8 = 82;

/// 迁移结果（供日志与前端提示）
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct CoverMigrateReport {
    pub indexed: u32,
    pub relinked: u32,
    pub missing_file: u32,
    pub failed: u32,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// rel_path（`thumb/x.webp`）→ 绝对路径
pub fn abs_path(rel_path: &str) -> PathBuf {
    let mut p = path::get_covers_dir();
    for seg in rel_path.split('/') {
        p.push(seg);
    }
    p
}

/// 绝对路径 → rel_path（仅当位于 covers 根目录内；不在则返回 None）
fn rel_path_of(abs: &Path) -> Option<String> {
    let root = path::get_covers_dir();
    let abs_norm = abs.canonicalize().unwrap_or_else(|_| abs.to_path_buf());
    let root_norm = root.canonicalize().unwrap_or(root);
    let rel = abs_norm.strip_prefix(&root_norm).ok()?;
    Some(
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// 该路径是否落在 covers 目录内（决定"能否删源文件"）
fn is_inside_covers(abs: &Path) -> bool {
    rel_path_of(abs).is_some()
}

/// 写文件：先写同目录唯一命名的 `.tmp`，校验大小后**直接 rename 替换**正式文件。
///
/// 两处关键语义（2026-10-05 修正，与 `cover_fetcher::unique_temp_path` 同口径）：
/// - **不先删旧文件**：`std::fs::rename` 在 Windows 走 `MoveFileEx(MOVEFILE_REPLACE_EXISTING)`，
///   本身就是替换语义。原实现「先 remove_file(target) 再 rename」不但多余，还在
///   「删旧成功、rename 失败」（杀软锁定 / 权限抖动 / 文件被占用）的窗口里让封面**凭空消失**，
///   而 DB 里的索引行仍指向它 —— 卡片永久破图且无法自愈。
/// - **临时名必须唯一**：固定名 `.tmp` 会让并发写同一目标（连点上传 / 后台抓封面与手动上传撞车）
///   互相删对方的临时文件；唯一名带 uuid，各写各的。
fn write_atomic(target: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = super::cover_fetcher::unique_temp_path(target);
    std::fs::write(&tmp, bytes).with_context(|| format!("写入临时文件失败: {}", tmp.display()))?;
    // 校验：文件真的落盘且大小一致，避免半张图覆盖掉好图
    let written = std::fs::metadata(&tmp)?.len();
    if written != bytes.len() as u64 {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("封面写入校验失败（期望 {} 字节，实际 {}）", bytes.len(), written);
    }
    if let Err(e) = std::fs::rename(&tmp, target) {
        // rename 失败时旧文件原样保留（远好于"删了旧的又没换上新的"）；顺手清掉孤儿临时文件
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("替换封面文件失败: {}", target.display()));
    }
    Ok(())
}

/// 一组文件替换的可回滚句柄。
///
/// `commit_prepared` / 缩略图升级都会「先换文件、再写 DB」。若第二步失败，
/// 旧文件必须能从备份恢复，否则磁盘是新字节、索引却仍记旧 sha256。备份用
/// 唯一临时名，且只在 DB 全部写成功后才删除。
struct FileSwap {
    backups: Vec<(PathBuf, PathBuf)>,
}

impl FileSwap {
    fn apply(staged: &[(PathBuf, PathBuf)]) -> Result<Self> {
        let mut backups = Vec::new();
        for (tmp, dst) in staged {
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let backup = if dst.exists() {
                let backup = super::cover_fetcher::unique_temp_path(dst);
                std::fs::copy(dst, &backup)
                    .with_context(|| format!("备份旧封面失败: {}", dst.display()))?;
                Some(backup)
            } else {
                None
            };

            if let Err(e) = std::fs::rename(tmp, dst) {
                // 失败的本次 tmp 与后续未消费的 tmp 都不能留成孤儿。
                for (t, _) in staged {
                    let _ = std::fs::remove_file(t);
                }
                if let Some(b) = &backup {
                    let _ = std::fs::remove_file(b);
                }
                Self::restore(&backups);
                return Err(e).with_context(|| format!("替换封面文件失败: {}", dst.display()));
            }
            if let Some(b) = backup {
                backups.push((dst.clone(), b));
            }
        }
        Ok(Self { backups })
    }

    fn restore(backups: &[(PathBuf, PathBuf)]) {
        for (dst, backup) in backups {
            match std::fs::read(backup) {
                Ok(bytes) => {
                    if let Err(e) = write_atomic(dst, &bytes) {
                        tracing::error!("封面回滚失败 {}：{}（备份仍在 {}）", dst.display(), e, backup.display());
                        continue;
                    }
                    let _ = std::fs::remove_file(backup);
                }
                Err(e) => tracing::error!("读取封面备份失败 {}：{}", backup.display(), e),
            }
        }
    }

    fn rollback(self) {
        Self::restore(&self.backups);
    }

    fn commit(self) {
        for (_, backup) in &self.backups {
            let _ = std::fs::remove_file(backup);
        }
    }
}

/// 移动文件（跨目录同盘直接 rename；失败退化为复制+删源）
fn move_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if to.exists() {
        let _ = std::fs::remove_file(to);
    }
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to)?;
            if std::fs::metadata(to)?.len() == std::fs::metadata(from)?.len() {
                let _ = std::fs::remove_file(from);
            }
            Ok(())
        }
    }
}

/// 换扩展名（`thumb/x.jpg` → `thumb/x.webp`）：只动最后一段的扩展名，目录层级不动
fn swap_ext(rel: &str, ext: &str) -> String {
    match rel.rfind('.') {
        Some(i) if !rel[i..].contains('/') => format!("{}.{}", &rel[..i], ext),
        _ => format!("{}.{}", rel, ext),
    }
}

/// 该文件是否还有别人（索引里任意行）在用
fn file_still_referenced(db: &Database, sha256: &str) -> Result<bool> {
    if sha256.is_empty() {
        return Ok(false);
    }
    Ok(db.cover_sha_refcount(sha256)? > 0)
}

/// 清理被摘掉索引的文件：无人引用才真删（含 archive 内的留档）
fn gc_rows(db: &Database, rows: &[CoverIndexRow], protect: &[String]) -> Result<u32> {
    let mut removed = 0u32;
    for row in rows {
        // 新写的文件不能删（重设封面时新旧同名会被列进 removed）
        if protect.iter().any(|p| p == &row.rel_path) {
            continue;
        }
        if file_still_referenced(db, &row.sha256)? {
            continue;
        }
        let abs = abs_path(&row.rel_path);
        if abs.exists() && is_inside_covers(&abs) {
            if let Err(e) = std::fs::remove_file(&abs) {
                tracing::warn!("删除封面文件失败 {}: {}", abs.display(), e);
            } else {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

// ==================== 编解码 ====================

/// 图片是否含真透明像素（类型带 alpha 且确实存在非 255 的 alpha）
fn has_real_alpha(img: &image::DynamicImage) -> bool {
    if !img.color().has_alpha() {
        return false;
    }
    // 零拷贝快路径：解码/缩放的产物通常本就是 Rgba8，`as_rgba8()` 命中时直接借用原缓冲扫描。
    // 原实现无条件 `to_rgba8()`，每次都要整帧分配 + 逐像素转换（4K 封面 ≈ 48MB 临时内存、
    // 1200 万次像素搬运），而这只为回答"有没有一个 alpha < 255"。判定结果与旧实现逐像素等价。
    if let Some(rgba) = img.as_rgba8() {
        return rgba.pixels().any(|p| p.0[3] < 255);
    }
    img.to_rgba8().pixels().any(|p| p.0[3] < 255)
}

fn encode_jpeg(img: &image::DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let rgb = img.to_rgb8();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    encoder.encode_image(&rgb).context("JPEG 编码失败")?;
    Ok(out)
}

fn encode_lossless_webp(img: &image::DynamicImage) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut out);
    encoder
        .encode(
            img.as_bytes(),
            img.width(),
            img.height(),
            img.color().into(),
        )
        .context("WebP 编码失败")?;
    Ok(out)
}

/// 生成缩略图：先等比缩到长边 ≤ `THUMB_MAX_EDGE`（当前 512），再按透明度选编码。
///
/// 返回 `(字节, 宽, 高, 扩展名)`——三者**都由真正落盘的那张缩略图导出**，一次算清：
/// - 早前两个调用方拿到字节后各自又 `img.thumbnail()` 重算一遍，**只为读宽高**，
///   白费一次原图重采样（缩略图必须从原图重采样，DynamicImage 不缓存中间结果）；
/// - 扩展名判定也曾被逐字复制在 `prepare_bytes` 与 `register_in_place` 两处，
///   一旦编码策略变更而漏改其一，索引里的 rel_path 就会与实际文件后缀不符，
///   封面直接"凭空消失"。
///
/// 扩展名仍按**编码结果的容器魔数**判定（而非复述 has_real_alpha），与旧行为逐字节一致。
fn build_thumb(img: &image::DynamicImage) -> Result<(Vec<u8>, u32, u32, &'static str)> {
    // 预算框长边**不超过源图长边**：小图（长边 < THUMB_MAX_EDGE）一律不放大 ——
    // 放大只会白占体积，还让卡片拿到一张"糊得更均匀"的假高清。
    let box_edge = THUMB_MAX_EDGE.min(img.width().max(img.height()));
    let thumb = img.thumbnail(box_edge, box_edge);
    let (width, height) = (thumb.width(), thumb.height());
    let bytes = if has_real_alpha(&thumb) {
        encode_lossless_webp(&thumb)?
    } else {
        encode_jpeg(&thumb, THUMB_JPEG_QUALITY)?
    };
    // WebP 是 RIFF 容器（魔数 RIFF），JPEG 为 FFD8，二者不重叠
    let ext = if bytes.starts_with(b"RIFF") { "webp" } else { "jpg" };
    Ok((bytes, width, height, ext))
}

/// 主图落盘字节：不透明 PNG 转 JPEG 省体积，其余原样保留
fn build_main(img: &image::DynamicImage, source: &[u8], source_format: image::ImageFormat) -> Result<(Vec<u8>, &'static str)> {
    let is_png = matches!(source_format, image::ImageFormat::Png);
    if is_png && !has_real_alpha(img) {
        let bytes = encode_jpeg(img, MAIN_JPEG_QUALITY)?;
        // 转码后反而更大就别换了（个别小图会这样）
        if bytes.len() < source.len() {
            return Ok((bytes, "jpg"));
        }
    }
    let ext = match source_format {
        image::ImageFormat::Png => "png",
        image::ImageFormat::Jpeg => "jpg",
        image::ImageFormat::WebP => "webp",
        image::ImageFormat::Gif => "gif",
        image::ImageFormat::Bmp => "bmp",
        _ => "img",
    };
    Ok((source.to_vec(), ext))
}

// ==================== 对外操作 ====================

/// 查询某主体可用的封面（绝对路径，文件不存在则视为无）
///
/// **仅测试使用**（2026-10-30）：原先唯一的命令层调用方是手账「刷新信息」里的
/// "新封面是否已在索引里"判重，那段随「不再自动拉封面」一并删掉了。
/// 留着是因为它是最直接的索引读取口径，几乎每个封面测试都要用它断言；
/// 真要恢复自动抓取，把 `cfg(test)` 去掉即可。
#[cfg(test)]
pub fn resolve(db: &Database, owner_kind: &str, owner_id: &str) -> Result<CoverSet> {
    let mut set = CoverSet::default();
    for row in db.cover_rows(owner_kind, owner_id)? {
        let abs = abs_path(&row.rel_path);
        if !abs.exists() {
            continue;
        }
        let s = abs.to_string_lossy().to_string();
        match row.kind.as_str() {
            KIND_MAIN => {
                set.main = Some(s);
                // 破缓存时间戳跟随**主图**：前端详情页/卡片用的都是它
                set.updated_at = Some(row.updated_at.clone());
            }
            KIND_THUMB => set.thumb = Some(s),
            _ => {}
        }
    }
    // 兜底：索引缺失但文件在（迁移未跑完/外部放入）——不返回，交给上层用 cover_local 兜
    Ok(set)
}

/// 入库一张图片：转码 → 生成缩略图 → 原子落盘 → 写索引 → 清理旧文件
/// 预处理产物：已编码落盘的主图/缩略图 + 待写入的索引行（**不包含任何数据库状态**）。
///
/// 拆出这一层是为了让命令层能「无锁做图像重活、短锁写索引」——
/// 大图的解码/编码动辄数百毫秒，若一直握着全局 DB 锁，其它命令全部排队。
pub struct PreparedCover {
    rows: Vec<CoverIndexRow>,
    /// 无锁阶段写好的临时文件 `(临时路径, 正式路径)`。
    ///
    /// 【2026-11-08】旧版 prepare 直接把主图/缩略图写到固定的
    /// `{owner_id}.{ext}`，两次并发换同一条目的封面时，后一次 rename
    /// 会盖掉前一次的文件，而 DB 索引可能还留着前一次的 sha256 ——
    /// 引用计数据此判断会误删仍被手账引用的图。现在 prepare 只写
    /// 唯一临时名，由 `commit_prepared` 在**全局 DB 锁内**串行替换。
    staged: Vec<(PathBuf, PathBuf)>,
}

impl Drop for PreparedCover {
    fn drop(&mut self) {
        // 预处理后被丢弃（条目已删 / 下载失败 / 提前返回）时清掉临时文件，
        // 不能让未提交的文件变成永远没人认领的孤儿。
        for (tmp, _) in &self.staged {
            let _ = std::fs::remove_file(tmp);
        }
    }
}

/// 无锁阶段：把编码结果写到同目录的唯一临时名，等 commit 时再原子替换正式文件。
fn write_staged(target: &Path, bytes: &[u8]) -> Result<(PathBuf, PathBuf)> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = super::cover_fetcher::unique_temp_path(target);
    std::fs::write(&tmp, bytes).with_context(|| format!("写入临时文件失败: {}", tmp.display()))?;
    let written = std::fs::metadata(&tmp)?.len();
    if written != bytes.len() as u64 {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("封面写入校验失败（期望 {} 字节，实际 {}）", bytes.len(), written);
    }
    Ok((tmp, target.to_path_buf()))
}

/// 预处理（**不接触数据库**）：识别格式 → 解码 → 编码主图/缩略图 → 写暂存文件 → 生成索引行。
///
/// 副作用只有两个唯一命名的暂存文件；正式文件与索引都尚未动，由
/// [`commit_prepared`] 在 DB 锁内一并收编（未提交的暂存文件随 Drop 清理）。
pub fn prepare_bytes(owner_kind: &str, owner_id: &str, bytes: &[u8]) -> Result<PreparedCover> {
    path::ensure_cover_dirs()?;

    // 【2026-11-08】解码前显式收紧尺寸/内存上限。image 0.25 默认只有
    // 512MiB 的 non-strict max_alloc、宽高无限制，本地手选一张 decompression
    // bomb（体积不大、像素极大）仍可能把进程撑爆；封面根本用不到超大图。
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .context("无法识别图片格式")?;
    let format = reader.format().context("无法识别图片格式")?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(128 * 1024 * 1024);
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    reader.limits(limits);
    let img = reader.decode().context("无法解码图片")?;
    let (width, height) = (img.width(), img.height());

    let (main_bytes, main_ext) = build_main(&img, bytes, format)?;
    let (thumb_bytes, thumb_width, thumb_height, thumb_ext) = build_thumb(&img)?;

    let main_rel = format!("{}.{}", owner_id, main_ext);
    let thumb_rel = format!("thumb/{}.{}", owner_id, thumb_ext);

    // 先建 PreparedCover 再逐个写暂存文件：任何一步 `?` 早退都会触发 Drop，
    // 把已写好的临时文件清干净，不给磁盘留孤儿。
    let mut prepared = PreparedCover { rows: Vec::new(), staged: Vec::new() };
    prepared.staged.push(write_staged(&abs_path(&main_rel), &main_bytes)?);
    prepared.staged.push(write_staged(&abs_path(&thumb_rel), &thumb_bytes)?);

    let now = now_rfc3339();
    prepared.rows = vec![
            CoverIndexRow {
                owner_kind: owner_kind.to_string(),
                owner_id: owner_id.to_string(),
                kind: KIND_MAIN.to_string(),
                rel_path: main_rel,
                sha256: sha256_hex(&main_bytes),
                width,
                height,
                bytes: main_bytes.len() as u64,
                state: STATE_ACTIVE.to_string(),
                updated_at: now.clone(),
            },
            CoverIndexRow {
                owner_kind: owner_kind.to_string(),
                owner_id: owner_id.to_string(),
                kind: KIND_THUMB.to_string(),
                rel_path: thumb_rel,
                sha256: sha256_hex(&thumb_bytes),
                width: thumb_width,
                height: thumb_height,
                bytes: thumb_bytes.len() as u64,
                state: STATE_ACTIVE.to_string(),
                updated_at: now,
            },
    ];
    Ok(prepared)
}

/// 读文件 + 预处理（**不接触数据库**，供命令层在无锁段调用）
pub fn prepare_path(owner_kind: &str, owner_id: &str, src: &Path) -> Result<PreparedCover> {
    // 本地手选路径也必须过体积预检（下载链路早已有 COVER_MAX_FILE_SIZE 防线）：
    // 误选一个视频/镜像文件不该被整包读进内存再交给解码器。
    let meta = std::fs::metadata(src)
        .with_context(|| format!("读取图片信息失败: {}", src.display()))?;
    if meta.len() > crate::utils::constants::COVER_MAX_FILE_SIZE {
        anyhow::bail!(
            "图片文件过大（{} 字节，上限 {} 字节）",
            meta.len(),
            crate::utils::constants::COVER_MAX_FILE_SIZE
        );
    }
    let bytes = std::fs::read(src).with_context(|| format!("读取图片失败: {}", src.display()))?;
    prepare_bytes(owner_kind, owner_id, &bytes)
}

/// 提交预处理结果：带备份地替换正式文件 → 写索引 → 回收旧文件 → 同步封面字段。
/// 调用方须持有 DB 锁；除 `rename` 与数据库操作外只做"无引用的旧文件"清理，耗时短。
/// `rename` 放在锁内是刻意的：同一 owner 的并发提交因此天然串行；DB 失败时按备份回滚文件。
pub fn commit_prepared(
    db: &Database,
    owner_kind: &str,
    owner_id: &str,
    mut prepared: PreparedCover,
) -> Result<CoverSet> {
    // 【2026-11-08】在调用方持有的全局 DB 锁内，把暂存文件替换到正式路径。
    // 同一 owner 的并发换图因此按 commit 顺序串行收敛；旧文件先备份，
    // 若随后任何一步 DB 写入失败则整体回滚，避免「磁盘新字节 / 索引旧 sha」。
    // `mem::take` 保证中途 return 时不会出现没人清理的 staged 列表。
    let staged = std::mem::take(&mut prepared.staged);
    let swap = FileSwap::apply(&staged)?;

    let commit = (|| -> Result<CoverSet> {
        let protect: Vec<String> = prepared.rows.iter().map(|r| r.rel_path.clone()).collect();
        let removed = db.replace_cover_rows(owner_kind, owner_id, &prepared.rows)?;
        gc_rows(db, &removed, &protect)?;
        sync_owner_paths(db, owner_kind, owner_id)?;
        Ok(CoverSet {
            main: prepared
                .rows
                .first()
                .map(|r| abs_path(&r.rel_path).to_string_lossy().to_string()),
            thumb: prepared
                .rows
                .get(1)
                .map(|r| abs_path(&r.rel_path).to_string_lossy().to_string()),
            // 主图行的时间戳就是本次替换时刻（prepared.rows 里 main 恒为首行）
            updated_at: prepared.rows.first().map(|r| r.updated_at.clone()),
        })
    })();

    match commit {
        Ok(set) => {
            swap.commit();
            Ok(set)
        }
        Err(e) => {
            swap.rollback();
            Err(e)
        }
    }
}

/// 入库成功后清理源文件（`ingest_path` 的第三步；命令层拆锁时也复用它）。
/// 两重保护：① 源必须位于 covers 目录内；② 源路径不得仍被其他主体的索引行引用（共享文件绝不删）。
pub fn cleanup_ingest_source(
    db: &Database,
    src: &Path,
    set: &CoverSet,
    delete_src: bool,
) -> Result<()> {
    if !delete_src {
        return Ok(());
    }
    if let Some(rel) = rel_path_of(src) {
        let new_main_rel = set.main.as_deref().and_then(|p| rel_path_of(Path::new(p)));
        let replaced_in_place = new_main_rel.as_deref() == Some(rel.as_str());
        if !replaced_in_place && db.cover_rel_refcount(&rel)? == 0 {
            let _ = std::fs::remove_file(src);
        }
    }
    Ok(())
}

/// 入库一个本地文件（手动选择封面 / 抓取器落盘的中间文件 / 迁移存量主图）
/// `delete_src`：入库成功后尝试删除源文件（保护规则见 `cleanup_ingest_source`）。
pub fn ingest_path(
    db: &Database,
    owner_kind: &str,
    owner_id: &str,
    src: &Path,
    delete_src: bool,
) -> Result<CoverSet> {
    let prepared = prepare_path(owner_kind, owner_id, src)?;
    let set = commit_prepared(db, owner_kind, owner_id, prepared)?;
    cleanup_ingest_source(db, src, &set, delete_src)?;
    Ok(set)
}

/// 迁移存量主图：
/// - 文件按本条目 id 命名（自己的文件）→ 走 ingest 转码替换（不透明 PNG 转 JPEG 省体积），旧的同名异扩展名文件由引用计数保护后清理；
/// - 文件属于别的条目（历史"裸抄路径"共享）→ 原地登记，不复制不删除，靠 sha256 引用计数保障后续删除安全。
#[cfg(test)]
pub fn migrate_existing_cover(
    db: &Database,
    owner_kind: &str,
    owner_id: &str,
    src: &Path,
) -> Result<bool> {
    let own_file = src
        .file_stem()
        .map(|s| s.to_string_lossy() == owner_id)
        .unwrap_or(false);
    if own_file {
        ingest_path(db, owner_kind, owner_id, src, true)?;
        Ok(true)
    } else {
        register_in_place(db, owner_kind, owner_id, src)
    }
}

/// 现有文件"原样登记"：不转码，只补缩略图并写索引（用于迁移存量封面）
#[cfg(test)]
pub fn register_in_place(
    db: &Database,
    owner_kind: &str,
    owner_id: &str,
    src: &Path,
) -> Result<bool> {
    let rel = match rel_path_of(src) {
        Some(r) => r,
        None => return Ok(false), // 不在 covers 目录内 → 交给 ingest_path 处理
    };
    let bytes = std::fs::read(src)?;
    let img = image::load_from_memory(&bytes).context("无法解码图片")?;
    let (thumb_bytes, thumb_width, thumb_height, thumb_ext) = build_thumb(&img)?;
    let thumb_rel = format!("thumb/{}.{}", owner_id, thumb_ext);
    // 缩略图已存在且非空则不覆盖（幂等）
    let thumb_abs = abs_path(&thumb_rel);
    if !thumb_abs.exists() {
        write_atomic(&thumb_abs, &thumb_bytes)?;
    }

    let now = now_rfc3339();
    let rows = vec![
        CoverIndexRow {
            owner_kind: owner_kind.to_string(),
            owner_id: owner_id.to_string(),
            kind: KIND_MAIN.to_string(),
            rel_path: rel.clone(),
            sha256: sha256_hex(&bytes),
            width: img.width(),
            height: img.height(),
            bytes: bytes.len() as u64,
            state: STATE_ACTIVE.to_string(),
            updated_at: now.clone(),
        },
        CoverIndexRow {
            owner_kind: owner_kind.to_string(),
            owner_id: owner_id.to_string(),
            kind: KIND_THUMB.to_string(),
            rel_path: thumb_rel.clone(),
            sha256: sha256_hex(&thumb_bytes),
            width: thumb_width,
            height: thumb_height,
            bytes: thumb_bytes.len() as u64,
            state: STATE_ACTIVE.to_string(),
            updated_at: now,
        },
    ];
    let protect = vec![rel, thumb_rel];
    let removed = db.replace_cover_rows(owner_kind, owner_id, &rows)?;
    gc_rows(db, &removed, &protect)?;
    Ok(true)
}

/// 共享引用：把 src 主体的封面行复制给 dst（不复制文件，同一物理文件按 sha256 共用）。
/// 用于手账从游戏导入——旧实现直接抄路径，导致删游戏时把共享文件一起删掉。
pub fn share_from(
    db: &Database,
    src_kind: &str,
    src_id: &str,
    dst_kind: &str,
    dst_id: &str,
    dst_state: &str,
) -> Result<bool> {
    let src_rows = db.cover_rows(src_kind, src_id)?;
    if src_rows.is_empty() {
        return Ok(false);
    }
    let now = now_rfc3339();
    let new_rows: Vec<CoverIndexRow> = src_rows
        .iter()
        .map(|r| CoverIndexRow {
            owner_kind: dst_kind.to_string(),
            owner_id: dst_id.to_string(),
            kind: r.kind.clone(),
            rel_path: r.rel_path.clone(),
            sha256: r.sha256.clone(),
            width: r.width,
            height: r.height,
            bytes: r.bytes,
            state: dst_state.to_string(),
            updated_at: now.clone(),
        })
        .collect();
    let protect: Vec<String> = new_rows.iter().map(|r| r.rel_path.clone()).collect();
    let removed = db.replace_cover_rows(dst_kind, dst_id, &new_rows)?;
    gc_rows(db, &removed, &protect)?;
    if dst_state == STATE_ACTIVE {
        sync_owner_paths(db, dst_kind, dst_id)?;
    }
    Ok(true)
}

/// 归档：删除条目但保留封面——文件挪进 archive/（被其他主体共享时只改状态、不动文件）
pub fn archive(db: &Database, owner_kind: &str, owner_id: &str) -> Result<()> {
    path::ensure_cover_dirs()?;
    let rows = db.cover_rows(owner_kind, owner_id)?;
    for row in rows.iter().filter(|r| r.state == STATE_ACTIVE) {
        let abs = abs_path(&row.rel_path);
        let shared = file_still_referenced(db, &row.sha256)? && db.cover_sha_refcount(&row.sha256)? > 1;
        if shared || !abs.exists() {
            db.set_cover_rel_path(owner_kind, owner_id, &row.kind, &row.rel_path, STATE_ARCHIVED)?;
            continue;
        }
        let file_name = abs
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("{}.bin", owner_id));
        let new_rel = if row.kind == KIND_THUMB {
            format!("archive/thumb/{}", file_name)
        } else {
            format!("archive/{}", file_name)
        };
        move_file(&abs, &abs_path(&new_rel))?;
        db.set_cover_rel_path(owner_kind, owner_id, &row.kind, &new_rel, STATE_ARCHIVED)?;
    }
    db.set_cover_state(owner_kind, owner_id, STATE_ARCHIVED)?;
    Ok(())
}

/// 彻底删除：摘索引 + 引用计数归零才删文件
pub fn purge(db: &Database, owner_kind: &str, owner_id: &str) -> Result<u32> {
    let removed = db.delete_cover_rows(owner_kind, owner_id)?;
    gc_rows(db, &removed, &[])?;
    sync_owner_paths(db, owner_kind, owner_id)?;
    Ok(removed.len() as u32)
}

/// 认领回归：把 archive 里的留档挪回正式目录并置回 active（重装后封面自动回归）
pub fn restore(db: &Database, owner_kind: &str, owner_id: &str) -> Result<bool> {
    let rows = db.cover_rows(owner_kind, owner_id)?;
    if rows.is_empty() {
        return Ok(false);
    }
    let mut restored = false;
    for row in rows.iter().filter(|r| r.state == STATE_ARCHIVED) {
        let file_name = Path::new(&row.rel_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("{}.bin", owner_id));
        let new_rel = if row.kind == KIND_THUMB {
            format!("thumb/{}", file_name)
        } else {
            file_name.clone()
        };
        let old_abs = abs_path(&row.rel_path);
        let new_abs = abs_path(&new_rel);
        if new_rel == row.rel_path {
            // 共享文件本就在正式目录（归档时没动它）——同样要确认文件真在，
            // 否则会把一条指向空路径的记录置为 active
            if abs_path(&new_rel).exists() {
                db.set_cover_rel_path(owner_kind, owner_id, &row.kind, &new_rel, STATE_ACTIVE)?;
                restored = true;
            } else {
                tracing::warn!("封面文件缺失，保持留档状态: {}", row.rel_path);
            }
            continue;
        }
        if !old_abs.exists() {
            continue; // 文件不在了，保持留档记录即可
        }
        if !new_abs.exists() {
            move_file(&old_abs, &new_abs)?;
        }
        db.set_cover_rel_path(owner_kind, owner_id, &row.kind, &new_rel, STATE_ACTIVE)?;
        restored = true;
    }
    // 不再无条件把该主体的所有行置为 active（2026-09-12 修正）：文件已缺失的行上面
    // 刻意 continue 保持留档，若在此统一置 active，活条目就会指向一个不存在的文件。
    // 每条成功恢复的行都已在上面各自置为 active，这里无需兜底。
    sync_owner_paths(db, owner_kind, owner_id)?;
    Ok(restored)
}

/// 把索引里的主图路径同步回 games/reviews 的封面字段（兼容旧读取路径）
///
/// 只认**文件确实存在**的主图行（2026-09-12 修正）：索引里可能存在文件已丢失的
/// 留档行，照抄其路径会让条目指向一张不存在的图。有主图行但文件都不在时，
/// 保持条目现有封面字段不动（宁可不改，也不写坏路径）。
pub fn sync_owner_paths(db: &Database, owner_kind: &str, owner_id: &str) -> Result<()> {
    let rows = db.cover_rows(owner_kind, owner_id)?;
    let main_rows: Vec<&_> = rows.iter().filter(|r| r.kind == KIND_MAIN).collect();
    let main = main_rows
        .iter()
        .map(|r| abs_path(&r.rel_path))
        .find(|p| p.exists())
        .map(|p| p.to_string_lossy().to_string());
    match (owner_kind, main) {
        (OWNER_GAME, Some(path)) => {
            db.update_game_cover(owner_id, &path)?;
        }
        (OWNER_GAME, None) if main_rows.is_empty() => {
            db.remove_game_cover(owner_id)?;
        }
        (OWNER_GAME, None) => {
            tracing::warn!("主图文件缺失，保留既有封面字段不动（游戏 {}）", owner_id);
        }
        (OWNER_REVIEW, Some(path)) => {
            db.update_review_cover(owner_id, &path)?;
        }
        (OWNER_REVIEW, None) if main_rows.is_empty() => {
            db.remove_review_cover(owner_id)?;
        }
        (OWNER_REVIEW, None) => {
            tracing::warn!("主图文件缺失，保留既有封面字段不动（手账 {}）", owner_id);
        }
        _ => {}
    }
    Ok(())
}

// ==================== 存量迁移 ====================

/// 缩略图规格标记：值就是当前规格（长边像素）。
/// **刻意存值而不是存 "1"** —— 将来再把 `THUMB_MAX_EDGE` 调大时，标记自然不等于新规格，
/// 升级会自己重跑，不需要记得去改这个键名或手工重置。
const THUMB_SPEC_FLAG: &str = "covers_thumb_spec";

/// 缩略图规格升级结果（供日志）
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ThumbUpgradeReport {
    pub rebuilt: u32,
    pub skipped: u32,
    pub failed: u32,
}

/// 把规格落后的缩略图按当前 `THUMB_MAX_EDGE` 重建（幂等，靠 `covers_thumb_spec` 记录已达标规格）。
///
/// **为什么必须有这一步**：缩略图是"渲染预算"的产物（见模块头注释），一旦把
/// `THUMB_MAX_EDGE` 调大，**老缩略图不会自己重生** —— 界面看着跟没改一样，白改。
/// 重建要点：
/// - 源一律取**主图**：缩略图必须从原图重采样，从旧缩略图再缩是二次损失；
/// - 按 `rel_path` **分组**：共享同一物理文件的多个主体一起改，引用计数不受影响；
/// - **同步推进主图行的 `updated_at`**：前端 asset URL 的破缓存参数取自主图行时间戳，
///   不推进的话 WebView2 照旧命中缓存里的小缩略图，用户会以为"改了没生效"。
///
/// 仅测试保留：生产入口是 `upgrade_thumbs_with_short_locks`，后者把图像重活移出 DB 锁。
#[cfg(test)]
pub fn upgrade_thumbs(db: &Database) -> Result<ThumbUpgradeReport> {
    let want = THUMB_MAX_EDGE.to_string();
    if db.get_setting(THUMB_SPEC_FLAG)?.as_deref() == Some(want.as_str()) {
        return Ok(ThumbUpgradeReport::default());
    }
    path::ensure_cover_dirs()?;

    // rel_path → 指向该文件的所有 thumb 行（共享封面会出现多条）
    let mut groups: std::collections::BTreeMap<String, Vec<CoverIndexRow>> = std::collections::BTreeMap::new();
    for row in db.all_cover_rows()? {
        if row.kind == KIND_THUMB {
            groups.entry(row.rel_path.clone()).or_default().push(row);
        }
    }

    let mut report = ThumbUpgradeReport::default();
    for (rel_path, group) in groups {
        // 只读文件头比尺寸，便宜；读不到（文件缺失/损坏）就跳过，交给 restore/migrate 那套逻辑
        match image::image_dimensions(abs_path(&rel_path)) {
            Ok((w, h)) if w.max(h) >= THUMB_MAX_EDGE => {
                report.skipped += 1;
                continue;
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("缩略图重建跳过（读不到尺寸）{}: {}", rel_path, e);
                report.skipped += 1;
                continue;
            }
        }

        let first = &group[0];
        let main_abs = db
            .cover_rows(&first.owner_kind, &first.owner_id)?
            .into_iter()
            .find(|r| r.kind == KIND_MAIN)
            .map(|r| abs_path(&r.rel_path))
            .filter(|p| p.exists());
        let Some(main_abs) = main_abs else {
            tracing::warn!("缩略图重建跳过（主图缺失）: {}", rel_path);
            report.skipped += 1;
            continue;
        };

        let img = match image::open(&main_abs) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!("缩略图重建失败（主图解码）{}: {}", main_abs.display(), e);
                report.failed += 1;
                continue;
            }
        };
        let (bytes, tw, th, thumb_ext) = match build_thumb(&img) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("缩略图重建失败（编码）{}: {}", rel_path, e);
                report.failed += 1;
                continue;
            }
        };

        // 编码结果换了容器（jpg ↔ webp，源图透明性变过）时不原地覆盖：按新扩展名落盘，索引跟着改
        let new_rel = swap_ext(&rel_path, thumb_ext);
        if let Err(e) = write_atomic(&abs_path(&new_rel), &bytes) {
            tracing::warn!("缩略图重建写盘失败 {}: {}", new_rel, e);
            report.failed += 1;
            continue;
        }
        let sha = sha256_hex(&bytes);
        let now = now_rfc3339();
        for row in &group {
            if new_rel != rel_path {
                db.set_cover_rel_path(&row.owner_kind, &row.owner_id, KIND_THUMB, &new_rel, &row.state)?;
            }
            db.update_cover_meta(
                &row.owner_kind,
                &row.owner_id,
                KIND_THUMB,
                &sha,
                tw,
                th,
                bytes.len() as u64,
                &now,
            )?;
            // 破缓存：前端 URL 的 t 取自主图行，主图行不动则界面还显示缓存里的小图
            db.touch_cover_row(&row.owner_kind, &row.owner_id, KIND_MAIN, &now)?;
        }
        if new_rel != rel_path && db.cover_rel_refcount(&rel_path)? == 0 {
            let _ = std::fs::remove_file(abs_path(&rel_path)); // 旧扩展名文件已无人引用
        }
        report.rebuilt += 1;
    }

    db.set_setting(THUMB_SPEC_FLAG, &want)?;
    Ok(report)
}

const MIGRATION_FLAG: &str = "covers_index_v1_done";
/// 强制重跑标记（置 1 → 下次启动重跑迁移）
const MIGRATION_FLAG_AGAIN: &str = "covers_index_v1_rerun";



/// 存量封面迁移（幂等，靠 settings 标记防重跑）：
/// 1. 把 games/reviews 现有封面登记进索引表；在 covers 目录内的文件原样保留，只补缩略图；
///    目录外的（历史手工路径）转码入库。
/// 2. 已移除条目（墓碑）没有封面时，按同名/英文名从手账条目**共享**一张过来
///    （这就是 Mafia: The Old Country 那类"游戏删了、手账里还留着图"的救图路径）。
///
/// 仅测试保留：生产入口是 `migrate_with_short_locks`，后者按条目拆锁且不阻塞首屏后的操作。
#[cfg(test)]
pub fn migrate(db: &Database) -> Result<CoverMigrateReport> {
    let done = db.get_setting(MIGRATION_FLAG)?.as_deref() == Some("1");
    let rerun = db.get_setting(MIGRATION_FLAG_AGAIN)?.as_deref() == Some("1");
    if done && !rerun {
        return Ok(CoverMigrateReport::default());
    }
    db.ensure_cover_storage()?;

    let mut report = CoverMigrateReport {
        bytes_before: dir_size(&path::get_covers_dir()),
        ..Default::default()
    };

    // ---- 第 1 步：现有游戏 / 手账封面入索引 ----
    let mut owners: Vec<(String, String, Option<String>)> = Vec::new();
    for game in db.get_games(&Default::default())? {
        owners.push((
            OWNER_GAME.to_string(),
            game.id,
            game.cover_local.or(game.cover_url),
        ));
    }
    for review in db.get_reviews(&Default::default())? {
        owners.push((
            OWNER_REVIEW.to_string(),
            review.id,
            review.cover_local.or(review.cover_url),
        ));
    }

    for (kind, id, cover_path) in owners {
        if !db.cover_rows(&kind, &id)?.is_empty() {
            continue; // 已登记
        }
        let Some(path_str) = cover_path else { continue };
        let src = Path::new(&path_str);
        if !src.exists() {
            report.missing_file += 1;
            continue;
        }
        match migrate_existing_cover(db, &kind, &id, src) {
            Ok(true) => report.indexed += 1,
            Ok(false) => {}
            Err(e) => {
                report.failed += 1;
                tracing::warn!("封面迁移失败 {} {}: {}", kind, id, e);
            }
        }
    }

    // ---- 第 2 步：已移除条目回挂（墓碑 ← 同名手账） ----
    for (tomb_id, name) in db.all_tombstones()? {
        if !db.cover_rows(OWNER_GAME, &tomb_id)?.is_empty() {
            continue;
        }
        let Some(review_id) = db.find_review_id_by_game_name(&name)? else {
            continue;
        };
        match share_from(db, OWNER_REVIEW, &review_id, OWNER_GAME, &tomb_id, STATE_ARCHIVED) {
            Ok(true) => {
                report.relinked += 1;
                tracing::info!("已移除条目「{}」从手账回挂封面", name);
            }
            Ok(false) => {}
            Err(e) => {
                report.failed += 1;
                tracing::warn!("回挂封面失败 {}: {}", name, e);
            }
        }
    }

    report.bytes_after = dir_size(&path::get_covers_dir());
    db.set_setting(MIGRATION_FLAG, "1")?;
    db.set_setting(MIGRATION_FLAG_AGAIN, "0")?;
    tracing::info!(
        "封面迁移完成：登记 {}，回挂 {}，缺文件 {}，失败 {}，目录 {} → {} 字节",
        report.indexed,
        report.relinked,
        report.missing_file,
        report.failed,
        report.bytes_before,
        report.bytes_after
    );
    Ok(report)
}

fn lock_db(db: &Arc<Mutex<Database>>) -> MutexGuard<'_, Database> {
    match db.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn owner_exists(db: &Database, owner_kind: &str, owner_id: &str) -> Result<bool> {
    match owner_kind {
        OWNER_GAME => Ok(db.get_game_by_id(owner_id)?.is_some()),
        OWNER_REVIEW => Ok(db.get_review_by_id(owner_id)?.is_some()),
        _ => Ok(false),
    }
}

/// 无锁准备一条存量封面的索引行与暂存文件。
///
/// 自己的文件走 `prepare_path`（转码 + 生成缩略图）；历史共享文件保留原主图，
/// 只替换主图行的元数据并暂存新缩略图。所有解码/编码都在 DB 锁外完成。
fn prepare_migration_cover(
    owner_kind: &str,
    owner_id: &str,
    src: &Path,
) -> Result<PreparedCover> {
    let own_file = src
        .file_stem()
        .map(|s| s.to_string_lossy() == owner_id)
        .unwrap_or(false);
    let mut prepared = prepare_path(owner_kind, owner_id, src)?;
    if own_file {
        return Ok(prepared);
    }

    let rel = rel_path_of(src).context("共享封面不在 covers 目录内")?;
    let bytes = std::fs::read(src).with_context(|| format!("读取共享封面失败: {}", src.display()))?;
    let (width, height) = image::image_dimensions(src)?;

    // prepare_path 为「自己的文件」生成了主图暂存；共享文件不需要替换主图，
    // 删掉这个暂存，再把 main 行改指回原始 rel_path。
    let mut staged = std::mem::take(&mut prepared.staged);
    if !staged.is_empty() {
        let (main_tmp, _) = staged.remove(0);
        let _ = std::fs::remove_file(main_tmp);
    }
    prepared.staged = staged;
    if let Some(main) = prepared.rows.first_mut() {
        main.rel_path = rel;
        main.sha256 = sha256_hex(&bytes);
        main.width = width;
        main.height = height;
        main.bytes = bytes.len() as u64;
    }

    // 缩略图已存在则沿用旧文件，不重复覆盖；索引元数据按磁盘上的真实字节写。
    let thumb_target = prepared.rows.get(1).map(|r| abs_path(&r.rel_path));
    if let Some(target) = thumb_target {
        if target.exists() {
            if let Some((tmp, _)) = prepared.staged.pop() {
                let _ = std::fs::remove_file(tmp);
            }
            let existing = std::fs::read(&target)?;
            let (tw, th) = image::image_dimensions(&target)?;
            if let Some(thumb) = prepared.rows.get_mut(1) {
                thumb.sha256 = sha256_hex(&existing);
                thumb.width = tw;
                thumb.height = th;
                thumb.bytes = existing.len() as u64;
            }
        }
    }
    Ok(prepared)
}

/// 生产用迁移入口：只在一张图的 prepare / commit 之间短暂拿锁。
pub fn migrate_with_short_locks(db: &Arc<Mutex<Database>>) -> Result<CoverMigrateReport> {
    let (done, rerun) = {
        let guard = lock_db(db);
        (
            guard.get_setting(MIGRATION_FLAG)?.as_deref() == Some("1"),
            guard.get_setting(MIGRATION_FLAG_AGAIN)?.as_deref() == Some("1"),
        )
    };
    if done && !rerun {
        return Ok(CoverMigrateReport::default());
    }
    {
        let guard = lock_db(db);
        guard.ensure_cover_storage()?;
    }

    let mut report = CoverMigrateReport {
        bytes_before: dir_size(&path::get_covers_dir()),
        ..Default::default()
    };
    let owners: Vec<(String, String, Option<String>)> = {
        let guard = lock_db(db);
        let mut owners = Vec::new();
        for game in guard.get_games(&Default::default())? {
            owners.push((OWNER_GAME.to_string(), game.id, game.cover_local.or(game.cover_url)));
        }
        for review in guard.get_reviews(&Default::default())? {
            owners.push((OWNER_REVIEW.to_string(), review.id, review.cover_local.or(review.cover_url)));
        }
        owners
    };

    for (kind, id, cover_path) in owners {
        let Some(path_str) = cover_path else { continue };
        let src = Path::new(&path_str);
        let needs_migration = {
            let guard = lock_db(db);
            guard.cover_rows(&kind, &id)?.is_empty()
        };
        if !needs_migration {
            continue;
        }
        if !src.exists() {
            report.missing_file += 1;
            continue;
        }

        let prepared = match prepare_migration_cover(&kind, &id, src) {
            Ok(p) => p,
            Err(e) => {
                report.failed += 1;
                tracing::warn!("封面迁移预处理失败 {} {}: {}", kind, id, e);
                continue;
            }
        };
        let outcome = (|| -> Result<bool> {
            let guard = lock_db(db);
            if !owner_exists(&guard, &kind, &id)? || !guard.cover_rows(&kind, &id)?.is_empty() {
                return Ok(false);
            }
            let set = commit_prepared(&guard, &kind, &id, prepared)?;
            cleanup_ingest_source(&guard, src, &set, true)?;
            Ok(true)
        })();
        match outcome {
            Ok(true) => report.indexed += 1,
            Ok(false) => {}
            Err(e) => {
                report.failed += 1;
                tracing::warn!("封面迁移失败 {} {}: {}", kind, id, e);
            }
        }
    }

    let tombstones = {
        let guard = lock_db(db);
        guard.all_tombstones()?
    };
    for (tomb_id, name) in tombstones {
        let outcome = (|| -> Result<bool> {
            let guard = lock_db(db);
            if !guard.cover_rows(OWNER_GAME, &tomb_id)?.is_empty() {
                return Ok(false);
            }
            let Some(review_id) = guard.find_review_id_by_game_name(&name)? else {
                return Ok(false);
            };
            share_from(&guard, OWNER_REVIEW, &review_id, OWNER_GAME, &tomb_id, STATE_ARCHIVED)
        })();
        match outcome {
            Ok(true) => {
                report.relinked += 1;
                tracing::info!("已移除条目「{}」从手账回挂封面", name);
            }
            Ok(false) => {}
            Err(e) => {
                report.failed += 1;
                tracing::warn!("回挂封面失败 {}: {}", name, e);
            }
        }
    }

    report.bytes_after = dir_size(&path::get_covers_dir());
    {
        let guard = lock_db(db);
        guard.set_setting(MIGRATION_FLAG, "1")?;
        guard.set_setting(MIGRATION_FLAG_AGAIN, "0")?;
    }
    tracing::info!(
        "封面迁移完成：登记 {}，回挂 {}，缺文件 {}，失败 {}，目录 {} → {} 字节",
        report.indexed,
        report.relinked,
        report.missing_file,
        report.failed,
        report.bytes_before,
        report.bytes_after
    );
    Ok(report)
}

/// 生产用缩略图升级：解码/编码/写暂存全在锁外，逐组短锁更新索引。
pub fn upgrade_thumbs_with_short_locks(db: &Arc<Mutex<Database>>) -> Result<ThumbUpgradeReport> {
    let want = THUMB_MAX_EDGE.to_string();
    let (done, rows) = {
        let guard = lock_db(db);
        let done = guard.get_setting(THUMB_SPEC_FLAG)?.as_deref() == Some(want.as_str());
        let rows = if done { Vec::new() } else { guard.all_cover_rows()? };
        (done, rows)
    };
    if done {
        return Ok(ThumbUpgradeReport::default());
    }
    path::ensure_cover_dirs()?;

    let mut groups: std::collections::BTreeMap<String, Vec<CoverIndexRow>> =
        std::collections::BTreeMap::new();
    let mut main_by_owner: std::collections::HashMap<(String, String), CoverIndexRow> =
        std::collections::HashMap::new();
    for row in rows {
        if row.kind == KIND_MAIN {
            main_by_owner
                .entry((row.owner_kind.clone(), row.owner_id.clone()))
                .or_insert(row.clone());
        }
        if row.kind == KIND_THUMB {
            groups.entry(row.rel_path.clone()).or_default().push(row);
        }
    }

    let mut report = ThumbUpgradeReport::default();
    for (rel_path, group) in groups {
        let abs = abs_path(&rel_path);
        match image::image_dimensions(&abs) {
            Ok((w, h)) if w.max(h) >= THUMB_MAX_EDGE => {
                report.skipped += 1;
                continue;
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("缩略图重建跳过（读不到尺寸）{}: {}", rel_path, e);
                report.skipped += 1;
                continue;
            }
        }

        let first = &group[0];
        let main_abs = main_by_owner
            .get(&(first.owner_kind.clone(), first.owner_id.clone()))
            .map(|r| abs_path(&r.rel_path))
            .filter(|p| p.exists());
        let Some(main_abs) = main_abs else {
            tracing::warn!("缩略图重建跳过（主图缺失）: {}", rel_path);
            report.skipped += 1;
            continue;
        };
        let img = match image::open(&main_abs) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!("缩略图重建失败（主图解码）{}: {}", main_abs.display(), e);
                report.failed += 1;
                continue;
            }
        };
        let (bytes, tw, th, thumb_ext) = match build_thumb(&img) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("缩略图重建失败（编码）{}: {}", rel_path, e);
                report.failed += 1;
                continue;
            }
        };
        let new_rel = swap_ext(&rel_path, thumb_ext);
        let (tmp, dst) = match write_staged(&abs_path(&new_rel), &bytes) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("缩略图重建写盘失败 {}: {}", new_rel, e);
                report.failed += 1;
                continue;
            }
        };
        let sha = sha256_hex(&bytes);
        let now = now_rfc3339();

        let outcome = (|| -> Result<()> {
            let guard = lock_db(db);
            // 锁外准备期间状态可能变化：提交时重新读取当前行，避免把旧快照的
            // owner/state 写回去，也顺带确认旧 rel_path 仍是这条缩略图。
            let mut current_rows = Vec::with_capacity(group.len());
            for row in &group {
                let current = guard.cover_rows(&row.owner_kind, &row.owner_id)?;
                let Some(thumb_row) = current
                    .into_iter()
                    .find(|r| r.kind == KIND_THUMB && r.rel_path == rel_path)
                else {
                    anyhow::bail!("封面索引在重建期间发生变化: {}", rel_path);
                };
                current_rows.push(thumb_row);
            }
            let swap = FileSwap::apply(&[(tmp.clone(), dst.clone())])?;
            let db_result = (|| -> Result<()> {
                for row in &current_rows {
                    if new_rel != rel_path {
                        guard.set_cover_rel_path(
                            &row.owner_kind,
                            &row.owner_id,
                            KIND_THUMB,
                            &new_rel,
                            &row.state,
                        )?;
                    }
                    guard.update_cover_meta(
                        &row.owner_kind,
                        &row.owner_id,
                        KIND_THUMB,
                        &sha,
                        tw,
                        th,
                        bytes.len() as u64,
                        &now,
                    )?;
                    guard.touch_cover_row(&row.owner_kind, &row.owner_id, KIND_MAIN, &now)?;
                }
                Ok(())
            })();
            match db_result {
                Ok(()) => {
                    swap.commit();
                    if new_rel != rel_path && guard.cover_rel_refcount(&rel_path)? == 0 {
                        let _ = std::fs::remove_file(abs_path(&rel_path));
                    }
                    Ok(())
                }
                Err(e) => {
                    swap.rollback();
                    Err(e)
                }
            }
        })();
        match outcome {
            Ok(()) => report.rebuilt += 1,
            Err(e) => {
                tracing::warn!("缩略图重建提交失败 {}: {}", rel_path, e);
                report.failed += 1;
            }
        }
        // 正常路径中 tmp 已被 rename；异常/索引变化时清掉暂存文件。
        let _ = std::fs::remove_file(&tmp);
    }

    {
        let guard = lock_db(db);
        guard.set_setting(THUMB_SPEC_FLAG, &want)?;
    }
    Ok(report)
}

/// 目录占用（含子目录），用于迁移前后体积对比
fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            total += dir_size(&p);
        } else if let Ok(meta) = entry.metadata() {
            total += meta.len();
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbImage, RgbaImage};

    /// 测试专用封面根目录（避免动到用户真实 covers 库）
    fn test_root() -> PathBuf {
        let dir = std::env::temp_dir().join("gamevault-cover-store-tests");
        path::set_covers_dir_override(dir.clone());
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn db() -> Database {
        test_root();
        Database::new(std::path::Path::new(":memory:")).expect("内存库初始化失败")
    }

    /// 一步入库（预处理 + 提交），测试专用：与生产调用方走同一条 prepare/commit 路径
    fn ingest_bytes(db: &Database, owner_kind: &str, owner_id: &str, bytes: &[u8]) -> Result<CoverSet> {
        let prepared = prepare_bytes(owner_kind, owner_id, bytes)?;
        commit_prepared(db, owner_kind, owner_id, prepared)
    }

    /// 不透明 PNG（噪声图案：PNG 压不动、JPEG 明显更小，贴近真实封面图）
    fn opaque_png(w: u32, h: u32) -> Vec<u8> {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |_, _| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let b = (seed >> 33) as u8;
            image::Rgb([b, b.wrapping_add(70), b.wrapping_mul(3).wrapping_add(11)])
        }));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    /// 含真透明的 PNG（应当保留 alpha）
    fn alpha_png(w: u32, h: u32) -> Vec<u8> {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_fn(w, h, |x, _| {
            image::Rgba([200, 40, 40, if x < w / 2 { 0 } else { 255 }])
        }));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    fn file_of(set: &CoverSet) -> (PathBuf, PathBuf) {
        (
            PathBuf::from(set.main.clone().unwrap()),
            PathBuf::from(set.thumb.clone().unwrap()),
        )
    }

    /// 入库：索引两行、两个文件都在、缩略图长边 ≤256
    #[test]
    fn ingest_writes_index_and_thumbnail() {
        let db = db();
        let id = uuid::Uuid::new_v4().to_string();
        let set = ingest_bytes(&db, OWNER_GAME, &id, &opaque_png(1200, 1800)).unwrap();
        let (main, thumb) = file_of(&set);
        assert!(main.exists() && thumb.exists(), "主图与缩略图都应落盘");
        assert!(thumb.components().any(|c| c.as_os_str() == "thumb"), "缩略图进 thumb/ 子目录");

        let rows = db.cover_rows(OWNER_GAME, &id).unwrap();
        assert_eq!(rows.len(), 2, "索引应有 main + thumb 两行");
        assert!(rows.iter().all(|r| !r.sha256.is_empty() && r.state == STATE_ACTIVE));

        let thumb_img = image::open(&thumb).unwrap();
        assert!(thumb_img.width().max(thumb_img.height()) <= THUMB_MAX_EDGE);

        // 游戏表封面字段被同步（兼容旧读取路径）
        let mut game = crate::models::Game::new("缩略图测试".into());
        game.id = id.clone();
        db.upsert_game(&game).unwrap();
        sync_owner_paths(&db, OWNER_GAME, &id).unwrap();
        let saved = db.get_game_by_id(&id).unwrap().unwrap();
        assert_eq!(saved.cover_local.as_deref(), Some(main.to_string_lossy().as_ref()));
    }

    /// 回归（2026-09-24）：`CoverSet.updated_at` 必须反映**封面文件最后一次被替换**的
    /// 时刻，且换封面后必须变——前端拿它拼 asset URL 的破缓存参数，若它不变，
    /// WebView2 会继续显示旧图（老爷实测的"卡片换封面不刷新"）。
    ///
    /// 同时守一个反向性质：**没换封面时它不该变**。否则前端 URL 无谓抖动、
    /// 白烧一次图片解码。
    #[test]
    fn cover_set_timestamp_tracks_replacement_only() {
        let db = db();
        let id = uuid::Uuid::new_v4().to_string();

        let first = ingest_bytes(&db, OWNER_GAME, &id, &opaque_png(600, 900)).unwrap();
        let t1 = first.updated_at.clone().expect("入库后应带时间戳");
        assert_eq!(
            t1,
            db.cover_rows(OWNER_GAME, &id).unwrap()
                .iter()
                .find(|r| r.kind == KIND_MAIN)
                .unwrap()
                .updated_at,
            "时间戳必须与索引表主图行一致（前端靠它破缓存）"
        );

        // `resolve` 是另一条取图通道，必须给出同一个时间戳
        let resolved = resolve(&db, OWNER_GAME, &id).unwrap();
        assert_eq!(resolved.updated_at.as_deref(), Some(t1.as_str()));

        // 不换封面：连跑几次 resolve / 读索引，时间戳必须岿然不动
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            assert_eq!(
                resolve(&db, OWNER_GAME, &id).unwrap().updated_at.as_deref(),
                Some(t1.as_str()),
                "没换封面时时间戳不得变动（否则前端 URL 白抖）"
            );
        }

        // 换封面（换一张不同内容的图）：时间戳必须变
        std::thread::sleep(std::time::Duration::from_millis(5));
        let second = ingest_bytes(&db, OWNER_GAME, &id, &opaque_png(700, 1000)).unwrap();
        let t2 = second.updated_at.clone().expect("入库后应带时间戳");
        assert_ne!(t1, t2, "换封面后时间戳必须变化，否则前端破缓存失效");

        // 且新值同样与索引表一致（旧行已被 replace 掉）
        assert_eq!(
            t2,
            db.cover_rows(OWNER_GAME, &id).unwrap()
                .iter()
                .find(|r| r.kind == KIND_MAIN)
                .unwrap()
                .updated_at
        );
    }

    /// 回归（2026-09-24）：封面文件名恒为 `<owner_id>.<ext>`，换封面是**原地覆盖同名文件**
    /// ——这正是不拼破缓存参数就会显示旧图的根因。本测试把这个前提钉住：**换了图，
    /// 路径逐字节不变**。哪天真改成"按内容哈希命名"，此测试会失败，提醒重新审视
    /// 前端那条破缓存逻辑是否还需要。
    #[test]
    fn cover_path_stays_identical_across_replacement() {
        let db = db();
        let id = uuid::Uuid::new_v4().to_string();

        let first = ingest_bytes(&db, OWNER_GAME, &id, &opaque_png(600, 900)).unwrap();
        let second = ingest_bytes(&db, OWNER_GAME, &id, &opaque_png(800, 1200)).unwrap();

        assert_eq!(first.main, second.main, "换封面应在原地覆盖同名文件（路径不变）");
        assert_ne!(first.updated_at, second.updated_at, "但时间戳必须变，供前端破缓存");
        assert!(std::path::Path::new(second.main.as_ref().unwrap()).exists());
    }

    /// 编码策略：不透明 PNG 转 JPEG（省体积）；含透明 PNG 保留 PNG（保 alpha）
    #[test]
    fn opaque_png_is_transcoded_and_alpha_png_is_not() {
        let db = db();
        let opaque_id = uuid::Uuid::new_v4().to_string();
        let set = ingest_bytes(&db, OWNER_GAME, &opaque_id, &opaque_png(900, 1400)).unwrap();
        assert!(
            set.main.as_ref().unwrap().ends_with(".jpg"),
            "不透明 PNG 应转 JPEG，实际 {}",
            set.main.unwrap()
        );

        let alpha_id = uuid::Uuid::new_v4().to_string();
        let set = ingest_bytes(&db, OWNER_REVIEW, &alpha_id, &alpha_png(600, 900)).unwrap();
        assert!(
            set.main.as_ref().unwrap().ends_with(".png"),
            "含透明 PNG 不应转 JPEG（会丢 alpha），实际 {}",
            set.main.unwrap()
        );
        // 透明图的缩略图走无损 WebP
        assert!(set.thumb.as_ref().unwrap().ends_with(".webp"));
    }

    /// 缩略图预算：源图长边小于规格时**不放大**（旧规格会一路放大到 256）
    #[test]
    fn thumb_is_not_upscaled_for_small_source() {
        let db = db();
        let id = uuid::Uuid::new_v4().to_string();
        let set = ingest_bytes(&db, OWNER_GAME, &id, &opaque_png(300, 400)).unwrap();
        let thumb = image::open(set.thumb.as_ref().unwrap()).unwrap();
        assert_eq!(
            (thumb.width(), thumb.height()),
            (300, 400),
            "小图不得被放大成 {} 长边",
            THUMB_MAX_EDGE
        );
    }

    /// 换扩展名的路径拼接（升级时 jpg ↔ webp 互转要用）
    #[test]
    fn swap_ext_only_touches_last_segment() {
        assert_eq!(swap_ext("thumb/x.jpg", "webp"), "thumb/x.webp");
        assert_eq!(swap_ext("archive/thumb/x.jpg", "png"), "archive/thumb/x.png");
        assert_eq!(swap_ext("x", "jpg"), "x.jpg");
    }

    /// 关键回归（2026-09-27）：`THUMB_MAX_EDGE` 调大后，**存量小缩略图必须被重建**，
    /// 且要推进主图行时间戳 —— 否则前端 asset URL 不变，WebView2 继续显示缓存里的
    /// 旧小图，用户看到的是"改了没生效"（卡片还是糊的）。
    #[test]
    fn thumb_spec_upgrade_rebuilds_small_thumbs_and_busts_cache() {
        let db = db();
        let id = uuid::Uuid::new_v4().to_string();
        let set = ingest_bytes(&db, OWNER_GAME, &id, &opaque_png(1200, 1800)).unwrap();
        let thumb_path = std::path::PathBuf::from(set.thumb.clone().unwrap());

        // 造一张"旧规格"缩略图覆盖上去，并把索引同步成它的尺寸（模拟升级前的存量）
        let old = image::open(&thumb_path).unwrap().thumbnail(256, 256);
        let (ow, oh) = (old.width(), old.height());
        let mut old_bytes = Vec::new();
        old.write_to(&mut std::io::Cursor::new(&mut old_bytes), image::ImageFormat::Jpeg).unwrap();
        std::fs::write(&thumb_path, &old_bytes).unwrap();
        db.update_cover_meta(OWNER_GAME, &id, KIND_THUMB, &sha256_hex(&old_bytes), ow, oh, old_bytes.len() as u64, "2026-01-01T00:00:00Z").unwrap();

        // 标记置为未达标 → 触发升级
        db.set_setting(THUMB_SPEC_FLAG, "0").unwrap();
        let main_t1 = db
            .cover_rows(OWNER_GAME, &id)
            .unwrap()
            .into_iter()
            .find(|r| r.kind == KIND_MAIN)
            .unwrap()
            .updated_at;
        std::thread::sleep(std::time::Duration::from_millis(5));

        let report = upgrade_thumbs(&db).unwrap();
        assert_eq!(report.rebuilt, 1, "存量小缩略图应被重建");
        assert_eq!(report.failed, 0);

        let now_img = image::open(&thumb_path).unwrap();
        assert!(
            now_img.width().max(now_img.height()) >= THUMB_MAX_EDGE,
            "重建后长边应达到新规格，实际 {}x{}",
            now_img.width(),
            now_img.height()
        );

        let rows = db.cover_rows(OWNER_GAME, &id).unwrap();
        let main_row = rows.iter().find(|r| r.kind == KIND_MAIN).unwrap();
        let thumb_row = rows.iter().find(|r| r.kind == KIND_THUMB).unwrap();
        assert_eq!(
            (thumb_row.width, thumb_row.height),
            (now_img.width(), now_img.height()),
            "索引尺寸必须与落盘文件一致（否则前端拿着对不上的账）"
        );
        assert_ne!(
            main_row.updated_at, main_t1,
            "必须推进主图行时间戳，否则前端 URL 不变、WebView2 命中缓存显示旧小图"
        );

        // 幂等：标记已达标时短路返回（不重复扫描），且不再动时间戳
        let want = THUMB_MAX_EDGE.to_string();
        let again = upgrade_thumbs(&db).unwrap();
        assert_eq!((again.rebuilt, again.skipped, again.failed), (0, 0, 0), "已达标应短路返回");
        assert_eq!(
            db.get_setting(THUMB_SPEC_FLAG).unwrap().as_deref(),
            Some(want.as_str()),
            "标记应记为当前规格（将来再调大时会自动重跑）"
        );

        // 再模拟一次"标记丢失"（手工清空 settings）：这次缩略图已达标，应**跳过**而非重建
        db.set_setting(THUMB_SPEC_FLAG, "0").unwrap();
        let rescan = upgrade_thumbs(&db).unwrap();
        assert_eq!((rescan.rebuilt, rescan.skipped), (0, 1), "已达标缩略图应跳过重建");
        assert_eq!(
            db.cover_rows(OWNER_GAME, &id).unwrap().iter().find(|r| r.kind == KIND_MAIN).unwrap().updated_at,
            main_row.updated_at,
            "跳过时不得动时间戳（否则前端 URL 白抖、白烧解码）"
        );
    }

    /// 归档保留、彻底删除才删文件
    #[test]
    fn archive_keeps_file_purge_removes_it() {
        let db = db();
        let id = uuid::Uuid::new_v4().to_string();
        let set = ingest_bytes(&db, OWNER_REVIEW, &id, &opaque_png(600, 900)).unwrap();
        let (main, thumb) = file_of(&set);

        archive(&db, OWNER_REVIEW, &id).unwrap();
        assert!(!main.exists() && !thumb.exists(), "归档应把文件挪出正式目录");
        let rows = db.cover_rows(OWNER_REVIEW, &id).unwrap();
        assert!(rows.iter().all(|r| r.state == STATE_ARCHIVED), "状态应转为 archived");
        assert!(rows.iter().all(|r| r.rel_path.starts_with("archive/")), "路径应挪进 archive/");
        assert!(rows.iter().all(|r| abs_path(&r.rel_path).exists()), "留档文件必须还在（这就是「删除不删图」）");

        // 认领回归：挪回正式目录
        assert!(restore(&db, OWNER_REVIEW, &id).unwrap());
        assert!(abs_path(&db.cover_rows(OWNER_REVIEW, &id).unwrap()[0].rel_path).exists());

        // 彻底删除：文件真没了
        let main_now = resolve(&db, OWNER_REVIEW, &id).unwrap().main.unwrap();
        purge(&db, OWNER_REVIEW, &id).unwrap();
        assert!(db.cover_rows(OWNER_REVIEW, &id).unwrap().is_empty());
        assert!(!PathBuf::from(main_now).exists(), "彻底删除后文件应消失");
    }

    /// 关键回归（2026-09-12）：留档文件被人为清掉后，认领恢复**不得**把「文件已不存在」的
    /// 行置回 active，也不得把留档路径写进封面字段——否则活条目会指向一张不存在的图。
    #[test]
    fn restore_keeps_archived_when_file_missing() {
        let db = db();
        let id = uuid::Uuid::new_v4().to_string();
        let mut game = crate::models::Game::new("留档文件丢失测试".into());
        game.id = id.clone();
        db.upsert_game(&game).unwrap();

        let set = ingest_bytes(&db, OWNER_GAME, &id, &opaque_png(600, 900)).unwrap();
        let (main, thumb) = file_of(&set);
        archive(&db, OWNER_GAME, &id).unwrap();
        assert!(!main.exists() && !thumb.exists(), "归档应把文件挪出正式目录");

        // 模拟用户手工清掉 covers/archive 里的留档图
        for row in db.cover_rows(OWNER_GAME, &id).unwrap() {
            std::fs::remove_file(abs_path(&row.rel_path)).unwrap();
        }
        let cover_before = db.get_game_by_id(&id).unwrap().unwrap().cover_local;

        // 重装认领 → 恢复失败，且不得改写状态与封面字段
        assert!(!restore(&db, OWNER_GAME, &id).unwrap(), "文件不在，不应报告恢复成功");
        let rows = db.cover_rows(OWNER_GAME, &id).unwrap();
        assert!(
            rows.iter().all(|r| r.state == STATE_ARCHIVED),
            "缺文件的留档行不得被置为 active"
        );
        assert!(
            rows.iter().all(|r| !abs_path(&r.rel_path).exists()),
            "前提校验：留档文件确实已不在"
        );
        assert_eq!(
            db.get_game_by_id(&id).unwrap().unwrap().cover_local,
            cover_before,
            "文件缺失时不得改写封面字段"
        );
    }

    /// 关键回归：共享封面的两个主体，删掉其中一个不得毁掉另一个的图
    /// （旧实现：手账从游戏导入时裸抄路径，删游戏会把共享文件一起删掉）
    #[test]
    fn shared_cover_survives_one_owner_deletion() {
        let db = db();
        let game_id = uuid::Uuid::new_v4().to_string();
        let review_id = uuid::Uuid::new_v4().to_string();

        let set = ingest_bytes(&db, OWNER_GAME, &game_id, &opaque_png(600, 900)).unwrap();
        let (main, _) = file_of(&set);
        let mut game = crate::models::Game::new("共享封面测试".into());
        game.id = game_id.clone();
        db.upsert_game(&game).unwrap();

        // 手账从游戏导入 → 共享同一物理文件
        assert!(share_from(&db, OWNER_GAME, &game_id, OWNER_REVIEW, &review_id, STATE_ACTIVE).unwrap());
        let mut review = crate::models::Review::new("共享封面测试".into());
        review.id = review_id.clone();
        db.insert_review(&review).unwrap();
        sync_owner_paths(&db, OWNER_REVIEW, &review_id).unwrap();

        let game_rel = db.cover_rows(OWNER_GAME, &game_id).unwrap()[0].rel_path.clone();
        let review_rel = db.cover_rows(OWNER_REVIEW, &review_id).unwrap()[0].rel_path.clone();
        assert_eq!(game_rel, review_rel, "共享引用应指向同一文件");
        assert_eq!(db.cover_sha_refcount(&db.cover_rows(OWNER_GAME, &game_id).unwrap()[0].sha256).unwrap(), 2);

        // 删掉游戏（连图一起删）——手账那张必须活着
        db.delete_game(&game_id).unwrap();
        purge(&db, OWNER_GAME, &game_id).unwrap();
        assert!(main.exists(), "仍被手账引用，文件不能删");
        assert_eq!(db.cover_rows(OWNER_REVIEW, &review_id).unwrap().len(), 2);
        assert!(resolve(&db, OWNER_REVIEW, &review_id).unwrap().main.is_some());

        // 手账也删掉后，引用归零 → 文件才真正消失
        purge(&db, OWNER_REVIEW, &review_id).unwrap();
        assert!(!main.exists(), "引用归零后文件应被回收");
    }

    /// 迁移：存量封面登记 + 已移除条目从同名手账回挂图片（Mafia 那类场景）
    #[test]
    fn migration_indexes_existing_and_relinks_removed_game() {
        let db = db();
        let review_id = uuid::Uuid::new_v4().to_string();

        // 手账里有一张图（模拟"游戏删了，手账还有"）
        let set = ingest_bytes(&db, OWNER_REVIEW, &review_id, &opaque_png(600, 900)).unwrap();
        let (main, _) = file_of(&set);
        let mut review = crate::models::Review::new("Mafia: The Old Country".into());
        review.id = review_id.clone();
        db.insert_review(&review).unwrap();
        sync_owner_paths(&db, OWNER_REVIEW, &review_id).unwrap();

        // 已移除游戏：只有墓碑，没有封面
        let gone_id = uuid::Uuid::new_v4().to_string();
        db.insert_tombstone(&gone_id, "Mafia: The Old Country", None, Some("MafiaTheOldCountry.exe"), "2026-09-10T00:00:00Z").unwrap();

        let report = migrate(&db).unwrap();
        assert!(report.relinked >= 1, "已移除条目应从同名手账回挂封面");
        let rows = db.cover_rows(OWNER_GAME, &gone_id).unwrap();
        assert!(!rows.is_empty(), "墓碑 id 下应有封面索引");
        assert!(rows.iter().all(|r| r.state == STATE_ARCHIVED), "已移除条目应标 archived");
        assert_eq!(rows[0].rel_path, db.cover_rows(OWNER_REVIEW, &review_id).unwrap()[0].rel_path);
        assert!(main.exists());

        // 幂等：再跑一次不重复处理
        let again = migrate(&db).unwrap();
        assert_eq!(again.relinked, 0);
        assert_eq!(again.indexed, 0);
    }

    /// 同名匹配优先，英文名也能命中
    #[test]
    fn tombstone_relink_matches_english_name() {
        let db = db();
        let review_id = uuid::Uuid::new_v4().to_string();
        let mut review = crate::models::Review::new("黑手党：故乡".into());
        review.id = review_id.clone();
        review.name_en = Some("Mafia: The Old Country".into());
        db.insert_review(&review).unwrap();
        assert_eq!(
            db.find_review_id_by_game_name("mafia: the old country").unwrap().as_deref(),
            Some(review_id.as_str())
        );
        assert_eq!(
            db.find_review_id_by_game_name("黑手党：故乡").unwrap().as_deref(),
            Some(review_id.as_str())
        );
        assert!(db.find_review_id_by_game_name("不存在").unwrap().is_none());
    }

    /// 真实数据演练（默认忽略，**只跑副本**）：
    /// ```text
    /// GV_DRYRUN_DB=<db 副本路径> GV_DRYRUN_COVERS=<covers 副本目录> \
    ///   cargo test --lib real_data_migration_dryrun -- --ignored --nocapture
    /// ```
    /// 上线前验证迁移是否符合预期，全程不触碰用户真实数据。
    #[test]
    #[ignore]
    fn real_data_migration_dryrun() {
        let Ok(db_path) = std::env::var("GV_DRYRUN_DB") else {
            eprintln!("未设置 GV_DRYRUN_DB，跳过");
            return;
        };
        let covers = std::env::var("GV_DRYRUN_COVERS").expect("需设置 GV_DRYRUN_COVERS");
        path::set_covers_dir_override(PathBuf::from(&covers));

        let db = Database::new(std::path::Path::new(&db_path)).expect("打开副本库失败");

        let before: Vec<_> = db.all_cover_rows().unwrap();
        println!("迁移前索引行数: {}", before.len());

        let report = migrate(&db).unwrap();
        println!(
            "迁移报告: 登记 {}，回挂 {}，缺文件 {}，失败 {}，体积 {} → {} 字节",
            report.indexed, report.relinked, report.missing_file, report.failed, report.bytes_before, report.bytes_after
        );

        for row in db.all_cover_rows().unwrap() {
            let abs = abs_path(&row.rel_path);
            println!(
                "  {:<6} {:<36} {:<8} {:<44} {:>8}B  {}  存在={}",
                row.owner_kind,
                row.owner_id,
                row.kind,
                row.rel_path,
                row.bytes,
                row.state,
                abs.exists()
            );
        }
        let tombstones = db.all_tombstones().unwrap();
        for (id, name) in tombstones {
            let rows = db.cover_rows(OWNER_GAME, &id).unwrap();
            println!("墓碑「{}」封面行数 = {}", name, rows.len());
        }
    }

    /// 生产入口必须能在 Arc<Mutex<Database>> 上跑通，且共享主图不被重写。
    #[test]
    fn short_lock_migration_and_thumb_upgrade_work() {
        use std::sync::{Arc, Mutex};

        let db = db();
        let id = uuid::Uuid::new_v4().to_string();
        let src = test_root().join(format!("legacy-{id}.png"));
        let original = opaque_png(1200, 1800);
        std::fs::write(&src, &original).unwrap();

        let mut game = crate::models::Game::new("短锁迁移测试".into());
        game.id = id.clone();
        game.cover_local = Some(src.to_string_lossy().to_string());
        db.upsert_game(&game).unwrap();

        let shared = Arc::new(Mutex::new(db));
        let report = migrate_with_short_locks(&shared).unwrap();
        assert_eq!(report.indexed, 1, "应登记一条存量封面，报告 {:?}", report);
        assert_eq!(std::fs::read(&src).unwrap(), original, "共享主图不得被改写");

        {
            let guard = shared.lock().unwrap();
            assert_eq!(guard.cover_rows(OWNER_GAME, &id).unwrap().len(), 2);
        }
        let thumb_report = upgrade_thumbs_with_short_locks(&shared).unwrap();
        assert_eq!(thumb_report.failed, 0, "缩略图升级不应失败");

        let want = THUMB_MAX_EDGE.to_string();
        let guard = shared.lock().unwrap();
        assert_eq!(
            guard.get_setting(THUMB_SPEC_FLAG).unwrap().as_deref(),
            Some(want.as_str()),
            "升级完成必须写入规格标记"
        );
    }
}
