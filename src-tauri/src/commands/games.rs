use tauri::{State, Emitter};
use std::sync::{Arc, Mutex};
use crate::core::{Database, PlayTimeTracker, GameLauncher};
use crate::core::cover_fetcher::CoverFetcher;
use crate::core::llm_fetcher::{self, LlmConfig, LlmProtocol, LlmProvider};
use crate::models::*;
use crate::models::settings::Settings;
use crate::utils;
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

    // 阶段 2：启动游戏（无锁状态下的 I/O 操作）
    GameLauncher::launch(&game).map_err(|e| e.to_string())?;

    // 阶段 3：开始追踪时长（独立获取 Tracker 锁）
    if let Some(ref exe_name) = game.exe_name {
        let mut tracker_guard = lock_or_recover(&tracker);
        if let Some(finished_session) = tracker_guard.start_tracking(&game_id, exe_name, game.exe_path.as_deref()) {
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

    // 清理封面缓存文件（.jpg 和 .png）
    let covers_dir = utils::path::get_covers_dir();
    for ext in &["jpg", "png"] {
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

    db.upsert_game(&game).map_err(|e| e.to_string())?;
    Ok(game)
}

/// 设置游戏封面（手动选择本地图片）
#[tauri::command]
pub fn set_game_cover(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
    cover_path: String,
) -> Result<(), String> {
    let db = lock_or_recover(&db);
    db.update_game_cover(&game_id, &cover_path).map_err(|e| e.to_string())
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
                    if metadata.len() >= 100 {
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
                        if metadata.len() < 100 {
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
                        if metadata.len() < 100 {
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
    let fetcher = CoverFetcher::new(cache_dir, api_key);

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

        let provider = match settings.llm_provider.as_str() {
            "deepseek" => LlmProvider::Deepseek,
            _ => LlmProvider::Xiaomi,
        };
        let protocol = match settings.llm_protocol.as_str() {
            "anthropic" => LlmProtocol::Anthropic,
            _ => LlmProtocol::Openai,
        };

        let config = LlmConfig {
            enabled: true,
            provider,
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

    let mut updated = game;
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

    db_guard.update_game(&updated).map_err(|e| e.to_string())?;
    Ok(updated)
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
    Ok(format!("data:image/jpeg;base64,{}", b64))
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
            result.insert(path, format!("data:image/jpeg;base64,{}", b64));
        }
    }

    Ok(result)
}
