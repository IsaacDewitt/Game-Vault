use tauri::State;
use std::sync::{Arc, Mutex};
use crate::core::{Database, PlayTimeTracker, GameLauncher};
use crate::core::cover_fetcher::CoverFetcher;
use crate::core::llm_fetcher::{self, LlmConfig, LlmProtocol, LlmProvider};
use crate::models::*;
use crate::models::settings::*;
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
        if let Some(finished_session) = tracker_guard.start_tracking(&game_id, exe_name) {
            // 如果有旧会话结束，持久化到数据库
            drop(tracker_guard);  // 释放 Tracker 锁再获取 DB 锁
            let db_guard = lock_or_recover(&db);
            let _ = db_guard.add_play_session(
                &finished_session.game_id,
                &finished_session.start_time,
                finished_session.duration_seconds,
            );
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
    let db = lock_or_recover(&db);
    db.delete_game(&game_id).map_err(|e| e.to_string())?;

    // 清理对应的封面图片缓存
    let cover_path = utils::path::get_covers_dir().join(format!("{}.jpg", game_id));
    let _ = std::fs::remove_file(cover_path);

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

/// 获取缺失封面的游戏封面
#[tauri::command]
pub fn fetch_missing_covers(
    db: State<'_, Arc<Mutex<Database>>>,
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

    for game in &games_without_cover {
        match fetcher.fetch_cover(game) {
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

        // 从设置中读取 LLM 配置
        let provider_str = db_guard.get_setting("llm_provider").map_err(|e| e.to_string())?.unwrap_or_else(|| DEFAULT_LLM_PROVIDER.to_string());
        let protocol_str = db_guard.get_setting("llm_protocol").map_err(|e| e.to_string())?.unwrap_or_else(|| DEFAULT_LLM_PROTOCOL.to_string());
        let api_key = db_guard.get_setting("llm_api_key").map_err(|e| e.to_string())?.unwrap_or_default();
        let base_url = db_guard.get_setting("llm_base_url").map_err(|e| e.to_string())?.unwrap_or_else(|| DEFAULT_LLM_BASE_URL.to_string());
        let model = db_guard.get_setting("llm_model").map_err(|e| e.to_string())?.unwrap_or_else(|| DEFAULT_LLM_MODEL.to_string());
        let enabled = db_guard.get_setting("llm_enabled").map_err(|e| e.to_string())?.unwrap_or_else(|| "false".to_string()) == "true";

        if !enabled {
            return Err("未启用 LLM 获取游戏信息，请在设置中配置".to_string());
        }

        if api_key.is_empty() {
            return Err("未配置 LLM API Key，请在设置中填写".to_string());
        }

        let provider = match provider_str.as_str() {
            "deepseek" => LlmProvider::Deepseek,
            _ => LlmProvider::Xiaomi,
        };
        let protocol = match protocol_str.as_str() {
            "anthropic" => LlmProtocol::Anthropic,
            _ => LlmProtocol::Openai,
        };

        let config = LlmConfig {
            enabled: true,
            provider,
            protocol,
            api_key,
            base_url,
            model,
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
#[tauri::command]
pub fn read_cover_as_base64(path: String) -> Result<String, String> {
    use base64::Engine as _;
    let file_path = std::path::Path::new(&path);
    if !file_path.exists() {
        return Err(format!("文件不存在: {}", path));
    }
    let bytes = std::fs::read(file_path).map_err(|e| format!("读取文件失败: {}", e))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:image/jpeg;base64,{}", b64))
}
