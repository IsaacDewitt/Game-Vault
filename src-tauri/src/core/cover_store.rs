//! 封面存储：`covers` 索引表 + `covers` 目录的权威读写层（2026-09-10 立）
//!
//! ## 目录布局（单一根目录，按用途分桶，不散落）
//! ```text
//! %APPDATA%/GameVault/covers/
//!   <owner_id>.<ext>            主图（游戏/手账共用；ext 视源格式而定）
//!   thumb/<owner_id>.<ext>      缩略图（长边 ≤256，卡片网格/统计排行使用）
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
//! - 缩略图：不透明 → JPEG q82；含真透明 → 无损 WebP（保留 alpha，256px 下体积可控）。

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use crate::core::Database;
use crate::models::cover::*;
use crate::utils::path;

/// 缩略图长边像素上限
pub const THUMB_MAX_EDGE: u32 = 256;
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

/// 写文件：先写 `.tmp`，校验大小后替换正式文件（Windows 上 rename 不覆盖，先删旧）
fn write_atomic(target: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = target.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(&tmp, bytes).with_context(|| format!("写入临时文件失败: {}", tmp.display()))?;
    // 校验：文件真的落盘且大小一致，避免半张图覆盖掉好图
    let written = std::fs::metadata(&tmp)?.len();
    if written != bytes.len() as u64 {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("封面写入校验失败（期望 {} 字节，实际 {}）", bytes.len(), written);
    }
    let _ = std::fs::remove_file(target);
    std::fs::rename(&tmp, target)
        .with_context(|| format!("替换封面文件失败: {}", target.display()))?;
    Ok(())
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
    let rgba = img.to_rgba8();
    rgba.pixels().any(|p| p.0[3] < 255)
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

/// 生成缩略图字节：先等比缩到长边 ≤256，再按透明度选编码
fn build_thumb(img: &image::DynamicImage) -> Result<Vec<u8>> {
    let thumb = img.thumbnail(THUMB_MAX_EDGE, THUMB_MAX_EDGE);
    if has_real_alpha(&thumb) {
        encode_lossless_webp(&thumb)
    } else {
        encode_jpeg(&thumb, THUMB_JPEG_QUALITY)
    }
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
pub fn resolve(db: &Database, owner_kind: &str, owner_id: &str) -> Result<CoverSet> {
    let mut set = CoverSet::default();
    for row in db.cover_rows(owner_kind, owner_id)? {
        let abs = abs_path(&row.rel_path);
        if !abs.exists() {
            continue;
        }
        let s = abs.to_string_lossy().to_string();
        match row.kind.as_str() {
            KIND_MAIN => set.main = Some(s),
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
}

/// 预处理（**不接触数据库**）：识别格式 → 解码 → 编码主图/缩略图 → 落盘 → 生成索引行。
///
/// 副作用只有两个原子写入（主图与缩略图）；索引尚未登记，提交前这些文件是"孤儿"，
/// 由 [`commit_prepared`] 收编。
pub fn prepare_bytes(owner_kind: &str, owner_id: &str, bytes: &[u8]) -> Result<PreparedCover> {
    path::ensure_cover_dirs()?;

    let format = image::guess_format(bytes).context("无法识别图片格式")?;
    let img = image::load_from_memory(bytes).context("无法解码图片")?;
    let (width, height) = (img.width(), img.height());

    let (main_bytes, main_ext) = build_main(&img, bytes, format)?;
    let thumb_bytes = build_thumb(&img)?;
    let thumb_ext = if thumb_bytes.starts_with(b"RIFF") { "webp" } else { "jpg" };

    let main_rel = format!("{}.{}", owner_id, main_ext);
    let thumb_rel = format!("thumb/{}.{}", owner_id, thumb_ext);
    write_atomic(&abs_path(&main_rel), &main_bytes)?;
    write_atomic(&abs_path(&thumb_rel), &thumb_bytes)?;

    let now = now_rfc3339();
    let thumb_img = img.thumbnail(THUMB_MAX_EDGE, THUMB_MAX_EDGE);
    Ok(PreparedCover {
        rows: vec![
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
                width: thumb_img.width(),
                height: thumb_img.height(),
                bytes: thumb_bytes.len() as u64,
                state: STATE_ACTIVE.to_string(),
                updated_at: now,
            },
        ],
    })
}

/// 读文件 + 预处理（**不接触数据库**，供命令层在无锁段调用）
pub fn prepare_path(owner_kind: &str, owner_id: &str, src: &Path) -> Result<PreparedCover> {
    let bytes = std::fs::read(src).with_context(|| format!("读取图片失败: {}", src.display()))?;
    prepare_bytes(owner_kind, owner_id, &bytes)
}

/// 提交预处理结果：写索引 → 回收被替换掉的旧文件 → 同步封面字段。
/// 调用方须持有 DB 锁，本函数只做数据库操作与"无引用的旧文件"清理，耗时短。
pub fn commit_prepared(
    db: &Database,
    owner_kind: &str,
    owner_id: &str,
    prepared: PreparedCover,
) -> Result<CoverSet> {
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
    })
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
    let thumb_bytes = build_thumb(&img)?;
    let thumb_ext = if thumb_bytes.starts_with(b"RIFF") { "webp" } else { "jpg" };
    let thumb_rel = format!("thumb/{}.{}", owner_id, thumb_ext);
    // 缩略图已存在且非空则不覆盖（幂等）
    let thumb_abs = abs_path(&thumb_rel);
    if !thumb_abs.exists() {
        write_atomic(&thumb_abs, &thumb_bytes)?;
    }
    let thumb_img = img.thumbnail(THUMB_MAX_EDGE, THUMB_MAX_EDGE);

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
            width: thumb_img.width(),
            height: thumb_img.height(),
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

const MIGRATION_FLAG: &str = "covers_index_v1_done";
/// 强制重跑标记（置 1 → 下次启动重跑迁移）
const MIGRATION_FLAG_AGAIN: &str = "covers_index_v1_rerun";

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
        db.insert_tombstone(&gone_id, "Mafia: The Old Country", Some("MafiaTheOldCountry.exe"), "2026-09-10T00:00:00Z").unwrap();

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
}

/// 存量封面迁移（幂等，靠 settings 标记防重跑）：
/// 1. 把 games/reviews 现有封面登记进索引表；在 covers 目录内的文件原样保留，只补缩略图；
///    目录外的（历史手工路径）转码入库。
/// 2. 已移除条目（墓碑）没有封面时，按同名/英文名从手账条目**共享**一张过来
///    （这就是 Mafia: The Old Country 那类"游戏删了、手账里还留着图"的救图路径）。
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
