use tauri::State;
use std::sync::{Arc, Mutex};
use crate::core::Database;
use crate::core::cover_fetcher::CoverFetcher;
use crate::core::llm_fetcher::{LlmFetcher, LlmConfig, LlmProtocol, LlmGameMeta};
use crate::models::*;
use crate::models::settings::Settings;
use crate::utils;
use super::lock_or_recover;

/// 获取手账条目列表
#[tauri::command]
pub fn get_reviews(
    db: State<'_, Arc<Mutex<Database>>>,
    filter: Option<ReviewFilter>,
) -> Result<Vec<Review>, String> {
    let db = lock_or_recover(&db);
    let filter = filter.unwrap_or_default();
    db.get_reviews(&filter).map_err(|e| e.to_string())
}

/// 获取单个手账条目详情
#[tauri::command]
pub fn get_review_detail(
    db: State<'_, Arc<Mutex<Database>>>,
    review_id: String,
) -> Result<Option<Review>, String> {
    let db = lock_or_recover(&db);
    db.get_review_by_id(&review_id).map_err(|e| e.to_string())
}

/// 添加手账条目（只需名字，元数据/封面由 refresh_review_info 后续填充）
#[tauri::command]
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

    let review = Review::new(trimmed);
    db.insert_review(&review).map_err(|e| e.to_string())?;
    Ok(review)
}

/// 将 LLM 返回的元数据合并到手账条目（只更新非空字段，保留已有数据）
fn apply_meta_to_review(updated: &mut Review, meta: &LlmGameMeta) {
    if let Some(name) = &meta.name {
        let trimmed = name.trim().to_string();
        if !trimmed.is_empty() && trimmed != updated.name {
            tracing::info!("LLM 纠正手账名称: '{}' -> '{}'", updated.name, trimmed);
            updated.name = trimmed;
        }
    }
    // name_en 特殊语义：Some("") 表示显式清除（手动编辑场景），其余非空才更新
    if let Some(name_en) = &meta.name_en {
        let trimmed = name_en.trim().to_string();
        if trimmed.is_empty() {
            updated.name_en = None;
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
        updated.genres = meta.genres.clone();
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

/// 刷新手账条目信息：LLM 拉元数据（若启用）+ SteamGridDB 拉封面（若有 Key）
#[tauri::command]
pub async fn refresh_review_info(
    db: State<'_, Arc<Mutex<Database>>>,
    review_id: String,
) -> Result<Review, String> {
    // 阶段 1：读取条目与设置，立即释放 DB 锁
    let (mut review, settings) = {
        let db_guard = lock_or_recover(&db);
        let review = db_guard.get_review_by_id(&review_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "手账条目不存在".to_string())?;
        let settings = Settings::load_from_db(&db_guard).map_err(|e| e.to_string())?;
        (review, settings)
    };

    // 阶段 2：LLM 拉取元数据（无锁状态下 await）
    if settings.llm_enabled && !settings.llm_api_key.is_empty() {
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
        let meta = fetcher.fetch_game_meta(&config, &review.name)
            .await
            .map_err(|e| format!("LLM 获取游戏信息失败: {}", e))?;
        apply_meta_to_review(&mut review, &meta);
    }

    // 阶段 3：SteamGridDB 拉封面（无锁状态下 await）
    // SteamGridDB 只认官方英文名：原名含非 ASCII 字符时改用英文名搜索（LLM 失败则退回原名）
    if !settings.steamgriddb_api_key.is_empty() {
        let search_name = if !review.name.is_ascii() {
            review.name_en.as_deref().unwrap_or(&review.name)
        } else {
            &review.name
        };
        let cache_dir = utils::path::get_app_data_dir().join("covers");
        let fetcher = CoverFetcher::new(cache_dir, settings.steamgriddb_api_key.clone())
            .map_err(|e| e.to_string())?;
        match fetcher.fetch_cover_for_id(&review.id, search_name).await {
            Ok(Some(path)) => {
                review.cover_local = Some(path);
                review.cover_url = review.cover_local.clone();
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("手账封面获取失败 {}（搜索词: {}）: {}", review.name, search_name, e),
        }
    }

    // 阶段 4：重新取锁保存
    let db_guard = lock_or_recover(&db);
    // 封面必须走专用函数落库：update_review 有意不含封面字段（见其注释），
    // 若只调 update_review，这里拉到的封面只存在于返回值中——前端当时显示正常，
    // 但 DB 里仍是 NULL，任何一次"读-改-写"（切状态/评分）都会让封面在界面上消失。
    if let Some(cover) = review.cover_local.as_ref() {
        db_guard.update_review_cover(&review.id, cover)
            .map_err(|e| e.to_string())?;
    }
    db_guard.update_review(&review).map_err(|e| e.to_string())?;
    Ok(review)
}

/// 手动更新手账条目元数据（与 LLM 字段一致，含改名）
#[tauri::command]
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
#[tauri::command]
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
#[tauri::command]
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

/// 设置手账状态: "wishlist" | "playing" | "completed" | "abandoned"
#[tauri::command]
pub fn set_review_status(
    db: State<'_, Arc<Mutex<Database>>>,
    review_id: String,
    status: String,
) -> Result<Review, String> {
    const VALID_STATUS: [&str; 4] = ["wishlist", "playing", "completed", "abandoned"];
    if !VALID_STATUS.contains(&status.as_str()) {
        return Err(format!("无效的状态: {}", status));
    }

    let db = lock_or_recover(&db);
    let mut review = db.get_review_by_id(&review_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "手账条目不存在".to_string())?;

    review.status = status;
    review.updated_at = Some(chrono::Utc::now().to_rfc3339());
    db.update_review(&review).map_err(|e| e.to_string())?;
    Ok(review)
}

/// 删除手账条目
#[tauri::command]
pub fn delete_review(
    db: State<'_, Arc<Mutex<Database>>>,
    review_id: String,
) -> Result<(), String> {
    let db = lock_or_recover(&db);
    db.delete_review(&review_id).map_err(|e| e.to_string())
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

    fetcher.fetch_cover_options(search_name, None)
        .await
        .map_err(|e| e.to_string())
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
    let save_path = covers_dir.join(format!("{}.jpg", review_id));

    let fetcher = CoverFetcher::new(covers_dir.clone(), api_key).map_err(|e| e.to_string())?;
    let actual_path = fetcher.download_from_url(&url, &save_path)
        .await
        .map_err(|e| format!("下载封面失败: {}", e))?;

    // 清理该条目同 id 的其他扩展名封面（避免残留旧文件）
    for ext in ["jpg", "png", "jpeg", "webp"] {
        let old = covers_dir.join(format!("{}.{}", review_id, ext));
        if old != actual_path {
            let _ = std::fs::remove_file(&old);
        }
    }

    let cover_path = actual_path.to_string_lossy().to_string();
    let db_guard = lock_or_recover(&db);
    db_guard.update_review_cover(&review_id, &cover_path)
        .map_err(|e| e.to_string())
}

/// 从游戏库导入游戏到游戏手账（复用元数据与封面，评分/评价为空）
#[tauri::command]
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

    let mut review = Review::new(game.name.clone());
    // 复用游戏库的封面缓存文件（covers 目录共享，路径直接引用）。
    // cover_local 优先（手动设置的封面只写 cover_local），cover_url 兜底，与 get_all_covers 同口径
    if let Some(cover) = game.cover_local.as_ref().or(game.cover_url.as_ref()) {
        review.cover_local = Some(cover.clone());
        review.cover_url = Some(cover.clone());
    }
    review.description = game.description.clone();
    review.developer = game.developer.clone();
    review.publisher = game.publisher.clone();
    review.release_date = game.release_date.clone();
    review.genres = game.genres.clone();
    review.hltb_main_story = game.hltb_main_story;
    review.hltb_main_extra = game.hltb_main_extra;
    review.hltb_completionist = game.hltb_completionist;

    db.insert_review(&review).map_err(|e| e.to_string())?;
    Ok(review)
}
