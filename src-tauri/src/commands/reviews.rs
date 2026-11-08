use tauri::State;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use crate::core::Database;
use crate::core::cover_fetcher::CoverFetcher;
use crate::core::cover_store;
use crate::core::llm_fetcher::{LlmFetcher, LlmConfig, LlmProtocol, LlmGameMeta};
use crate::models::cover::{OWNER_GAME, OWNER_REVIEW, STATE_ACTIVE};
use crate::models::*;
use crate::models::settings::Settings;
use crate::utils;
use super::lock_or_recover;

/// 获取手账条目列表
#[tauri::command(async)]
pub fn get_reviews(
    db: State<'_, Arc<Mutex<Database>>>,
    filter: Option<ReviewFilter>,
) -> Result<Vec<Review>, String> {
    let db = lock_or_recover(&db);
    let filter = filter.unwrap_or_default();
    db.get_reviews(&filter).map_err(|e| e.to_string())
}

/// 获取单个手账条目详情
#[tauri::command(async)]
pub fn get_review_detail(
    db: State<'_, Arc<Mutex<Database>>>,
    review_id: String,
) -> Result<Option<Review>, String> {
    let db = lock_or_recover(&db);
    db.get_review_by_id(&review_id).map_err(|e| e.to_string())
}

/// 添加手账条目（只需名字，元数据/封面由 refresh_review_info 后续填充）
#[tauri::command(async)]
pub fn add_review(
    db: State<'_, Arc<Mutex<Database>>>,
    name: String,
) -> Result<Review, String> {
    let trimmed = name.trim().to_string();
    if trimmed.is_empty() {
        return Err("游戏名称不能为空".to_string());
    }

    let db = lock_or_recover(&db);

    // 同名去重
    if db.find_review_by_name(&trimmed).map_err(|e| e.to_string())?.is_some() {
        return Err(format!("《{}》已存在于手账中", trimmed));
    }

    let mut review = Review::new(trimmed);
    normalize_review_name(&mut review);
    db.insert_review(&review).map_err(|e| e.to_string())?;
    Ok(review)
}

/// 名称语言归位：把名称按语言分配到正确的字段。
///
/// 背景（2026-09-06 修复）：此前「游戏库导入 / 手动添加 / 改名」三条路径都把名字
/// 原样塞进 `name`，纯英文原名（如 "Mafia: The Old Country"）被当成中文名存下来，
/// `name_en` 始终为空 —— 详情页英文名框空着，SteamGridDB 检索词切换
/// （见 refresh_review_info 与 fetch_review_cover_options 的 is_ascii 判定）也形同虚设。
///
/// 规则（判定口径与检索词切换保持一致：`str::is_ascii()`）：
/// - 纯 ASCII → `name` 存原名（主显示名 + 去重键，列 NOT NULL 不能留空），
///   并在 `name_en` 为空时补成同名，保证英文名框与封面检索都有值；
/// - 含非 ASCII → `name` 存中文原名，**并清空 `name_en`**（2026-10-06 补，与
///   `games.rs` `rename_game_inner` 的 else 分支同口径）。此前这里缺 else，改名成中文时
///   旧英文名原样残留 → 卡片「英文名优先展示」显示的是**与当前条目对不上的旧名**，
///   残留值还会被 `pick_search_term` 拿去当检索词搜错封面。清掉后由 LLM/手动重填。
fn normalize_review_name(review: &mut Review) {
    let name = review.name.trim().to_string();
    if name.is_empty() {
        return;
    }
    review.name = name.clone();
    if name.is_ascii() {
        if review.name_en.as_deref().is_none_or(|v| v.trim().is_empty()) {
            tracing::info!("名称为纯英文，同步填入英文名: '{}'", name);
            review.name_en = Some(name);
        }
    } else if review.name_en.is_some() {
        tracing::info!(
            "新名称含非 ASCII 字符，清空不再匹配的英文名: {:?}",
            review.name_en
        );
        review.name_en = None;
    }
}

/// LLM 名称语言闸门（2026-10-09，与 `games.rs::apply_llm_meta` 内的同款判据同口径）：
/// 条目现名含中文（非 ASCII）时，**丢弃** LLM 返回的纯 ASCII `name`。
///
/// 病灶：LLM 面对英文原名的游戏常把官方英文名原样回填进 name，一次「刷新信息」就把
/// 用户手上的中文名顶成英文；随后 `normalize_review_name` 见现名是 ASCII，又会把 `name_en`
/// 补成同一个英文串 —— 详情页「中文名 / 英文名」双栏一起变英文。
///
/// **只在 LLM 路径调用**：`update_review_meta`（手动改名）复用的是 `apply_meta_to_review`，
/// 把闸门塞进那里会连用户「故意把条目改成英文名」的操作一并挡掉。
fn drop_english_name_for_chinese_entry(meta: &mut LlmGameMeta, current_name: &str) {
    if let Some(incoming) = meta.name.as_deref() {
        if incoming.trim().is_ascii() && !current_name.is_ascii() {
            tracing::warn!(
                "LLM 返回的 name 为纯 ASCII，拒绝覆盖现有中文名: {:?}（保留 {:?}）",
                meta.name,
                current_name
            );
            meta.name = None;
        }
    }
}

/// 将 LLM 返回的元数据合并到手账条目（只更新非空字段，保留已有数据）
fn apply_meta_to_review(updated: &mut Review, meta: &LlmGameMeta) {
    let mut name_changed = false;
    if let Some(name) = &meta.name {
        let trimmed = name.trim().to_string();
        if !trimmed.is_empty() && trimmed != updated.name {
            tracing::info!("LLM 纠正手账名称: '{}' -> '{}'", updated.name, trimmed);
            updated.name = trimmed;
            name_changed = true;
        }
    }
    // 名称**真的被本次调用改过**才做语言归位（纯英文名自动补 name_en；中文名清掉对不上的旧英文名）。
    // 「只在改过时执行」是必须的：update_review_meta 支持**部分提交**（前端 saveNameEn 只传
    // name_en、saveName 只传 name），若无条件跑归位，一次"只改英文名"的调用会因为
    // 「现名恰好是中文」而先把用户已有的英文名清掉 —— 虽然紧接着的 name_en 分支会填回来，
    // 但那是巧合而非契约；一旦将来出现别的部分提交入口就会静默丢数据。
    // 位置必须早于下面的 name_en 分支：用户显式清除英文名（Some("")）要能盖掉这里的自动填充。
    if name_changed {
        normalize_review_name(updated);
    }
    // name_en 特殊语义：Some("") 表示显式清除（手动编辑场景），其余非空才更新。
    // 语言闸门：name_en 的契约是「官方英文名」，含非 ASCII 字符（中文/日文等）一律拒收 ——
    // LLM 有时偷懒把中文名原样复制到 name_en（如 name_name 都填「空洞骑士」），
    // 一旦落库就会让卡片「英文名优先展示」退化成显示中文（2026-10-05 修复）。
    if let Some(name_en) = &meta.name_en {
        let trimmed = name_en.trim().to_string();
        if trimmed.is_empty() {
            updated.name_en = None;
        } else if !trimmed.is_ascii() {
            tracing::warn!(
                "LLM 返回的 name_en 含非 ASCII 字符，已丢弃: '{}'（保留原值 {:?}）",
                trimmed,
                updated.name_en
            );
        } else if trimmed != updated.name_en.as_deref().unwrap_or("") {
            tracing::info!("更新手账英文名: '{:?}' -> '{}'", updated.name_en, trimmed);
            updated.name_en = Some(trimmed);
        }
    }
    if let Some(desc) = &meta.description {
        if !desc.is_empty() {
            updated.description = Some(desc.clone());
        }
    }
    if let Some(dev) = &meta.developer {
        if !dev.is_empty() {
            updated.developer = Some(dev.clone());
        }
    }
    if let Some(pub_) = &meta.publisher {
        if !pub_.is_empty() {
            updated.publisher = Some(pub_.clone());
        }
    }
    if let Some(date) = &meta.release_date {
        if !date.is_empty() {
            updated.release_date = Some(date.clone());
        }
    }
    if !meta.genres.is_empty() {
        // 手动填写也走归一化，保证库内类型口径与规范表一致
        updated.genres = crate::core::genres::normalize_genres(&meta.genres);
    }
    if let Some(v) = meta.hltb_main_story {
        updated.hltb_main_story = Some(v);
    }
    if let Some(v) = meta.hltb_main_extra {
        updated.hltb_main_extra = Some(v);
    }
    if let Some(v) = meta.hltb_completionist {
        updated.hltb_completionist = Some(v);
    }
}

/// 刷新手账条目信息：只拉 LLM 元数据（含命名与官方英文名）。
///
/// **不再拉封面**（2026-10-30 按老爷要求改）：封面的三个来源全部交给用户手动挑选
/// （本地图片 / Steam 官方 / SteamGridDB，见 `set_review_cover_from_local` /
/// `set_review_cover_from_steam` / `set_review_cover_from_url`）。
/// 旧实现里点一次「刷新信息」就会用 SteamGridDB 抓一张图并**无条件覆盖**当前封面，
/// 手动挑好的图白挑一次 —— 信息归信息、图归图，两条链路就此分开。
///
/// 写回前重新读库再合并（与 `games.rs::fetch_game_info_llm` 同口径）：LLM 请求动辄
/// 十几秒，期间用户改的评分/评价/截图目录不能被阶段 1 的快照整条写回抹掉。
#[tauri::command]
pub async fn refresh_review_info(
    db: State<'_, Arc<Mutex<Database>>>,
    review_id: String,
) -> Result<Review, String> {
    // 阶段 1：读取条目与设置，立即释放 DB 锁
    let (review, settings) = {
        let db_guard = lock_or_recover(&db);
        let review = db_guard.get_review_by_id(&review_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "手账条目不存在".to_string())?;
        let settings = Settings::load_from_db(&db_guard).map_err(|e| e.to_string())?;
        (review, settings)
    };

    // 与 games.rs::fetch_game_info_llm 同口径：没配 LLM 就明说，不假装"刷新成功"。
    // 添加流程里这条错误被前端静默吞掉（catch 不弹框），不影响入库；
    // 手动点「刷新信息」时则会如实弹出来，用户知道该去设置页。
    if !settings.llm_enabled {
        return Err("未启用 LLM 获取游戏信息，请在设置中配置".to_string());
    }
    if settings.llm_api_key.is_empty() {
        return Err("未配置 LLM API Key，请在设置中填写".to_string());
    }

    // 阶段 2：LLM 拉取元数据（无锁状态下 await）
    let protocol = match settings.llm_protocol.as_str() {
        "anthropic" => LlmProtocol::Anthropic,
        _ => LlmProtocol::Openai,
    };
    let config = LlmConfig {
        enabled: true,
        protocol,
        api_key: settings.llm_api_key.clone(),
        base_url: settings.llm_base_url.clone(),
        model: settings.llm_model.clone(),
    };

    let fetcher = LlmFetcher::new().map_err(|e| format!("创建 LLM 客户端失败: {}", e))?;
    let mut meta = fetcher.fetch_game_meta(&config, &review.name)
        .await
        .map_err(|e| format!("LLM 获取游戏信息失败: {}", e))?;

    // 阶段 3：重取锁，以**库内最新值**为底合并后保存
    let db_guard = lock_or_recover(&db);
    let mut latest = db_guard.get_review_by_id(&review_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "手账条目不存在".to_string())?;
    // 名称语言闸门只作用于 LLM 路径（手动改名走 update_review_meta，用户说了算），
    // 故在合入之前先把越界的 name 摘掉
    drop_english_name_for_chinese_entry(&mut meta, &latest.name);
    apply_meta_to_review(&mut latest, &meta);
    db_guard.update_review(&latest).map_err(|e| e.to_string())?;
    Ok(latest)
}

/// 手动更新手账条目元数据（与 LLM 字段一致，含改名）
#[tauri::command(async)]
pub fn update_review_meta(
    db: State<'_, Arc<Mutex<Database>>>,
    review_id: String,
    meta: LlmGameMeta,
) -> Result<Review, String> {
    let db = lock_or_recover(&db);

    let mut review = db.get_review_by_id(&review_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "手账条目不存在".to_string())?;

    apply_meta_to_review(&mut review, &meta);

    db.update_review(&review).map_err(|e| e.to_string())?;
    Ok(review)
}

/// 设置评分（0-10 分制，null 表示清除评分）
#[tauri::command(async)]
pub fn set_review_rating(
    db: State<'_, Arc<Mutex<Database>>>,
    review_id: String,
    rating: Option<u32>,
) -> Result<Review, String> {
    if let Some(v) = rating {
        if v > 10 {
            return Err("评分必须在 0-10 之间".to_string());
        }
    }

    let db = lock_or_recover(&db);
    let mut review = db.get_review_by_id(&review_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "手账条目不存在".to_string())?;

    review.rating = rating;
    review.updated_at = Some(chrono::Utc::now().to_rfc3339());
    db.update_review(&review).map_err(|e| e.to_string())?;
    Ok(review)
}

/// 设置我的评价（null 表示清除）
#[tauri::command(async)]
pub fn set_review_review(
    db: State<'_, Arc<Mutex<Database>>>,
    review_id: String,
    review_text: Option<String>,
) -> Result<Review, String> {
    let db = lock_or_recover(&db);
    let mut review = db.get_review_by_id(&review_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "手账条目不存在".to_string())?;

    review.review = review_text.map(|t| {
        let trimmed = t.trim().to_string();
        if trimmed.is_empty() { String::new() } else { trimmed }
    });
    review.updated_at = Some(chrono::Utc::now().to_rfc3339());
    db.update_review(&review).map_err(|e| e.to_string())?;
    Ok(review)
}

/// 设置手账条目的截图目录（相对截图根目录的单层文件夹名；null/空串表示清除，回到自动推断）
///
/// 手账自持该字段后，删不删游戏、改不改名都不再影响手账的截图库（2026-09-12）。
#[tauri::command(async)]
pub fn set_review_screenshot_dir(
    db: State<'_, Arc<Mutex<Database>>>,
    review_id: String,
    dir: Option<String>,
) -> Result<Review, String> {
    let db = lock_or_recover(&db);
    let mut review = db.get_review_by_id(&review_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "手账条目不存在".to_string())?;

    review.screenshot_dir = match dir.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(raw) => Some(
            crate::core::screenshot::sanitize_dir_name(raw)
                .ok_or_else(|| "截图目录名不合法：只接受截图根目录下的单层文件夹名".to_string())?,
        ),
    };
    review.updated_at = Some(chrono::Utc::now().to_rfc3339());
    db.update_review(&review).map_err(|e| e.to_string())?;
    Ok(review)
}

/// 删除手账条目
///
/// `keep_cover`：默认保留——封面挪进 covers/archive 留档；`false` 则连图片一并删除
/// （若该图与游戏封面共享则在引用计数归零前不会真删）。前端删除前会询问用户。
#[tauri::command(async)]
pub fn delete_review(
    db: State<'_, Arc<Mutex<Database>>>,
    review_id: String,
    keep_cover: Option<bool>,
) -> Result<(), String> {
    let db = lock_or_recover(&db);
    let keep_cover = keep_cover.unwrap_or(true);

    if keep_cover {
        if let Err(e) = cover_store::archive(&db, OWNER_REVIEW, &review_id) {
            tracing::warn!("归档手账封面失败（不影响删除）: {}", e);
        }
    }
    db.delete_review(&review_id).map_err(|e| e.to_string())?;
    if !keep_cover {
        if let Err(e) = cover_store::purge(&db, OWNER_REVIEW, &review_id) {
            tracing::warn!("删除手账封面失败（不影响删除）: {}", e);
        }
    }
    Ok(())
}

/// 获取手账条目的可选封面列表（封面选择器用）
#[tauri::command]
pub async fn fetch_review_cover_options(
    db: State<'_, Arc<Mutex<Database>>>,
    review_id: String,
) -> Result<Vec<CoverOption>, String> {
    let (review, api_key) = {
        let db_guard = lock_or_recover(&db);
        let review = db_guard.get_review_by_id(&review_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "手账条目不存在".to_string())?;
        let api_key = db_guard.get_setting("steamgriddb_api_key")
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        (review, api_key)
    };

    if api_key.is_empty() {
        return Err("未配置 SteamGridDB API Key，请在设置中填写".to_string());
    }

    // SteamGridDB 只认官方英文名：原名含非 ASCII 字符时优先用英文名搜索
    let search_name = if !review.name.is_ascii() {
        review.name_en.as_deref().unwrap_or(&review.name)
    } else {
        &review.name
    };

    let cache_dir = utils::path::get_app_data_dir().join("covers");
    let fetcher = CoverFetcher::new(cache_dir, api_key).map_err(|e| e.to_string())?;

    // 手账条目没有平台信息（也无安装目录），故不给 Steam 官方图，只走 SteamGridDB
    fetcher.fetch_cover_options(search_name, None, None)
        .await
        .map_err(|e| e.to_string())
}

/// 把手账封面收进封面库（下载来源无关的统一入库口径）
///
/// 「本地选图 / Steam 官方 / 从 URL 设封面」三条路共用：差别只在图片从哪来，
/// 入库规则（转码、缩略图、sha256 去重、引用计数、清理中间文件）必须完全一致。
/// 与 `games.rs::ingest_downloaded_cover` 对称。
///
/// 拆两段的目的同 `set_game_cover`：解码/编码是数百毫秒的重活，放**无锁段**做，
/// 只有写索引这一步短暂持锁，避免一张大图把其它 DB 命令全堵住。
fn ingest_review_cover(
    db: &State<'_, Arc<Mutex<Database>>>,
    review_id: &str,
    actual_path: &std::path::Path,
) -> Result<(), String> {
    let prepared = cover_store::prepare_path(OWNER_REVIEW, review_id, actual_path)
        .map_err(|e| e.to_string())?;
    let db_guard = lock_or_recover(db);
    // 【2026-11-08】下载/解码可能耗时数十秒，期间条目可能已被删除。
    // 必须与写索引共用同一把锁复查主体存在，否则会留下一条 owner_id
    // 已不存在的封面索引：它自己就是引用，sha256 计数永远 ≥1，文件再也删不掉。
    db_guard
        .get_review_by_id(review_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "手账条目不存在".to_string())?;
    let set = cover_store::commit_prepared(&db_guard, OWNER_REVIEW, review_id, prepared)
        .map_err(|e| e.to_string())?;
    // 中间文件入库成功后清理（共享/原地替换的保护规则在函数内）
    let _ = cover_store::cleanup_ingest_source(&db_guard, actual_path, &set, true);
    Ok(())
}

/// 设置手账封面：手动选择本地图片
///
/// 与 `games.rs::set_game_cover` 同口径：只把图片内容收进封面库，
/// **用户选的原文件不动**（故不调 `cleanup_ingest_source`）。
#[tauri::command(async)]
pub fn set_review_cover_from_local(
    db: State<'_, Arc<Mutex<Database>>>,
    review_id: String,
    cover_path: String,
) -> Result<(), String> {
    let src = std::path::Path::new(&cover_path);
    if !src.exists() {
        return Err("选择的图片文件不存在".to_string());
    }

    // 先校验条目存在（同 set_game_cover）：否则会给已经删掉的 review_id 写进一条
    // 封面索引 + 文件，而那条索引没有任何界面能读到、也没有清理逻辑会碰它 ——
    // 它自己就是那条引用，引用计数永远 ≥1，文件永远删不掉。
    {
        let db_guard = lock_or_recover(&db);
        db_guard.get_review_by_id(&review_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "手账条目不存在".to_string())?;
    }

    let prepared = cover_store::prepare_path(OWNER_REVIEW, &review_id, src)
        .map_err(|e| e.to_string())?;
    let db = lock_or_recover(&db);
    // 【2026-11-08 复审】解码/编码可能耗数百毫秒，期间条目可能被删。
    // 与 `ingest_review_cover` 同口径，在写索引的同一把锁里复查一次，
    // 否则会留下自引用、永远删不掉的孤儿封面索引。
    db.get_review_by_id(&review_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "手账条目不存在".to_string())?;
    cover_store::commit_prepared(&db, OWNER_REVIEW, &review_id, prepared)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// 使用 Steam 官方封面（手账条目没有平台/安装目录信息，appid 靠两条来源认定）
///
/// 1. 先在**游戏库**里按名字认领（`name`/`name_en` 2×2 交叉，与手账游玩时长同口径）：
///    命中就用那条目的 appid —— 零网络、最准（Steam 平台条目直取 `platform_id`，
///    本地 repack 从安装目录的 appid 线索文件反查）；
/// 2. 游戏库里没有，再打 Steam 商店搜索接口按名字查 appid（无需 API Key）。
///
/// 认不到就给一句明确的错误，绝不"随便贴一张"。
#[tauri::command]
pub async fn set_review_cover_from_steam(
    db: State<'_, Arc<Mutex<Database>>>,
    review_id: String,
) -> Result<(), String> {
    // 阶段 1：读取条目 / 名字认领游戏库 / 取设置，立即释放 DB 锁
    let (search_name, local_url, api_key) = {
        let db_guard = lock_or_recover(&db);
        let review = db_guard.get_review_by_id(&review_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "手账条目不存在".to_string())?;

        // Steam（商店搜索与图床）只认官方英文名：原名含非 ASCII 时优先用英文名
        let search_name = if !review.name.is_ascii() {
            review.name_en.clone().unwrap_or_else(|| review.name.clone())
        } else {
            review.name.clone()
        };

        // 先认领游戏库：两个名字都拿去撞 games.name / games.name_en
        let keys = vec![review.name.clone(), search_name.clone()];
        let local_url = db_guard
            .find_game_by_any_name(&keys)
            .map_err(|e| e.to_string())?
            .and_then(|game| crate::core::cover_fetcher::steam_cover_url_for(&game));

        let api_key = db_guard.get_setting("steamgriddb_api_key")
            .map_err(|e| e.to_string())?
            .unwrap_or_default();

        (search_name, local_url, api_key)
    };

    let covers_dir = utils::path::get_covers_dir();
    utils::path::ensure_dir_exists(&covers_dir).map_err(|e| e.to_string())?;
    let save_path = covers_dir.join(format!(".{}.steam", review_id));

    // 下载客户端只为复用它的代理/超时配置；Steam 图床本身不需要 API Key
    let fetcher = CoverFetcher::new(covers_dir.clone(), api_key).map_err(|e| e.to_string())?;

    // 阶段 2：确定封面地址 —— 游戏库认得就用它，否则打商店搜索（无锁状态下 await）
    let steam_url = match local_url {
        Some(url) => {
            tracing::info!("手账《{}》按名认领到游戏库条目，直取 Steam 官方封面", search_name);
            url
        }
        None => fetcher
            .steam_cover_url_by_name(&search_name)
            .await
            .map_err(|e| format!("未能确定 Steam AppID：{}", e))?,
    };

    // 阶段 3：下载 → 入库（入库在无锁段做图像重活，只有写索引时短锁）
    let actual_path = fetcher.download_from_url(&steam_url, &save_path)
        .await
        .map_err(|e| format!("下载 Steam 官方封面失败: {}", e))?;
    ingest_review_cover(&db, &review_id, &actual_path)
}

/// 从 URL 下载封面并设置为手账条目封面
#[tauri::command]
pub async fn set_review_cover_from_url(
    db: State<'_, Arc<Mutex<Database>>>,
    review_id: String,
    url: String,
) -> Result<(), String> {
    let api_key = {
        let db_guard = lock_or_recover(&db);
        let _review = db_guard.get_review_by_id(&review_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "手账条目不存在".to_string())?;
        db_guard.get_setting("steamgriddb_api_key")
            .map_err(|e| e.to_string())?
            .unwrap_or_default()
    };

    let covers_dir = utils::path::get_covers_dir();
    utils::path::ensure_dir_exists(&covers_dir).map_err(|e| e.to_string())?;
    let save_path = covers_dir.join(format!(".{}.picked", review_id));

    let fetcher = CoverFetcher::new(covers_dir.clone(), api_key).map_err(|e| e.to_string())?;
    let actual_path = fetcher.download_from_url(&url, &save_path)
        .await
        .map_err(|e| format!("下载封面失败: {}", e))?;

    ingest_review_cover(&db, &review_id, &actual_path)
}

/// 由游戏库条目构造手账条目：复用元数据 + 语言归位 + 补官方英文名。
///
/// 抽成独立函数是为了能直接单测 —— `import_review_from_game` 包着 `State` 壳子，
/// 单测里没法调用（同 `games.rs::rename_game_inner` 的处理方式）。
fn review_from_game(game: &crate::models::Game) -> Review {
    let mut review = Review::new(game.name.clone());
    review.description = game.description.clone();
    review.developer = game.developer.clone();
    review.publisher = game.publisher.clone();
    review.release_date = game.release_date.clone();
    review.genres = game.genres.clone();
    review.hltb_main_story = game.hltb_main_story;
    review.hltb_main_extra = game.hltb_main_extra;
    review.hltb_completionist = game.hltb_completionist;
    // 游戏库里的名字可能是纯英文原名（如 "Mafia: The Old Country"），
    // 直接存进 name 会让它被当成中文名，故落库前做一次语言归位。
    normalize_review_name(&mut review);
    // 补游戏库里的官方英文名（2026-10-06）：中文题名的游戏只有在导入这条路径上才拿得到
    // 英文名，否则封面检索只能拿中文名去 SteamGridDB 撞 0 命中 —— 而"复用游戏库的元数据"
    // 正是导入手账的核心价值之一。
    // 顺序必须在 normalize **之后**：normalize 的 else 分支会「名字含非 ASCII → 清空 name_en」，
    // 先补就会被它抹掉。带 is_ascii 过滤，避免把游戏库里的历史脏值一起搬进手账。
    if review.name_en.is_none() {
        if let Some(en) = game.name_en.as_deref().map(str::trim) {
            if !en.is_empty() && en.is_ascii() {
                review.name_en = Some(en.to_string());
            }
        }
    }
    review
}

/// 从游戏库导入游戏到游戏手账（复用元数据与封面，评分/评价为空）
#[tauri::command(async)]
pub fn import_review_from_game(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
) -> Result<Review, String> {
    let db = lock_or_recover(&db);

    let game = db.get_game_by_id(&game_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "游戏不存在".to_string())?;

    // 同名去重
    if db.find_review_by_name(&game.name).map_err(|e| e.to_string())?.is_some() {
        return Err(format!("《{}》已存在于手账中", game.name));
    }

    let mut review = review_from_game(&game);

    // 截图目录：导入时若能确定就固化进手账（此后从游戏库删除游戏，手账的截图库照旧可用）。
    // 目录不存在或无图则留空，继续走运行时自动推断。
    if let Some(exe) = game.exe_name.as_deref() {
        if let Ok(settings) = Settings::load_from_db(&db) {
            let dir = crate::core::screenshot::screenshot_dir_for_process(&settings.screenshot_dir, exe);
            if crate::core::screenshot::count_images(&dir) > 0 {
                review.screenshot_dir = Some(crate::core::screenshot::process_stem(exe));
            }
        }
    }

    db.insert_review(&review).map_err(|e| e.to_string())?;

    // 封面：与游戏**共享同一物理文件**（covers 索引按 sha256 记两条引用），
    // 不再像旧实现那样把游戏的路径裸抄过来——那样删掉游戏会连手账封面一起删。
    // 顺序上必须先 insert 再挂封面：sync_owner_paths 是 UPDATE，行不存在就白写。
    let shared = cover_store::share_from(&db, OWNER_GAME, &game_id, OWNER_REVIEW, &review.id, STATE_ACTIVE)
        .map_err(|e| e.to_string())?;
    if !shared {
        // 游戏侧还没进索引（迁移未跑完 / 手动放的文件）→ 用旧字段兜底入库
        if let Some(cover) = game.cover_local.as_ref().or(game.cover_url.as_ref()) {
            let src = std::path::Path::new(cover);
            if src.exists() {
                if let Err(e) = cover_store::ingest_path(&db, OWNER_REVIEW, &review.id, src, false) {
                    tracing::warn!("导入手账时收编封面失败: {}", e);
                }
            }
        }
    }

    // 回读封面字段（share_from / ingest 已写库），保证返回值与库内一致
    if let Ok(Some(saved)) = db.get_review_by_id(&review.id) {
        review.cover_local = saved.cover_local;
        review.cover_url = saved.cover_url;
    }
    Ok(review)
}

/// 游玩时长按名匹配（手账 × 游戏库）。
///
/// 手账与游戏库完全隔离（无外键），沿用「按名字认领」的既有口径
/// （同 `find_review_id_by_game_name` / 墓碑认领）：手账的 `name`/`name_en`
/// 与游戏的 `name`/`name_en` 2×2 交叉比对，trim + 小写归一后任一相等即视为
/// 同一款游戏。多条命中（同名重复入库等）取游玩时长最大的一条。
///
/// 空名字不参与匹配——已删游戏的孤儿历史（无墓碑）名字还原为空串，
/// 绝不能凭「两边都空」误命中到任意手账条目。
fn pick_play_time(
    review_name: &str,
    review_name_en: Option<&str>,
    candidates: &[PlayTimeCandidate],
) -> Option<ReviewPlayTime> {
    let keys: Vec<String> = [Some(review_name), review_name_en]
        .into_iter()
        .flatten()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if keys.is_empty() {
        return None;
    }

    candidates
        .iter()
        .filter(|c| {
            [Some(c.name.as_str()), c.name_en.as_deref()]
                .into_iter()
                .flatten()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .any(|k| keys.iter().any(|x| x == &k))
        })
        .max_by_key(|c| c.play_time_seconds)
        .map(|c| ReviewPlayTime {
            play_time_seconds: c.play_time_seconds,
            last_played: c.last_played.clone(),
        })
}

/// 手账「我的游玩时长」批量查询：按名字把手账条目对上游戏库（含已删游戏的
/// play_stats_daily 历史），返回 review_id → 时长/最近游玩。无历史的条目不在 map 中。
#[tauri::command(async)]
pub fn get_review_play_times(
    db: State<'_, Arc<Mutex<Database>>>,
) -> Result<HashMap<String, ReviewPlayTime>, String> {
    let db = lock_or_recover(&db);
    // 只取 id/name/name_en：认领只认名字，没必要把评价正文整表搬一遍
    let reviews = db.get_review_names().map_err(|e| e.to_string())?;
    let candidates = db.get_play_time_candidates().map_err(|e| e.to_string())?;

    let mut map = HashMap::new();
    for (id, name, name_en) in reviews {
        if let Some(pt) = pick_play_time(&name, name_en.as_deref(), &candidates) {
            map.insert(id, pt);
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 纯英文原名自动补英文名() {
        let mut r = Review::new("Mafia: The Old Country".to_string());
        normalize_review_name(&mut r);
        assert_eq!(r.name, "Mafia: The Old Country");
        assert_eq!(r.name_en.as_deref(), Some("Mafia: The Old Country"));
    }

    #[test]
    fn 中文原名不填英文名() {
        let mut r = Review::new("空洞骑士".to_string());
        normalize_review_name(&mut r);
        assert_eq!(r.name, "空洞骑士");
        assert_eq!(r.name_en, None);
    }

    #[test]
    fn 已有英文名不被覆盖() {
        let mut r = Review::new("Hollow Knight".to_string());
        r.name_en = Some("Hollow Knight: Silksong".to_string());
        normalize_review_name(&mut r);
        assert_eq!(r.name_en.as_deref(), Some("Hollow Knight: Silksong"));
    }

    #[test]
    fn 名称两端空白被裁剪() {
        let mut r = Review::new("  Celeste  ".to_string());
        normalize_review_name(&mut r);
        assert_eq!(r.name, "Celeste");
        assert_eq!(r.name_en.as_deref(), Some("Celeste"));
    }

    #[test]
    fn 空名称不动() {
        let mut r = Review::new("   ".to_string());
        normalize_review_name(&mut r);
        assert_eq!(r.name, "   ");
        assert_eq!(r.name_en, None);
    }

    /// LLM 名称语言闸门（2026-10-09）：纯 ASCII 的 name 不得覆盖手账已有中文名，
    /// 否则「刷新信息」会把中文名顶成英文，随后 normalize 再把 name_en 补成同一个英文串。
    /// 现名是英文时不拦（英文→英文纠错、英文→中文都放行）。
    #[test]
    fn llm纯英文名不得覆盖手账中文名() {
        // ① 现名中文 + LLM 回英文 → name 被摘掉
        let mut meta = LlmGameMeta {
            name: Some("  God of War  ".to_string()),
            ..Default::default()
        };
        drop_english_name_for_chinese_entry(&mut meta, "战神");
        assert_eq!(meta.name, None, "英文 name 必须被摘掉");

        // ② 现名英文 → 不拦（修正是合法的）
        let mut meta2 = LlmGameMeta {
            name: Some("Gears of War: Reloaded".to_string()),
            ..Default::default()
        };
        drop_english_name_for_chinese_entry(&mut meta2, "Gears of War Reloaded");
        assert_eq!(meta2.name.as_deref(), Some("Gears of War: Reloaded"));

        // ③ 中文名覆盖中文名 → 放行
        let mut meta3 = LlmGameMeta {
            name: Some("只狼：影逝二度".to_string()),
            ..Default::default()
        };
        drop_english_name_for_chinese_entry(&mut meta3, "只狼");
        assert_eq!(meta3.name.as_deref(), Some("只狼：影逝二度"));

        // ④ name 为 null 时不动
        let mut meta4 = LlmGameMeta {
            name: None,
            ..Default::default()
        };
        drop_english_name_for_chinese_entry(&mut meta4, "战神");
        assert_eq!(meta4.name, None);
    }

    /// LLM 把中文名原样塞进 name_en 时必须拒收，且不得覆盖已有英文名（2026-10-05 修复）
    #[test]
    fn llm返回的中文英文名被拒收() {
        let mut r = Review::new("空洞骑士".to_string());
        r.name_en = Some("Hollow Knight".to_string());
        let meta = LlmGameMeta {
            name: Some("空洞骑士".to_string()),
            // 模型偷懒：把中文名复制进了 name_en
            name_en: Some("空洞骑士".to_string()),
            description: None,
            developer: None,
            publisher: None,
            release_date: None,
            genres: vec![],
            hltb_main_story: None,
            hltb_main_extra: None,
            hltb_completionist: None,
            save_paths: vec![],
        };
        apply_meta_to_review(&mut r, &meta);
        // 中文 name_en 被丢弃，原英文名安然无恙
        assert_eq!(r.name_en.as_deref(), Some("Hollow Knight"));
    }

    /// name_en 为空时，中文 name_en 被拒收后应保持 None（不落中文脏数据）
    #[test]
    fn llm中文英文名不落库() {
        let mut r = Review::new("空洞骑士".to_string());
        let meta = LlmGameMeta {
            name: Some("空洞骑士".to_string()),
            name_en: Some("空洞骑士".to_string()),
            description: None,
            developer: None,
            publisher: None,
            release_date: None,
            genres: vec![],
            hltb_main_story: None,
            hltb_main_extra: None,
            hltb_completionist: None,
            save_paths: vec![],
        };
        apply_meta_to_review(&mut r, &meta);
        assert_eq!(r.name_en, None);
        assert_eq!(r.name, "空洞骑士");
    }

    /// 合法英文名（含标点/数字）正常写入
    #[test]
    fn llm合法英文名正常写入() {
        let mut r = Review::new("四海兄弟：故乡".to_string());
        let meta = LlmGameMeta {
            name: Some("四海兄弟：故乡".to_string()),
            name_en: Some("Mafia: The Old Country".to_string()),
            description: None,
            developer: None,
            publisher: None,
            release_date: None,
            genres: vec![],
            hltb_main_story: None,
            hltb_main_extra: None,
            hltb_completionist: None,
            save_paths: vec![],
        };
        apply_meta_to_review(&mut r, &meta);
        assert_eq!(r.name_en.as_deref(), Some("Mafia: The Old Country"));
    }

    /// 改名成中文必须清空旧英文名（2026-10-06 修：此前 normalize 缺 else 分支）。
    /// 与 games.rs::rename_game_normalizes_name_en 是同一语义的对称防线。
    #[test]
    fn 改名成中文清空旧英文名() {
        let mut r = Review::new("Hollow Knight".to_string());
        r.name_en = Some("Hollow Knight".to_string());
        let meta = LlmGameMeta {
            name: Some("空洞骑士：丝之歌".to_string()),
            ..Default::default()
        };
        apply_meta_to_review(&mut r, &meta);
        assert_eq!(r.name, "空洞骑士：丝之歌");
        assert_eq!(r.name_en, None, "改成中文名必须清空 name_en（防错配展示与错检索）");
    }

    /// 部分提交保护：只改英文名时，不能因为"现名恰好是中文"而先被归位清掉。
    /// 这条守着 apply_meta_to_review 里 `if name_changed` 那道闸门。
    #[test]
    fn 只改英文名不被归位误清() {
        let mut r = Review::new("空洞骑士".to_string());
        r.name_en = Some("Hollow Knight".to_string());
        // 前端 saveNameEn 的提交形态：只带 name_en，name 为 None
        let meta = LlmGameMeta {
            name_en: Some("Hollow Knight: Silksong".to_string()),
            ..Default::default()
        };
        apply_meta_to_review(&mut r, &meta);
        assert_eq!(r.name_en.as_deref(), Some("Hollow Knight: Silksong"));
    }

    /// 从游戏库导入手账：中文题名 + 游戏库有官方英文名 → 英文名必须带过来
    /// （2026-10-06 修：此前只搬 name，英文名在此丢失，封面检索退回中文名 0 命中）
    #[test]
    fn 导入手账带上游戏库英文名() {
        let mut g = crate::models::Game::new("空洞骑士".to_string());
        g.name_en = Some("Hollow Knight".to_string());
        let r = review_from_game(&g);
        assert_eq!(r.name, "空洞骑士");
        assert_eq!(r.name_en.as_deref(), Some("Hollow Knight"));
    }

    /// 导入时不得把游戏库里的中文脏英文名搬进手账（语言闸门同样适用于这条路径）
    #[test]
    fn 导入不搬中文脏英文名() {
        let mut g = crate::models::Game::new("空洞骑士".to_string());
        g.name_en = Some("空洞骑士".to_string());
        let r = review_from_game(&g);
        assert_eq!(r.name_en, None);
    }

    /// 导入纯英文题名：normalize 已把 name 补成 name_en，不该被后面的补全逻辑改动
    #[test]
    fn 导入纯英文题名沿用原名作英文名() {
        let mut g = crate::models::Game::new("Hollow Knight".to_string());
        g.name_en = Some("Hollow Knight: Silksong".to_string());
        let r = review_from_game(&g);
        assert_eq!(r.name, "Hollow Knight");
        // normalize 先按 ASCII 名填了同名，补全分支不覆盖（name_en 已非 None）
        assert_eq!(r.name_en.as_deref(), Some("Hollow Knight"));
    }

    // ==================== pick_play_time（手账 × 游戏库按名匹配时长） ====================

    fn cand(name: &str, name_en: Option<&str>, secs: u64) -> PlayTimeCandidate {
        PlayTimeCandidate {
            name: name.to_string(),
            name_en: name_en.map(|s| s.to_string()),
            play_time_seconds: secs,
            last_played: None,
        }
    }

    /// 中文名对中文名命中，大小写/首尾空格不敏感
    #[test]
    fn 时长匹配同名命中() {
        let c = vec![cand("空洞骑士", Some("Hollow Knight"), 3600)];
        let hit = pick_play_time("空洞骑士", None, &c).unwrap();
        assert_eq!(hit.play_time_seconds, 3600);

        // 空格与大小写归一（英文名交叉）
        let c2 = vec![cand("  HOLLOW KNIGHT ", None, 7200)];
        let hit2 = pick_play_time("hollow knight", None, &c2).unwrap();
        assert_eq!(hit2.play_time_seconds, 7200);
    }

    /// 2×2 交叉：手账 name_en 能对上游戏 name，手账 name 也能对上游戏 name_en
    #[test]
    fn 时长匹配英文名交叉命中() {
        // 手账只有英文名（name 是中文、name_en 是英文）对上纯英文游戏名
        let c = vec![cand("Hollow Knight", None, 3600)];
        let hit = pick_play_time("空洞骑士", Some("Hollow Knight"), &c).unwrap();
        assert_eq!(hit.play_time_seconds, 3600);

        // 反向：手账 name（英文）对上游戏的 name_en
        let c2 = vec![cand("空洞骑士", Some("Hollow Knight"), 3600)];
        let hit2 = pick_play_time("Hollow Knight", None, &c2).unwrap();
        assert_eq!(hit2.play_time_seconds, 3600);
    }

    /// 同名重复入库（多条命中）取游玩时长最大的一条
    #[test]
    fn 时长匹配多条命中取最大() {
        let c = vec![
            cand("空洞骑士", None, 600),
            cand("空洞骑士", Some("Hollow Knight"), 9999),
            cand("Hollow Knight", None, 3000),
        ];
        let hit = pick_play_time("空洞骑士", Some("Hollow Knight"), &c).unwrap();
        assert_eq!(hit.play_time_seconds, 9999);
    }

    /// 没有游玩历史/完全对不上 → 不返回（详情页这一栏静默不显示）
    #[test]
    fn 时长匹配无命中返回空() {
        let c = vec![cand("空洞骑士", Some("Hollow Knight"), 3600)];
        assert!(pick_play_time("只狼", Some("Sekiro"), &c).is_none());
        assert!(pick_play_time("", None, &c).is_none());
    }

    /// 空名字不得互相「匹配」——已删游戏的孤儿历史（无墓碑）名字还原为空串，
    /// 若空串算相等，任何手账条目都会被误挂上孤儿时长
    #[test]
    fn 时长匹配空名不误命中() {
        let c = vec![
            cand("", None, 88888),
            cand("", Some(""), 99999),
        ];
        assert!(pick_play_time("空洞骑士", None, &c).is_none());
        assert!(pick_play_time("", None, &c).is_none());
        assert!(pick_play_time("", Some(""), &c).is_none());
    }
}
