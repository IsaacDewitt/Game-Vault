use tauri::{State, Emitter};
use tauri_plugin_opener::OpenerExt;
use std::sync::{Arc, Mutex};
use crate::core::{Database, PlayTimeTracker, GameLauncher};
use crate::core::cover_fetcher::CoverFetcher;
use crate::core::llm_fetcher::{self, LlmConfig, LlmProtocol};
use crate::models::*;
use crate::models::settings::Settings;
use crate::utils;
use crate::utils::constants::COVER_MIN_FILE_SIZE;
use super::lock_or_recover;

/// 获取游戏列表
#[tauri::command]
pub fn get_games(
    db: State<'_, Arc<Mutex<Database>>>,
    filter: Option<GameFilter>,
) -> Result<Vec<Game>, String> {
    let db = lock_or_recover(&db);
    let filter = filter.unwrap_or_default();
    db.get_games(&filter).map_err(|e| e.to_string())
}

/// 获取单个游戏详情
#[tauri::command]
pub fn get_game_detail(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
) -> Result<Option<Game>, String> {
    let db = lock_or_recover(&db);
    db.get_game_by_id(&game_id).map_err(|e| e.to_string())
}

/// 启动游戏
#[tauri::command]
pub fn launch_game(
    db: State<'_, Arc<Mutex<Database>>>,
    tracker: State<'_, Arc<Mutex<PlayTimeTracker>>>,
    game_id: String,
) -> Result<(), String> {
    // 阶段 1：获取游戏数据，然后立即释放 DB 锁
    let game = {
        let db_guard = lock_or_recover(&db);
        let game = db_guard.get_game_by_id(&game_id).map_err(|e| e.to_string())?;
        match game {
            Some(g) => g,
            None => return Err("游戏不存在".to_string()),
        }
    };
    // DB 锁已释放

    // 阶段 2：启动游戏（无锁状态下的 I/O 操作），获取 PID 用于进程树追踪
    let spawned_pid = GameLauncher::launch(&game).map_err(|e| e.to_string())?;

    // 阶段 3：开始追踪时长（独立获取 Tracker 锁）
    if let Some(ref exe_name) = game.exe_name {
        let mut tracker_guard = lock_or_recover(&tracker);
        if let Some(finished_session) = tracker_guard.start_tracking(
            &game_id,
            exe_name,
            game.exe_path.as_deref(),
            Some(spawned_pid),
            game.install_path.as_deref(),
        ) {
            // 如果有旧会话结束，持久化到数据库
            drop(tracker_guard);  // 释放 Tracker 锁再获取 DB 锁
            let db_guard = lock_or_recover(&db);
            if let Err(e) = db_guard.add_play_session(
                &finished_session.game_id,
                &finished_session.start_time,
                finished_session.duration_seconds,
            ) {
                tracing::error!("保存旧游戏会话失败 (game_id: {}): {}", finished_session.game_id, e);
            }
        }
    }

    Ok(())
}

/// 切换收藏状态
#[tauri::command]
pub fn toggle_favorite(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
) -> Result<bool, String> {
    let db = lock_or_recover(&db);
    db.toggle_favorite(&game_id).map_err(|e| e.to_string())
}

/// 删除游戏
#[tauri::command]
pub fn delete_game(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
) -> Result<(), String> {
    // 先获取游戏信息以清理封面文件
    let cover_local = {
        let db_guard = lock_or_recover(&db);
        let game = db_guard.get_game_by_id(&game_id).map_err(|e| e.to_string())?;
        db_guard.delete_game(&game_id).map_err(|e| e.to_string())?;
        game.and_then(|g| g.cover_local)
    };

    // 清理封面缓存文件（所有可能的扩展名，与 set_game_cover 保持一致）
    let covers_dir = utils::path::get_covers_dir();
    for ext in &["jpg", "png", "jpeg", "webp"] {
        let cover_path = covers_dir.join(format!("{}.{}", game_id, ext));
        let _ = std::fs::remove_file(cover_path);
    }

    // 清理 cover_local 指向的文件（仅当在 covers 目录下时）
    if let Some(ref local_path) = cover_local {
        let path = std::path::Path::new(local_path);
        if let Ok(canonical) = path.canonicalize() {
            let covers_canonical = covers_dir.canonicalize().unwrap_or(covers_dir.clone());
            if canonical.starts_with(&covers_canonical) {
                let _ = std::fs::remove_file(&canonical);
            }
        }
    }

    Ok(())
}

/// 手动添加游戏
#[tauri::command]
pub fn add_game_manual(
    db: State<'_, Arc<Mutex<Database>>>,
    name: String,
    exe_path: String,
) -> Result<Game, String> {
    let db = lock_or_recover(&db);

    let mut game = Game::new(name);
    game.exe_path = Some(exe_path.clone());
    game.exe_name = Some(std::path::Path::new(&exe_path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string());
    game.install_path = Some(std::path::Path::new(&exe_path)
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .to_string_lossy()
        .to_string());
    // 从 exe 文件读取版本号
    game.exe_version = utils::path::read_exe_version(&exe_path);

    db.upsert_game(&game).map_err(|e| e.to_string())?;
    Ok(game)
}

/// 启动时批量刷新所有游戏的 exe 版本号
/// 仅对有 exe_path 且版本号为空或 exe 文件已变更的游戏进行更新
#[tauri::command]
pub fn refresh_exe_versions(
    db: State<'_, Arc<Mutex<Database>>>,
) -> Result<u32, String> {
    let db_guard = lock_or_recover(&db);
    let filter = GameFilter::default();
    let games = db_guard.get_games(&filter).map_err(|e| e.to_string())?;

    let mut updated = 0u32;
    for game in games {
        let exe_path = match game.exe_path {
            Some(ref p) => p.clone(),
            None => continue,
        };

        // 读取当前 exe 的版本号
        let new_version = utils::path::read_exe_version(&exe_path);

        // 仅当版本号有变化时才更新数据库
        if new_version != game.exe_version {
            let mut updated_game = game.clone();
            updated_game.exe_version = new_version;
            updated_game.updated_at = Some(chrono::Utc::now().to_rfc3339());
            if let Err(e) = db_guard.update_game(&updated_game) {
                tracing::warn!("更新游戏版本号失败 {}: {}", updated_game.name, e);
            } else {
                updated += 1;
            }
        }
    }

    Ok(updated)
}

/// 设置游戏封面（手动选择本地图片）
/// 将用户选择的图片复制到 covers 目录，再将内部路径存入数据库
#[tauri::command]
pub fn set_game_cover(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
    cover_path: String,
) -> Result<(), String> {
    let src = std::path::Path::new(&cover_path);
    if !src.exists() {
        return Err("选择的图片文件不存在".to_string());
    }

    // 取源文件扩展名，默认 jpg
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("jpg");
    let covers_dir = utils::path::get_covers_dir();
    utils::path::ensure_dir_exists(&covers_dir).map_err(|e| e.to_string())?;
    let dest = covers_dir.join(format!("{}.{}", game_id, ext));

    // 清理旧封面文件（删除 covers 目录下该 game_id 的所有图片）
    for old_ext in &["jpg", "png", "jpeg", "webp"] {
        let old_path = covers_dir.join(format!("{}.{}", game_id, old_ext));
        if old_path != dest {
            let _ = std::fs::remove_file(&old_path);
        }
    }

    std::fs::copy(src, &dest).map_err(|e| format!("复制封面文件失败: {}", e))?;

    let db = lock_or_recover(&db);
    db.update_game_cover(&game_id, &dest.to_string_lossy())
        .map_err(|e| e.to_string())
}

/// 删除游戏封面（清除 cover_url 和 cover_local，同时删除本地文件）
#[tauri::command]
pub fn remove_game_cover(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
) -> Result<(), String> {
    let db_guard = lock_or_recover(&db);

    // 先获取游戏信息，拿到封面文件路径
    let game = db_guard.get_game_by_id(&game_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "游戏不存在".to_string())?;

    // 删除本地封面文件
    if let Some(ref cover_url) = game.cover_url {
        let path = std::path::Path::new(cover_url);
        if path.exists() {
            if let Err(e) = std::fs::remove_file(path) {
                tracing::warn!("删除封面文件失败 {}: {}", cover_url, e);
            }
        }
    }
    if let Some(ref cover_local) = game.cover_local {
        let path = std::path::Path::new(cover_local);
        if path.exists() {
            if let Err(e) = std::fs::remove_file(path) {
                tracing::warn!("删除封面文件失败 {}: {}", cover_local, e);
            }
        }
    }

    // 清除数据库记录
    db_guard.remove_game_cover(&game_id).map_err(|e| e.to_string())
}

/// 获取所有游戏的有效封面路径（供前端通过 asset 协议加载）
#[tauri::command]
pub fn get_all_covers(
    db: State<'_, Arc<Mutex<Database>>>,
) -> Result<std::collections::HashMap<String, String>, String> {
    let db_guard = lock_or_recover(&db);
    let filter = GameFilter::default();
    let games = db_guard.get_games(&filter).map_err(|e| e.to_string())?;

    let mut covers = std::collections::HashMap::new();

    for game in games {
        // 优先使用 cover_local，其次 cover_url
        let cover_path = game.cover_local.as_ref().or(game.cover_url.as_ref());
        if let Some(path_str) = cover_path {
            let path = std::path::Path::new(path_str);
            if path.exists() {
                if let Ok(metadata) = std::fs::metadata(path) {
                    if metadata.len() >= COVER_MIN_FILE_SIZE {
                        covers.insert(game.id.clone(), path_str.clone());
                    }
                }
            }
        }
    }

    Ok(covers)
}

/// 获取缺失封面的游戏封面（异步版本，带进度通知）
#[tauri::command]
pub async fn fetch_missing_covers(
    db: State<'_, Arc<Mutex<Database>>>,
    app_handle: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    // 第一阶段：收集需要获取封面的游戏信息，然后立即释放数据库锁
    let (games_without_cover, api_key) = {
        let db_guard = lock_or_recover(&db);

        let filter = GameFilter::default();
        let games = db_guard.get_games(&filter).map_err(|e| e.to_string())?;
        let games_without_cover: Vec<Game> = games.into_iter()
            .filter(|g| {
                // 条件1: 没有设置任何封面
                if g.cover_url.is_none() && g.cover_local.is_none() {
                    return true;
                }
                // 条件2: cover_url 指向的文件无效（不存在或太小）
                if let Some(ref cover_url) = g.cover_url {
                    let path = std::path::Path::new(cover_url);
                    if !path.exists() {
                        return true;
                    }
                    if let Ok(metadata) = std::fs::metadata(path) {
                        if metadata.len() < COVER_MIN_FILE_SIZE {
                            return true;
                        }
                    }
                }
                // 条件3: cover_local 指向的文件无效
                if let Some(ref cover_local) = g.cover_local {
                    let path = std::path::Path::new(cover_local);
                    if !path.exists() {
                        return true;
                    }
                    if let Ok(metadata) = std::fs::metadata(path) {
                        if metadata.len() < COVER_MIN_FILE_SIZE {
                            return true;
                        }
                    }
                }
                false
            })
            .collect();

        let api_key = db_guard.get_setting("steamgriddb_api_key")
            .map_err(|e| e.to_string())?
            .unwrap_or_default();

        (games_without_cover, api_key)
        // db_guard 在此释放
    };

    if games_without_cover.is_empty() {
        return Ok(serde_json::json!({
            "fetched": 0,
            "total": 0,
            "errors": [],
        }));
    }

    if api_key.is_empty() {
        return Ok(serde_json::json!({
            "fetched": 0,
            "total": games_without_cover.len(),
            "errors": ["未配置 SteamGridDB API Key，请在设置中填写"],
        }));
    }

    // 第二阶段：进行网络请求（不持有数据库锁）
    let cache_dir = utils::path::get_app_data_dir().join("covers");
    let fetcher = CoverFetcher::new(cache_dir, api_key).map_err(|e| e.to_string())?;

    let mut fetched_count = 0u32;
    let mut errors: Vec<String> = Vec::new();
    let total_missing = games_without_cover.len() as u32;

    for (index, game) in games_without_cover.iter().enumerate() {
        // 发送进度事件
        let _ = app_handle.emit("cover-fetch-progress", serde_json::json!({
            "current": index + 1,
            "total": total_missing,
            "game_name": game.name,
        }));

        match fetcher.fetch_cover(game).await {
            Ok(Some(cover_url)) => {
                // 第三阶段：单次获取锁更新封面，然后立即释放
                let update_result = {
                    let db_guard = lock_or_recover(&db);
                    db_guard.update_game_cover_url(&game.id, &cover_url)
                };
                match update_result {
                    Ok(_) => {
                        fetched_count += 1;
                        tracing::info!("获取到封面: {}", game.name);
                    }
                    Err(e) => {
                        errors.push(format!("{}: 更新封面失败 - {}", game.name, e));
                    }
                }
            }
            Ok(None) => {
                tracing::warn!("未找到封面: {} (游戏名可能不在 SteamGridDB 中)", game.name);
                errors.push(format!("{}: 未在 SteamGridDB 中找到封面，请检查游戏名称或手动设置", game.name));
            }
            Err(e) => {
                tracing::warn!("获取封面失败 {}: {}", game.name, e);
                errors.push(format!("{}: {}", game.name, e));
            }
        }
    }

    Ok(serde_json::json!({
        "fetched": fetched_count,
        "total": total_missing,
        "errors": errors,
    }))
}

/// 获取游戏的所有可选封面（从 SteamGridDB）
#[tauri::command]
pub async fn fetch_cover_options(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
) -> Result<Vec<CoverOption>, String> {
    let (game_name, install_path, api_key) = {
        let db_guard = lock_or_recover(&db);
        let game = db_guard.get_game_by_id(&game_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "游戏不存在".to_string())?;
        let api_key = db_guard.get_setting("steamgriddb_api_key")
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        (game.name, game.install_path, api_key)
    };

    if api_key.is_empty() {
        return Err("未配置 SteamGridDB API Key，请在设置中填写".to_string());
    }

    let cache_dir = utils::path::get_app_data_dir().join("covers");
    let fetcher = CoverFetcher::new(cache_dir, api_key).map_err(|e| e.to_string())?;

    fetcher.fetch_cover_options(&game_name, install_path.as_deref())
        .await
        .map_err(|e| e.to_string())
}

/// 从 URL 下载封面图片并设置为游戏封面
#[tauri::command]
pub async fn set_game_cover_from_url(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
    url: String,
) -> Result<(), String> {
    let api_key = {
        let db_guard = lock_or_recover(&db);
        // 验证游戏存在
        let _game = db_guard.get_game_by_id(&game_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "游戏不存在".to_string())?;
        db_guard.get_setting("steamgriddb_api_key")
            .map_err(|e| e.to_string())?
            .unwrap_or_default()
    };

    let covers_dir = utils::path::get_covers_dir();
    utils::path::ensure_dir_exists(&covers_dir).map_err(|e| e.to_string())?;
    let save_path = covers_dir.join(format!("{}.jpg", game_id));

    let fetcher = CoverFetcher::new(covers_dir.clone(), api_key).map_err(|e| e.to_string())?;
    fetcher.download_from_url(&url, &save_path)
        .await
        .map_err(|e| format!("下载封面失败: {}", e))?;

    let cover_path = save_path.to_string_lossy().to_string();
    let db_guard = lock_or_recover(&db);
    db_guard.update_game_cover(&game_id, &cover_path)
        .map_err(|e| e.to_string())
}

/// 从 LLM 获取游戏元数据
#[tauri::command]
pub async fn fetch_game_info_llm(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
) -> Result<Game, String> {
    // 用作用域块确保 MutexGuard 在 await 之前被释放
    let (game, config) = {
        let db_guard = lock_or_recover(&db);
        let game = db_guard.get_game_by_id(&game_id).map_err(|e| e.to_string())?;
        let game = game.ok_or_else(|| "游戏不存在".to_string())?;

        // 使用统一的设置加载方法
        let settings = Settings::load_from_db(&db_guard).map_err(|e| e.to_string())?;

        if !settings.llm_enabled {
            return Err("未启用 LLM 获取游戏信息，请在设置中配置".to_string());
        }

        if settings.llm_api_key.is_empty() {
            return Err("未配置 LLM API Key，请在设置中填写".to_string());
        }

        let protocol = match settings.llm_protocol.as_str() {
            "anthropic" => LlmProtocol::Anthropic,
            _ => LlmProtocol::Openai,
        };

        let config = LlmConfig {
            enabled: true,
            protocol,
            api_key: settings.llm_api_key,
            base_url: settings.llm_base_url,
            model: settings.llm_model,
        };

        (game, config)
        // db_guard 在此作用域结束时自动释放
    };

    // 此处已无 MutexGuard，可以安全 .await
    let meta = llm_fetcher::fetch_game_meta(&config, &game.name)
        .await
        .map_err(|e| format!("LLM 获取游戏信息失败: {}", e))?;

    // 重新获取锁更新游戏数据
    let db_guard = lock_or_recover(&db);

    // 重新从数据库读取最新数据，避免覆盖 LLM 请求期间用户的并发修改
    let mut updated = db_guard.get_game_by_id(&game_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "游戏不存在".to_string())?;
    if let Some(name) = meta.name {
        let trimmed = name.trim().to_string();
        if !trimmed.is_empty() && trimmed != updated.name {
            tracing::info!("LLM 纠正游戏名称: '{}' -> '{}'", updated.name, trimmed);
            updated.name = trimmed;
        }
    }
    if let Some(desc) = meta.description {
        if !desc.is_empty() {
            updated.description = Some(desc);
        }
    }
    if let Some(dev) = meta.developer {
        if !dev.is_empty() {
            updated.developer = Some(dev);
        }
    }
    if let Some(pub_) = meta.publisher {
        if !pub_.is_empty() {
            updated.publisher = Some(pub_);
        }
    }
    if let Some(date) = meta.release_date {
        if !date.is_empty() {
            updated.release_date = Some(date);
        }
    }
    if !meta.genres.is_empty() {
        updated.genres = meta.genres;
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
    if !meta.save_paths.is_empty() {
        updated.save_paths = meta.save_paths;
    }

    db_guard.update_game(&updated).map_err(|e| e.to_string())?;
    Ok(updated)
}

/// 根据文件扩展名检测图片 MIME 类型
fn detect_image_mime(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        _ => "image/jpeg", // 默认 jpeg（包括 .jpg）
    }
}

/// 读取本地图片文件并返回 base64 data URL（绕过 asset protocol）
/// 仅允许读取 covers 目录下的文件，防止路径遍历攻击
#[tauri::command]
pub fn read_cover_as_base64(path: String) -> Result<String, String> {
    use base64::Engine as _;

    let file_path = std::path::Path::new(&path);

    // 安全检查：验证路径在 covers 目录下（防止路径遍历）
    let covers_dir = utils::path::get_covers_dir();
    let canonical_covers = covers_dir.canonicalize()
        .unwrap_or(covers_dir.clone());
    let canonical_file = file_path.canonicalize()
        .map_err(|_| format!("文件不存在: {}", path))?;

    if !canonical_file.starts_with(&canonical_covers) {
        return Err("不允许读取 covers 目录之外的文件".to_string());
    }

    let bytes = std::fs::read(&canonical_file).map_err(|e| format!("读取文件失败: {}", e))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let mime = detect_image_mime(&canonical_file);
    Ok(format!("data:{};base64,{}", mime, b64))
}

/// 重命名游戏
#[tauri::command]
pub fn rename_game(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
    new_name: String,
) -> Result<(), String> {
    if new_name.trim().is_empty() {
        return Err("游戏名称不能为空".to_string());
    }
    let db = lock_or_recover(&db);
    let mut game = db.get_game_by_id(&game_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "游戏不存在".to_string())?;
    game.name = new_name.trim().to_string();
    game.updated_at = Some(chrono::Utc::now().to_rfc3339());
    db.update_game(&game).map_err(|e| e.to_string())
}

/// 更新游戏可执行文件路径（同时刷新 exe_name、install_path、exe_version）
#[tauri::command]
pub fn update_exe_path(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
    new_exe_path: String,
) -> Result<Game, String> {
    let db = lock_or_recover(&db);
    let mut game = db.get_game_by_id(&game_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "游戏不存在".to_string())?;

    game.exe_path = Some(new_exe_path.clone());
    game.exe_name = Some(std::path::Path::new(&new_exe_path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string());
    game.install_path = Some(std::path::Path::new(&new_exe_path)
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .to_string_lossy()
        .to_string());
    game.exe_version = utils::path::read_exe_version(&new_exe_path);
    game.updated_at = Some(chrono::Utc::now().to_rfc3339());

    db.update_game(&game).map_err(|e| e.to_string())?;
    Ok(game)
}

/// 设置游戏状态
#[tauri::command]
pub fn set_game_status(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
    status: String,
) -> Result<(), String> {
    // 验证状态值
    let valid_statuses = ["unplayed", "playing", "completed", "abandoned"];
    if !valid_statuses.contains(&status.as_str()) {
        return Err(format!("无效的游戏状态: {}，有效值为: {:?}", status, valid_statuses));
    }

    let db = lock_or_recover(&db);

    // 验证游戏存在
    let _game = db.get_game_by_id(&game_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "游戏不存在".to_string())?;

    db.set_game_status(&game_id, &status).map_err(|e| e.to_string())
}

/// 导出游戏库数据为 JSON 文件
#[tauri::command]
pub fn export_game_data(
    db: State<'_, Arc<Mutex<Database>>>,
) -> Result<String, String> {
    let db_guard = lock_or_recover(&db);

    // 获取所有游戏数据
    let filter = GameFilter::default();
    let games = db_guard.get_games(&filter).map_err(|e| e.to_string())?;

    // 获取设置
    let settings = Settings::load_from_db(&db_guard).map_err(|e| e.to_string())?;

    // 构建导出数据结构
    let export_data = serde_json::json!({
        "version": "1.0",
        "exported_at": chrono::Utc::now().to_rfc3339(),
        "games": games,
        "settings": settings,
    });

    // 序列化为 JSON
    let json = serde_json::to_string_pretty(&export_data)
        .map_err(|e| format!("序列化失败: {}", e))?;

    Ok(json)
}

/// 导入游戏库数据（从 JSON 备份恢复）
#[tauri::command]
pub fn import_game_data(
    db: State<'_, Arc<Mutex<Database>>>,
    json_data: String,
) -> Result<serde_json::Value, String> {
    let import_data: serde_json::Value = serde_json::from_str(&json_data)
        .map_err(|e| format!("JSON 解析失败: {}", e))?;

    let db_guard = lock_or_recover(&db);

    // 导入游戏
    let mut imported_games = 0u32;
    if let Some(games_array) = import_data["games"].as_array() {
        for game_json in games_array {
            match serde_json::from_value::<Game>(game_json.clone()) {
                Ok(game) => {
                    if let Err(e) = db_guard.upsert_game(&game) {
                        tracing::warn!("导入游戏失败 {}: {}", game.name, e);
                    } else {
                        imported_games += 1;
                    }
                }
                Err(e) => {
                    tracing::warn!("解析游戏数据失败: {}", e);
                }
            }
        }
    }

    // 导入设置
    let mut settings_restored = false;
    if let Some(settings_json) = import_data.get("settings") {
        match serde_json::from_value::<Settings>(settings_json.clone()) {
            Ok(settings) => {
                if let Err(e) = settings.save_to_db(&db_guard) {
                    tracing::warn!("导入设置失败: {}", e);
                } else {
                    settings_restored = true;
                }
            }
            Err(e) => {
                tracing::warn!("解析设置数据失败: {}", e);
            }
        }
    }

    Ok(serde_json::json!({
        "imported_games": imported_games,
        "settings_restored": settings_restored,
    }))
}

/// 获取所有游戏类型（去重列表）
#[tauri::command]
pub fn get_all_genres(
    db: State<'_, Arc<Mutex<Database>>>,
) -> Result<Vec<String>, String> {
    let db = lock_or_recover(&db);
    db.get_all_genres().map_err(|e| e.to_string())
}

/// 批量读取封面图片为 base64 data URL（减少 IPC 调用次数）
/// 仅允许读取 covers 目录下的文件
#[tauri::command]
pub fn read_covers_batch_as_base64(paths: Vec<String>) -> Result<std::collections::HashMap<String, String>, String> {
    use base64::Engine as _;

    let covers_dir = utils::path::get_covers_dir();
    let canonical_covers = covers_dir.canonicalize()
        .unwrap_or(covers_dir.clone());

    let mut result = std::collections::HashMap::new();

    for path in paths {
        let file_path = std::path::Path::new(&path);

        // 安全检查
        let canonical_file = match file_path.canonicalize() {
            Ok(p) => p,
            Err(_) => continue, // 文件不存在，跳过
        };

        if !canonical_file.starts_with(&canonical_covers) {
            continue; // 不在 covers 目录下，跳过
        }

        if let Ok(bytes) = std::fs::read(&canonical_file) {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let mime = detect_image_mime(&canonical_file);
            result.insert(path, format!("data:{};base64,{}", mime, b64));
        }
    }

    Ok(result)
}

/// 打开存档路径（在文件管理器中）
#[tauri::command]
pub async fn open_save_path(path: String, app_handle: tauri::AppHandle) -> Result<(), String> {
    let expanded = utils::path::expand_env_vars(&path);
    let path = std::path::PathBuf::from(&expanded);

    if !path.exists() {
        return Err(format!("路径不存在: {}", expanded));
    }

    // 如果是文件，打开其父目录；如果是目录，直接打开
    let target = if path.is_file() {
        path.parent().unwrap_or(&path).to_path_buf()
    } else {
        path
    };

    app_handle.opener().open_path(
        target.to_string_lossy(),
        None::<&str>,
    ).map_err(|e| format!("打开路径失败: {}", e))?;

    Ok(())
}

/// 更新游戏存档路径列表
#[tauri::command]
pub fn update_save_paths(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
    save_paths: Vec<String>,
) -> Result<(), String> {
    let db_guard = lock_or_recover(&db);
    let mut game = db_guard.get_game_by_id(&game_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "游戏不存在".to_string())?;

    game.save_paths = save_paths;
    game.updated_at = Some(chrono::Utc::now().to_rfc3339());
    db_guard.update_game(&game).map_err(|e| e.to_string())
}

/// 将目录或文件添加到 ZIP 归档中
fn add_path_to_zip(
    zip: &mut zip::ZipWriter<std::io::BufWriter<std::fs::File>>,
    base_path: &std::path::Path,
    current_path: &std::path::Path,
    zip_prefix: &str,
) -> Result<(), String> {
    use zip::write::FileOptions;

    if current_path.is_file() {
        let relative = current_path.strip_prefix(base_path)
            .unwrap_or(current_path);
        let zip_path = if zip_prefix.is_empty() {
            relative.to_string_lossy().to_string()
        } else {
            format!("{}/{}", zip_prefix, relative.to_string_lossy())
        };

        let options = FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file(&zip_path, options)
            .map_err(|e| format!("创建 ZIP 文件条目失败: {}", e))?;
        let mut file = std::fs::File::open(current_path)
            .map_err(|e| format!("读取文件失败 {}: {}", current_path.display(), e))?;
        std::io::copy(&mut file, zip)
            .map_err(|e| format!("写入 ZIP 失败: {}", e))?;
    } else if current_path.is_dir() {
        for entry in std::fs::read_dir(current_path)
            .map_err(|e| format!("读取目录失败 {}: {}", current_path.display(), e))?
        {
            let entry = entry.map_err(|e| format!("读取目录项失败: {}", e))?;
            add_path_to_zip(zip, base_path, &entry.path(), zip_prefix)?;
        }
    }

    Ok(())
}

/// 导出存档备份为 ZIP 文件
#[tauri::command]
pub async fn export_saves_backup(
    db: State<'_, Arc<Mutex<Database>>>,
    export_path: String,
) -> Result<serde_json::Value, String> {
    let games = {
        let db_guard = lock_or_recover(&db);
        let filter = GameFilter::default();
        db_guard.get_games(&filter).map_err(|e| e.to_string())?
    };

    let file = std::fs::File::create(&export_path)
        .map_err(|e| format!("创建 ZIP 文件失败: {}", e))?;
    let buf_writer = std::io::BufWriter::new(file);
    let mut zip = zip::ZipWriter::new(buf_writer);

    let mut manifest: Vec<serde_json::Value> = Vec::new();
    let mut exported_count = 0u32;
    let mut errors: Vec<String> = Vec::new();

    for game in &games {
        if game.save_paths.is_empty() {
            continue;
        }

        for (idx, save_path) in game.save_paths.iter().enumerate() {
            let expanded = utils::path::expand_env_vars(save_path);
            let path = std::path::PathBuf::from(&expanded);

            if !path.exists() {
                errors.push(format!("{}: 路径不存在 - {}", game.name, expanded));
                continue;
            }

            // ZIP 内的目录名：游戏名_序号（避免特殊字符）
            let safe_name = game.name.chars()
                .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
                .collect::<String>();
            let zip_prefix = if game.save_paths.len() > 1 {
                format!("{}_{}", safe_name, idx + 1)
            } else {
                safe_name.clone()
            };

            match add_path_to_zip(&mut zip, &path, &path, &zip_prefix) {
                Ok(_) => {
                    manifest.push(serde_json::json!({
                        "game_id": game.id,
                        "game_name": game.name,
                        "original_path": save_path,
                        "zip_prefix": zip_prefix,
                    }));
                    exported_count += 1;
                }
                Err(e) => {
                    errors.push(format!("{}: {}", game.name, e));
                }
            }
        }
    }

    // 写入 manifest.json
    use std::io::Write;
    use zip::write::FileOptions;
    let options = FileOptions::default();
    zip.start_file("manifest.json", options)
        .map_err(|e| format!("创建 manifest 失败: {}", e))?;
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("序列化 manifest 失败: {}", e))?;
    zip.write_all(manifest_json.as_bytes())
        .map_err(|e| format!("写入 manifest 失败: {}", e))?;

    zip.finish().map_err(|e| format!("完成 ZIP 文件失败: {}", e))?;

    Ok(serde_json::json!({
        "exported": exported_count,
        "errors": errors,
    }))
}

/// 从 ZIP 备份文件导入存档（不需要数据库锁，仅做文件 I/O）
#[tauri::command]
pub async fn import_saves_backup(
    zip_path: String,
) -> Result<serde_json::Value, String> {
    let file = std::fs::File::open(&zip_path)
        .map_err(|e| format!("打开 ZIP 文件失败: {}", e))?;
    let buf_reader = std::io::BufReader::new(file);
    let mut archive = zip::ZipArchive::new(buf_reader)
        .map_err(|e| format!("读取 ZIP 文件失败: {}", e))?;

    // 读取 manifest.json
    let manifest: Vec<serde_json::Value> = {
        let mut manifest_file = archive.by_name("manifest.json")
            .map_err(|_| "ZIP 文件中缺少 manifest.json".to_string())?;
        let mut content = String::new();
        std::io::Read::read_to_string(&mut manifest_file, &mut content)
            .map_err(|e| format!("读取 manifest.json 失败: {}", e))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("解析 manifest.json 失败: {}", e))?
    };

    // 预处理：一次性收集所有 ZIP 文件名，避免对每个 manifest 条目重复遍历 ZIP 目录
    let all_zip_names: Vec<String> = archive.file_names().map(|s| s.to_string()).collect();

    // 构建每个 manifest 条目对应的目标路径映射
    let mut extract_plan: Vec<(String, std::path::PathBuf)> = Vec::new();
    for entry in &manifest {
        let original_path = entry["original_path"].as_str().unwrap_or("");
        let zip_prefix = entry["zip_prefix"].as_str().unwrap_or("");

        if original_path.is_empty() || zip_prefix.is_empty() {
            continue;
        }

        let expanded = utils::path::expand_env_vars(original_path);
        let target_path = std::path::PathBuf::from(&expanded);

        // 创建目标目录
        if let Some(parent) = target_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!("创建目录失败 {}: {}", parent.display(), e);
                continue;
            }
        }

        // 从已缓存的 ZIP 文件名列表中筛选该前缀下的条目
        let prefix_with_slash = format!("{}/", zip_prefix);
        let file_names: Vec<&str> = all_zip_names.iter()
            .filter(|name| name.as_str() == zip_prefix || name.starts_with(&prefix_with_slash))
            .map(|s| s.as_str())
            .collect();

        for file_name in file_names {
            let relative = file_name.strip_prefix(&prefix_with_slash)
                .unwrap_or(file_name);

            // 安全检查：防止 ZIP 路径穿越攻击（如 prefix/../../etc/important_file）
            let relative_path = std::path::Path::new(relative);
            let has_parent_traversal = relative_path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir));
            if has_parent_traversal {
                tracing::warn!("跳过包含路径遍历的 ZIP 条目: {}", file_name);
                continue;
            }
            // 安全检查：拒绝绝对路径的 ZIP 条目
            let has_absolute = relative_path
                .components()
                .any(|c| matches!(c, std::path::Component::RootDir | std::path::Component::Prefix(_)));
            if has_absolute {
                tracing::warn!("跳过包含绝对路径的 ZIP 条目: {}", file_name);
                continue;
            }

            let dest = if relative.is_empty() {
                target_path.clone()
            } else {
                target_path.join(relative)
            };
            // 最终安全检查：确保解压目标在预期目录下
            // canonicalize 要求路径存在，因此对不存在的路径使用父目录检查
            let is_safe = if let Ok(canonical_dest) = dest.canonicalize() {
                // 路径已存在，直接比较
                target_path.canonicalize()
                    .map(|ct| canonical_dest.starts_with(&ct))
                    .unwrap_or(false)
            } else {
                // 路径不存在（新文件），检查其父目录
                let parent = dest.parent().unwrap_or(&dest);
                if let Ok(canonical_parent) = parent.canonicalize() {
                    if let Ok(canonical_target) = target_path.canonicalize() {
                        // 父目录必须在目标目录下，且文件名不含路径分隔符
                        canonical_parent.starts_with(&canonical_target)
                            && dest.file_name().is_some()
                    } else {
                        false
                    }
                } else {
                    false
                }
            };
            if !is_safe {
                tracing::warn!("跳过目标路径超出预期目录的 ZIP 条目: {}", file_name);
                continue;
            }
            extract_plan.push((file_name.to_string(), dest));
        }
    }

    // 一次性遍历计划，逐个从 archive 中提取（archive 只打开一次）
    let mut errors: Vec<String> = Vec::new();

    for (file_name, dest) in extract_plan {
        let mut zip_file = match archive.by_name(&file_name) {
            Ok(f) => f,
            Err(e) => {
                errors.push(format!("读取 ZIP 条目失败 {}: {}", file_name, e));
                continue;
            }
        };

        if zip_file.is_dir() {
            if let Err(e) = std::fs::create_dir_all(&dest) {
                errors.push(format!("创建目录失败: {}", e));
            }
        } else {
            if let Some(parent) = dest.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    errors.push(format!("创建目录失败: {}", e));
                    continue;
                }
            }
            let mut dest_file = match std::fs::File::create(&dest) {
                Ok(f) => f,
                Err(e) => {
                    errors.push(format!("创建文件失败 {}: {}", dest.display(), e));
                    continue;
                }
            };
            if let Err(e) = std::io::copy(&mut zip_file, &mut dest_file) {
                errors.push(format!("写入文件失败 {}: {}", dest.display(), e));
            }
        }
    }

    // 恢复计数 = manifest 中有效条目数
    let restored_count = manifest.iter().filter(|e| {
        !e["original_path"].as_str().unwrap_or("").is_empty()
        && !e["zip_prefix"].as_str().unwrap_or("").is_empty()
    }).count() as u32;

    for entry in &manifest {
        let game_id = entry["game_id"].as_str().unwrap_or("");
        let original_path = entry["original_path"].as_str().unwrap_or("");
        if !original_path.is_empty() {
            tracing::info!("已恢复存档: {} -> {}", game_id, utils::path::expand_env_vars(original_path));
        }
    }

    Ok(serde_json::json!({
        "restored": restored_count,
        "errors": errors,
    }))
}

/// 批量刷新缺失游戏信息的游戏
#[tauri::command]
pub async fn fetch_missing_game_info(
    db: State<'_, Arc<Mutex<Database>>>,
    app_handle: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    // 阶段1：收集需要获取信息的游戏
    let (missing_games, settings) = {
        let db_guard = lock_or_recover(&db);
        let filter = GameFilter::default();
        let games = db_guard.get_games(&filter).map_err(|e| e.to_string())?;

        // 使用统一的设置加载方法
        let settings = Settings::load_from_db(&db_guard).map_err(|e| e.to_string())?;

        // 检查 LLM 配置
        if !settings.llm_enabled {
            return Err("未启用 LLM 获取游戏信息，请在设置中配置".to_string());
        }
        if settings.llm_api_key.is_empty() {
            return Err("未配置 LLM API Key，请在设置中填写".to_string());
        }

        // 判断游戏信息是否"完全缺失"：所有可获取的元数据字段都为空
        let missing_games: Vec<Game> = games.into_iter().filter(|g| {
            g.description.is_none()
                && g.developer.is_none()
                && g.publisher.is_none()
                && g.release_date.is_none()
                && g.genres.is_empty()
                && g.hltb_main_story.is_none()
                && g.hltb_main_extra.is_none()
                && g.hltb_completionist.is_none()
        }).collect();

        (missing_games, settings)
        // db_guard 在此释放
    };

    if missing_games.is_empty() {
        return Ok(serde_json::json!({
            "fetched": 0,
            "total": 0,
            "errors": [],
        }));
    }

    let total = missing_games.len() as u32;

    // 构建 LLM 配置
    let protocol = match settings.llm_protocol.as_str() {
        "anthropic" => LlmProtocol::Anthropic,
        _ => LlmProtocol::Openai,
    };
    let config = LlmConfig {
        enabled: true,
        protocol,
        api_key: settings.llm_api_key,
        base_url: settings.llm_base_url,
        model: settings.llm_model,
    };

    // 阶段2：逐个获取游戏信息（串行，避免 API 限流）
    let mut fetched_count: u32 = 0;
    let mut errors: Vec<String> = Vec::new();

    for (index, game) in missing_games.iter().enumerate() {
        // 发送进度事件
        let _ = app_handle.emit("game-info-fetch-progress", serde_json::json!({
            "current": index + 1,
            "total": total,
            "game_name": game.name,
        }));

        // 调用 LLM 获取游戏信息
        match llm_fetcher::fetch_game_meta(&config, &game.name).await {
            Ok(meta) => {
                // 将获取的信息更新到游戏数据（只更新非空字段，保留用户已有的数据）
                // 使用闭包限制 ? 传播：单个游戏失败不应中断整个批量处理
                let update_result: Result<(), String> = (|| {
                    let db_guard = lock_or_recover(&db);

                    // 重新从数据库读取最新数据，避免覆盖 LLM 请求期间用户的并发修改
                    let mut updated = db_guard.get_game_by_id(&game.id)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| "游戏不存在".to_string())?;
                    if let Some(name) = meta.name {
                        let trimmed = name.trim().to_string();
                        if !trimmed.is_empty() && trimmed != updated.name {
                            tracing::info!("LLM 纠正游戏名称: '{}' -> '{}'", updated.name, trimmed);
                            updated.name = trimmed;
                        }
                    }
                    if let Some(desc) = meta.description {
                        if !desc.is_empty() {
                            updated.description = Some(desc);
                        }
                    }
                    if let Some(dev) = meta.developer {
                        if !dev.is_empty() {
                            updated.developer = Some(dev);
                        }
                    }
                    if let Some(pub_) = meta.publisher {
                        if !pub_.is_empty() {
                            updated.publisher = Some(pub_);
                        }
                    }
                    if let Some(date) = meta.release_date {
                        if !date.is_empty() {
                            updated.release_date = Some(date);
                        }
                    }
                    if !meta.genres.is_empty() {
                        updated.genres = meta.genres;
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
                    if !meta.save_paths.is_empty() {
                        updated.save_paths = meta.save_paths;
                    }

                    db_guard.update_game(&updated).map_err(|e| e.to_string())
                })();

                match update_result {
                    Ok(_) => {
                        fetched_count += 1;
                        tracing::info!("获取到游戏信息: {}", game.name);
                    }
                    Err(e) => {
                        errors.push(format!("{}: 更新信息失败 - {}", game.name, e));
                    }
                }
            }
            Err(e) => {
                tracing::warn!("获取游戏信息失败 {}: {}", game.name, e);
                errors.push(format!("{}: {}", game.name, e));
            }
        }
    }

    // 阶段3：发送完成事件（进度归零）
    let _ = app_handle.emit("game-info-fetch-progress", serde_json::json!({
        "current": 0,
        "total": 0,
        "game_name": "",
    }));

    Ok(serde_json::json!({
        "fetched": fetched_count,
        "total": total,
        "errors": errors,
    }))
}
