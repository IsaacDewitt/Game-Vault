use tauri::{State, Emitter};
use tauri_plugin_opener::OpenerExt;
use std::sync::{Arc, Mutex};
use crate::core::{Database, PlayTimeTracker, GameLauncher};
use crate::core::launcher::LaunchOutcome;
use crate::core::cover_fetcher::CoverFetcher;
use crate::core::cover_store;
use crate::models::cover::OWNER_GAME;
use crate::core::llm_fetcher::{LlmFetcher, LlmConfig, LlmProtocol, LlmGameMeta};
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

    // 阶段 2：启动（无锁状态下的 I/O 操作）
    // - local：直接 spawn exe，拿到 PID 可走进程树追踪
    // - steam / epic：交协议 URI 给客户端，**拿不到 PID**，须靠 armed 待命发现进程
    let outcome = GameLauncher::launch_game(&game).map_err(|e| e.to_string())?;

    // 阶段 3：开始追踪（独立获取 Tracker 锁）
    //
    // 平台游戏走 armed 待命而非即时计时：进程由客户端异步拉起（还要过 DRM 校验、云存档同步），
    // 在进程现身之前无法确认游戏真的跑起来了，先待命可避免"点了启动但没起来"记下一笔假时长。
    // Steam 游戏的 exe_name 为空（acf 不记录 exe 名），照样能靠 install_path 前缀匹配识别。
    let exe_name = game.exe_name.clone().unwrap_or_default();
    let mut tracker_guard = lock_or_recover(&tracker);

    let finished_session = match outcome {
        LaunchOutcome::Spawned(pid) => {
            // 既无 exe 名也无安装目录 → 无从追踪（与旧行为一致：不启动追踪）
            if exe_name.is_empty() && game.install_path.is_none() {
                tracing::warn!("游戏 {} 缺少 exe 名与安装目录，无法追踪时长", game.name);
                None
            } else {
                tracker_guard.start_tracking(
                    &game_id,
                    &exe_name,
                    game.exe_path.as_deref(),
                    Some(pid),
                    game.install_path.as_deref(),
                )
            }
        }
        LaunchOutcome::Delegated { uri } => {
            tracing::info!("已委托 {} 客户端拉起「{}」: {}", game.platform, game.name, uri);
            tracker_guard.arm_session(
                &game_id,
                &exe_name,
                game.exe_path.as_deref(),
                game.install_path.as_deref(),
            )
        }
    };

    if let Some(finished_session) = finished_session {
        // 有旧会话结束 → 释放 Tracker 锁再获取 DB 锁持久化
        drop(tracker_guard);
        let db_guard = lock_or_recover(&db);
        if let Err(e) = db_guard.add_play_session(
            &finished_session.game_id,
            &finished_session.start_time,
            finished_session.duration_seconds,
        ) {
            tracing::error!("保存旧游戏会话失败 (game_id: {}): {}", finished_session.game_id, e);
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
///
/// 语义拍板（2026-09-04）：删除只表示「从库中移除条目」，play_stats_daily/hourly
/// 的历史汇总**有意保留**（通关后移除条目、历史统计留念）。因此删除后
/// SUM(play_stats_daily) >= SUM(games.play_time_seconds)，差额即已删游戏的历史时长——
/// 这是有意设计，不是数据错误，后续维护时不要"修复"这个差值。
///
/// `keep_cover`：`Some(true)`（默认）保留封面——文件挪进 covers/archive 留档，重装认领后自动回归；
/// `Some(false)` 连封面一起删。前端删除前会询问用户，二者都遵循
/// 「删除游戏 = 仅移除库内条目」的大原则（历史时长/成就/封面都不随删除蒸发）。
#[tauri::command]
pub fn delete_game(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
    keep_cover: Option<bool>,
) -> Result<(), String> {
    let keep_cover = keep_cover.unwrap_or(true);

    // 先获取游戏信息以清理封面文件
    let has_any_cover = {
        let db_guard = lock_or_recover(&db);
        let game = db_guard.get_game_by_id(&game_id).map_err(|e| e.to_string())?;

        // 删除留档：写墓碑（2026-09-06）。历史汇总与成就本就删而留档，墓碑记住旧 id + 识别键
        // （名字 + exe 文件名），重装再入库时按名字认领、复用旧 id，孤儿历史统计即可自动续接。
        // 路径刻意不存——换盘/挪目录重装照样能认领。墓碑写失败不应阻断删除，仅告警。
        if let Some(g) = &game {
            if let Err(e) = db_guard.insert_tombstone(
                &g.id,
                &g.name,
                g.exe_name.as_deref(),
                &chrono::Utc::now().to_rfc3339(),
            ) {
                tracing::warn!("写游戏墓碑失败（不影响删除）: {}", e);
            }
        }

        let has_cover = !db_guard.cover_rows(OWNER_GAME, &game_id)
            .map_err(|e| e.to_string())?
            .is_empty()
            || game.as_ref().map(|g| g.cover_local.is_some() || g.cover_url.is_some()).unwrap_or(false);

        // 保留：归档（挪进 archive/，共享文件只改状态）；不保留：交给下面的 purge 递归处理
        if keep_cover {
            if let Err(e) = cover_store::archive(&db_guard, OWNER_GAME, &game_id) {
                tracing::warn!("归档封面失败（不影响删除）: {}", e);
            }
        }

        db_guard.delete_game(&game_id).map_err(|e| e.to_string())?;
        has_cover
    };

    if !keep_cover {
        let db_guard = lock_or_recover(&db);
        if let Err(e) = cover_store::purge(&db_guard, OWNER_GAME, &game_id) {
            tracing::warn!("删除封面失败（不影响删除）: {}", e);
        }
    } else if has_any_cover {
        tracing::info!("已删除游戏，封面已留档（重装可续接）: {}", game_id);
    }

    Ok(())
}

/// 入库收尾（供「手动添加」与「平台导入」共用）
///
/// 流程：墓碑认领（可选）→ upsert → **入库成功后**清墓碑 → 回填累计时长/次数 → 恢复留档封面。
/// 抽成公用函数是为了让两条入口的历史续接语义完全一致——平台导入若绕过认领，
/// "删掉再装回来"就会生成新 id，留档的历史统计与成就再也接不上。
///
/// 重装认领（2026-09-06）：若存在匹配的墓碑（即之前删除过、又装回来了的游戏），
/// 复用其旧 id 入库——play_stats_daily/hourly 与 achievement_unlocks 里留存的孤儿历史
/// 凭这个 id 自动续接（热力图/时长/成就原样回归）。匹配键刻意不含路径，换盘重装也能认领。
///
/// 两段式：① 名字 + exe 文件名双键（能区分同名不同游戏就尽量区分）；
/// ② 双键未命中且**库中已无同名活条目**时按名字兜底，且**不限 exe 名**
///   （2026-09-12 修订）——覆盖换安装程序重装（Steam 版删了装 GOG 版）、原条目无安装路径
///   等双键对不上的真实场景；前置的"无同名活条目"由这里把关，故不存在同名不同游戏的歧义。
///
/// Steam 平台条目 `exe_name` 为 None，自然落到第二段（名字兜底），语义正确。
pub(crate) fn insert_game_with_reclaim(db: &Database, mut game: Game) -> Result<Game, String> {
    let mut claimed_tombstone = false;
    let mut tombstone_id = match game.exe_name.as_deref() {
        Some(exe_name) => db
            .find_tombstone_for_reclaim(&game.name, Some(exe_name))
            .map_err(|e| e.to_string())?,
        None => None,
    };
    if tombstone_id.is_none() {
        let same_name_alive = db
            .count_games_by_name(&game.name)
            .map_err(|e| e.to_string())?;
        if same_name_alive == 0 {
            tombstone_id = db
                .find_tombstone_for_reclaim(&game.name, None)
                .map_err(|e| e.to_string())?;
        }
    }
    if let Some(old_id) = tombstone_id {
        match db.get_game_by_id(&old_id) {
            // 防呆：旧 id 已被其他途径复活（如备份导入）时不再认领，避免覆盖活条目
            Ok(Some(_)) => {
                tracing::warn!(
                    "墓碑 id {} 已存在于库中（疑似备份导入复活），跳过认领",
                    old_id
                );
            }
            Ok(None) => {
                game.id = old_id.clone();
                claimed_tombstone = true;
                tracing::info!(
                    "重装认领成功：「{}」复用旧 id {}，历史游玩统计与成就已续接",
                    game.name,
                    old_id
                );
            }
            Err(e) => {
                tracing::warn!("查重墓碑 id 失败，跳过认领: {}", e);
            }
        }
    }

    db.upsert_game(&game).map_err(|e| e.to_string())?;

    // 墓碑在**入库成功之后**才清除（2026-09-12 修正）：若先清后写而在 upsert 处失败，
    // 墓碑已丢、旧 id 未回库，留档的历史统计与封面就再也认领不回来了。
    if claimed_tombstone {
        if let Err(e) = db.remove_tombstone(&game.id) {
            // 清不掉也不算致命：后续认领会被"该 id 已在库中"的防呆挡下，不会误接
            tracing::warn!("清除墓碑失败（不影响本次入库）: {}", e);
        }
    }

    // 认领场景：新条目从 0 计，从预聚合表 SUM 回填累计时长/次数/上次游玩，让卡片数字完整回归
    if claimed_tombstone {
        if let Err(e) = db.reclaim_game_totals(&game.id) {
            tracing::warn!("回填认领游戏累计时长失败: {}", e);
        }
        // 封面一并回归：留档在 archive/ 的封面挪回正式目录（与历史时长同一套续接语义）
        match cover_store::restore(db, OWNER_GAME, &game.id) {
            Ok(true) => tracing::info!("已恢复「{}」的留档封面", game.name),
            Ok(false) => {}
            Err(e) => tracing::warn!("恢复留档封面失败: {}", e),
        }
    }

    Ok(game)
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

    // 获取并保存文件元数据（用于后续缓存判断）
    if let Some(metadata) = utils::path::get_file_metadata(&exe_path) {
        game.exe_modified_at = Some(metadata.modified_at);
        game.exe_file_size = Some(metadata.file_size);
    }

    insert_game_with_reclaim(&db, game)
}

/// 启动时批量刷新所有游戏的 exe 版本号
/// 仅对有 exe_path 且版本号为空或 exe 文件已变更的游戏进行更新
/// 使用文件元数据（修改时间 + 文件大小）进行缓存判断，避免每次都读取 exe 文件
/// 两阶段设计：收集游戏列表后立即释放 DB 锁，文件 I/O 在无锁状态下进行，最后重取锁批量更新
#[tauri::command]
pub fn refresh_exe_versions(
    db: State<'_, Arc<Mutex<Database>>>,
) -> Result<u32, String> {
    // 阶段 1：收集所有游戏，然后立即释放锁
    let games = {
        let db_guard = lock_or_recover(&db);
        let filter = GameFilter::default();
        db_guard.get_games(&filter).map_err(|e| e.to_string())?
    };

    // 阶段 2：无锁状态下逐个检查文件元数据、读取版本号
    let mut to_update: Vec<Game> = Vec::new();
    for game in games {
        let exe_path = match game.exe_path {
            Some(ref p) => p.clone(),
            None => continue,
        };

        let current_metadata = match utils::path::get_file_metadata(&exe_path) {
            Some(m) => m,
            None => {
                tracing::debug!("无法读取文件元数据: {}", exe_path);
                continue;
            }
        };

        // 检查文件是否发生变化（比较修改时间和文件大小）
        let file_changed = match (game.exe_modified_at, game.exe_file_size) {
            (Some(cached_modified), Some(cached_size)) => {
                current_metadata.modified_at != cached_modified || current_metadata.file_size != cached_size
            }
            _ => true, // 没有缓存，需要读取版本号
        };

        if file_changed {
            let new_version = utils::path::read_exe_version(&exe_path);
            let mut updated_game = game.clone();
            updated_game.exe_version = new_version;
            updated_game.exe_modified_at = Some(current_metadata.modified_at);
            updated_game.exe_file_size = Some(current_metadata.file_size);
            updated_game.updated_at = Some(chrono::Utc::now().to_rfc3339());
            to_update.push(updated_game);
        }
    }

    if to_update.is_empty() {
        return Ok(0);
    }

    // 阶段 3：重新获取锁批量更新
    let db_guard = lock_or_recover(&db);
    let mut updated = 0u32;
    for updated_game in to_update {
        if let Err(e) = db_guard.update_game(&updated_game) {
            tracing::warn!("更新游戏版本号失败 {}: {}", updated_game.name, e);
        } else {
            updated += 1;
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

    // 入库统一走 cover_store：解码 → 必要时转码 → 生成缩略图 → 写索引 → 清理旧文件。
    // delete_src = false：用户手选的文件不动它，只把内容收进封面库。
    //
    // 拆成两段（2026-09-12）：图像解码/编码是数百毫秒级的重活，放在**无锁段**做；
    // 只有写索引这一步短暂持锁，避免一张大图把其它 DB 命令全部堵住。
    let prepared = cover_store::prepare_path(OWNER_GAME, &game_id, src)
        .map_err(|e| e.to_string())?;
    let db = lock_or_recover(&db);
    cover_store::commit_prepared(&db, OWNER_GAME, &game_id, prepared)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// 删除游戏封面（摘索引 + 引用计数归零才删文件）
#[tauri::command]
pub fn remove_game_cover(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
) -> Result<(), String> {
    let db_guard = lock_or_recover(&db);
    cover_store::purge(&db_guard, OWNER_GAME, &game_id).map_err(|e| e.to_string())?;
    Ok(())
}

/// 获取所有游戏的有效封面路径（供前端通过 asset 协议加载）
///
/// 返回 `{ game_id: { main, thumb } }`：
/// - **含已移除游戏的留档封面**（state=archived）——时长排行里已删条目照样显示图片；
/// - `thumb` 供卡片网格/排行等小尺寸场景使用，避免整张原图参与解码。
/// - 索引缺失但 `cover_local` 指向的文件确实存在时兜底返回（迁移未跑完/外部放入）。
#[tauri::command]
pub fn get_all_covers(
    db: State<'_, Arc<Mutex<Database>>>,
) -> Result<std::collections::HashMap<String, crate::models::CoverSet>, String> {
    let db_guard = lock_or_recover(&db);

    let mut covers: std::collections::HashMap<String, crate::models::CoverSet> =
        std::collections::HashMap::new();

    for row in db_guard.all_cover_rows().map_err(|e| e.to_string())? {
        if row.owner_kind != OWNER_GAME {
            continue;
        }
        let abs = cover_store::abs_path(&row.rel_path);
        if !abs.exists() {
            continue;
        }
        let path = abs.to_string_lossy().to_string();
        let entry = covers.entry(row.owner_id).or_default();
        match row.kind.as_str() {
            crate::models::KIND_MAIN => entry.main = Some(path),
            crate::models::KIND_THUMB => entry.thumb = Some(path),
            _ => {}
        }
    }

    // 兜底：索引里没有、但 games.cover_local 指向的文件还在（迁移未跑完 / 外部放入）
    let games = db_guard
        .get_games(&GameFilter::default())
        .map_err(|e| e.to_string())?;
    for game in games {
        if covers.contains_key(&game.id) {
            continue;
        }
        let Some(path_str) = game.cover_local.or(game.cover_url) else {
            continue;
        };
        let path = std::path::Path::new(&path_str);
        if !path.exists() {
            continue;
        }
        if let Ok(metadata) = std::fs::metadata(path) {
            if metadata.len() >= COVER_MIN_FILE_SIZE {
                let entry = crate::models::CoverSet {
                    main: Some(path_str),
                    thumb: None,
                };
                covers.insert(game.id.clone(), entry);
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
            Ok(Some(cover_path)) => {
                // 第三阶段：封面入库。解码/编码在网络请求之后的**无锁段**完成，
                // 只有写索引这一步短暂持锁（2026-09-12 拆锁）——批量抓封面时不再让
                // 每一张图都占着全局 DB 锁，前端其它查询不会被堵住。
                // 抓取器落盘的中间文件由 cover_store 在入库成功后清理（delete_src = true）。
                let src = std::path::Path::new(&cover_path);
                let prepared_result = cover_store::prepare_path(OWNER_GAME, &game.id, src);
                let update_result = prepared_result.and_then(|prepared| {
                    let db_guard = lock_or_recover(&db);
                    let set = cover_store::commit_prepared(&db_guard, OWNER_GAME, &game.id, prepared)?;
                    cover_store::cleanup_ingest_source(&db_guard, src, &set, true)?;
                    Ok(())
                });
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
    // 先落到 covers 目录下的临时文件（download_from_url 会按实际图片格式定扩展名），
    // 再由 cover_store 统一入库并清理这个中间文件
    let save_path = covers_dir.join(format!(".{}.picked", game_id));

    let fetcher = CoverFetcher::new(covers_dir.clone(), api_key).map_err(|e| e.to_string())?;
    let actual_path = fetcher.download_from_url(&url, &save_path)
        .await
        .map_err(|e| format!("下载封面失败: {}", e))?;

    // 同样拆锁（2026-09-12）：解码/编码在无锁段完成，只有写索引时短暂持锁
    let prepared = cover_store::prepare_path(OWNER_GAME, &game_id, &actual_path)
        .map_err(|e| e.to_string())?;
    let db_guard = lock_or_recover(&db);
    let set = cover_store::commit_prepared(&db_guard, OWNER_GAME, &game_id, prepared)
        .map_err(|e| e.to_string())?;
    // 中间文件入库成功后清理（共享/原地替换的保护规则在函数内）
    let _ = cover_store::cleanup_ingest_source(&db_guard, &actual_path, &set, true);
    Ok(())
}

/// 将 LLM 返回的元数据合并到游戏对象（只更新非空字段，保留用户已有数据）
/// 供单游戏刷新与批量刷新共用
fn apply_llm_meta(updated: &mut Game, meta: &LlmGameMeta) {
    if let Some(name) = &meta.name {
        let trimmed = name.trim().to_string();
        if !trimmed.is_empty() && trimmed != updated.name {
            tracing::info!("LLM 纠正游戏名称: '{}' -> '{}'", updated.name, trimmed);
            updated.name = trimmed;
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
    if !meta.save_paths.is_empty() {
        updated.save_paths = meta.save_paths.clone();
    }
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
    let fetcher = LlmFetcher::new().map_err(|e| format!("创建 LLM 客户端失败: {}", e))?;
    let meta = fetcher.fetch_game_meta(&config, &game.name)
        .await
        .map_err(|e| format!("LLM 获取游戏信息失败: {}", e))?;

    // 重新获取锁更新游戏数据
    let db_guard = lock_or_recover(&db);

    // 重新从数据库读取最新数据，避免覆盖 LLM 请求期间用户的并发修改
    let mut updated = db_guard.get_game_by_id(&game_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "游戏不存在".to_string())?;
    apply_llm_meta(&mut updated, &meta);

    db_guard.update_game(&updated).map_err(|e| e.to_string())?;
    // LLM 补全计数（成就 G-13）
    let _ = db_guard.increment_llm_filled_count();
    Ok(updated)
}

/// 手动更新游戏元数据（与 LLM 获取的字段一致）
/// hltb 参数用 Option<Option<u32>>：外层 Some 表示字段被提交，内层 Some(v) 设置值、None 表示清空
#[tauri::command]
pub fn update_game_meta(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
    description: Option<String>,
    developer: Option<String>,
    publisher: Option<String>,
    release_date: Option<String>,
    genres: Option<Vec<String>>,
    hltb_main_story: Option<Option<u32>>,
    hltb_main_extra: Option<Option<u32>>,
    hltb_completionist: Option<Option<u32>>,
    save_paths: Option<Vec<String>>,
) -> Result<Game, String> {
    let db_guard = lock_or_recover(&db);
    let mut game = db_guard.get_game_by_id(&game_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "游戏不存在".to_string())?;

    if let Some(v) = description {
        game.description = if v.is_empty() { None } else { Some(v) };
    }
    if let Some(v) = developer {
        game.developer = if v.is_empty() { None } else { Some(v) };
    }
    if let Some(v) = publisher {
        game.publisher = if v.is_empty() { None } else { Some(v) };
    }
    if let Some(v) = release_date {
        game.release_date = if v.is_empty() { None } else { Some(v) };
    }
    if let Some(v) = genres {
        // 手动填写也走归一化（与 apply_llm_meta / apply_meta_to_review 口径一致），
        // 否则 NSelect 自由输入仍能造出「Action」这类非规范类型
        game.genres = crate::core::genres::normalize_genres(&v);
    }
    // Some(Some(v)) 设置值，Some(None) 清空（用户清空输入框），None 不处理
    if let Some(v) = hltb_main_story {
        game.hltb_main_story = v;
    }
    if let Some(v) = hltb_main_extra {
        game.hltb_main_extra = v;
    }
    if let Some(v) = hltb_completionist {
        game.hltb_completionist = v;
    }
    if let Some(v) = save_paths {
        game.save_paths = v;
    }

    db_guard.update_game(&game).map_err(|e| e.to_string())?;
    Ok(game)
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

    // 获取并保存文件元数据（用于后续缓存判断）
    if let Some(metadata) = utils::path::get_file_metadata(&new_exe_path) {
        game.exe_modified_at = Some(metadata.modified_at);
        game.exe_file_size = Some(metadata.file_size);
    } else {
        game.exe_modified_at = None;
        game.exe_file_size = None;
    }

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

/// 从主备份文件路径推导独立密钥文件路径（xxx.json -> xxx.keys.json）
/// 后缀剥离大小写不敏感（.json / .Json / .JSON 都命中），否则用户手输混合大小写
/// 文件名会得到 backup.Json.keys.json 这类怪名。
fn derive_keys_path(file_path: &str) -> String {
    let stripped = if file_path.to_lowercase().ends_with(".json") {
        // ".json" 是 5 个 ASCII 字节：从末尾截断必然落在字符边界
        // （后缀之前即使紧贴 CJK 等多字节字符，截点也在其完整字节之后）
        &file_path[..file_path.len() - ".json".len()]
    } else {
        file_path
    };
    format!("{}.keys.json", stripped)
}

/// 导出游戏库数据为 JSON 文件。
/// 主备份（file_path）脱敏 API Key；另存独立密钥文件（xxx.keys.json），
/// 与主数据隔离，避免分享主备份时把密钥一并泄露出去。
#[tauri::command]
pub fn export_game_data(
    db: State<'_, Arc<Mutex<Database>>>,
    file_path: String,
) -> Result<serde_json::Value, String> {
    let db_guard = lock_or_recover(&db);

    // 获取所有游戏数据
    let filter = GameFilter::default();
    let games = db_guard.get_games(&filter).map_err(|e| e.to_string())?;

    // 获取设置
    let settings = Settings::load_from_db(&db_guard).map_err(|e| e.to_string())?;

    // 提前取出两个密钥（后续要写入独立文件）
    let sgdb_key = settings.steamgriddb_api_key.clone();
    let llm_key = settings.llm_api_key.clone();

    // 构建导出数据结构（脱敏 API Key，避免用户分享备份时泄露密钥）
    let sanitized_settings = serde_json::json!({
        "theme": settings.theme,
        "language": settings.language,
        "accent_color": settings.accent_color,
        "steamgriddb_api_key": "",
        "llm_protocol": settings.llm_protocol,
        "llm_api_key": "",
        "llm_base_url": settings.llm_base_url,
        "llm_model": settings.llm_model,
        "llm_enabled": settings.llm_enabled,
        "window_width": settings.window_width,
        "window_height": settings.window_height,
        "screenshot_dir": settings.screenshot_dir,
        "screenshot_hotkey": settings.screenshot_hotkey,
    });
    let export_data = serde_json::json!({
        "version": "1.0",
        "exported_at": chrono::Utc::now().to_rfc3339(),
        "games": games,
        "settings": sanitized_settings,
    });

    // 序列化并写入主备份
    let json = serde_json::to_string_pretty(&export_data)
        .map_err(|e| format!("序列化失败: {}", e))?;
    std::fs::write(&file_path, json)
        .map_err(|e| format!("写入主备份失败: {}", e))?;

    // 独立密钥文件（敏感信息，单独存放、单独标注）
    let keys_json = serde_json::to_string_pretty(&serde_json::json!({
        "type": "gamevault-api-keys",
        "description": "GameVault API 密钥备份（敏感信息，请勿分享或上传到公开位置）",
        "steamgriddb_api_key": sgdb_key,
        "llm_api_key": llm_key,
    }))
    .map_err(|e| format!("序列化密钥失败: {}", e))?;
    let keys_path = derive_keys_path(&file_path);
    std::fs::write(&keys_path, keys_json)
        .map_err(|e| format!("写入密钥文件失败: {}", e))?;

    Ok(serde_json::json!({
        "main_path": file_path,
        "keys_path": keys_path,
    }))
}

/// 导入游戏库数据（从 JSON 备份文件恢复，自动识别同目录的密钥文件）
#[tauri::command]
pub fn import_game_data(
    db: State<'_, Arc<Mutex<Database>>>,
    file_path: String,
) -> Result<serde_json::Value, String> {
    let json_data = std::fs::read_to_string(&file_path)
        .map_err(|e| format!("读取备份文件失败: {}", e))?;

    let import_data: serde_json::Value = serde_json::from_str(&json_data)
        .map_err(|e| format!("JSON 解析失败: {}", e))?;

    let db_guard = lock_or_recover(&db);

    // 导入游戏
    let mut imported_games = 0u32;
    if let Some(games_array) = import_data["games"].as_array() {
        for game_json in games_array {
            match serde_json::from_value::<Game>(game_json.clone()) {
                Ok(game) => {
                    // 校验 game_id 格式（必须为合法 UUID，防止路径遍历）
                    if uuid::Uuid::parse_str(&game.id).is_err() {
                        tracing::warn!("跳过无效 game_id 的游戏: {}", game.id);
                        continue;
                    }
                    // 封面策略：upsert_game 的 ON CONFLICT 对 cover 字段使用
                    // COALESCE(excluded, games) —— 导入时若本地已有该游戏，保留现有封面；
                    // 其他机器导出的封面路径不会生效（文件不存在时前端自动显示占位并可由"刷新封面"重新获取）
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

    // 自动恢复密钥文件（若存在）。只恢复非空值，避免空值覆盖现有密钥
    let mut keys_restored = 0u32;
    let keys_path = derive_keys_path(&file_path);
    if let Ok(keys_data) = std::fs::read_to_string(&keys_path) {
        if let Ok(keys_json) = serde_json::from_str::<serde_json::Value>(&keys_data) {
            if let Some(key) = keys_json["steamgriddb_api_key"].as_str() {
                if !key.is_empty() {
                    match db_guard.set_setting("steamgriddb_api_key", key) {
                        Ok(_) => keys_restored += 1,
                        Err(e) => tracing::warn!("恢复 SteamGridDB 密钥失败: {}", e),
                    }
                }
            }
            if let Some(key) = keys_json["llm_api_key"].as_str() {
                if !key.is_empty() {
                    match db_guard.set_setting("llm_api_key", key) {
                        Ok(_) => keys_restored += 1,
                        Err(e) => tracing::warn!("恢复 LLM 密钥失败: {}", e),
                    }
                }
            }
        }
    }

    Ok(serde_json::json!({
        "imported_games": imported_games,
        "settings_restored": settings_restored,
        "keys_restored": keys_restored,
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

/// 打开存档路径（在文件管理器中）
/// 如果精确路径不存在，自动向上查找存在的游戏级父文件夹并打开
#[tauri::command]
pub async fn open_save_path(path: String, app_handle: tauri::AppHandle) -> Result<(), String> {
    /// 系统/用户级大文件夹名称（与 check_save_paths 保持一致）
    const GENERIC_DIRS: &[&str] = &[
        "Documents", "文档", "My Documents",
        "AppData", "Local", "Roaming", "LocalLow",
        "ProgramData", "Program Files", "Program Files (x86)",
        "Users", "Windows", "System32",
        "Saved Games", "AppDataLocal", "AppDataRoaming",
    ];

    let expanded = utils::path::expand_env_vars(&path);
    let path = std::path::PathBuf::from(&expanded);

    // 如果精确路径不存在，向上查找存在的游戏级父文件夹
    let target = if path.exists() {
        if path.is_file() {
            path.parent().unwrap_or(&path).to_path_buf()
        } else {
            path
        }
    } else {
        let mut current = path.as_path();
        let mut found = None;
        loop {
            if let Some(parent) = current.parent() {
                if parent == current { break; }
                // 遇到系统级大文件夹就停止
                if let Some(name) = current.file_name().and_then(|n| n.to_str()) {
                    if GENERIC_DIRS.iter().any(|g| g.eq_ignore_ascii_case(name)) {
                        break;
                    }
                }
                if parent.exists() {
                    found = Some(parent.to_path_buf());
                    break;
                }
                current = parent;
            } else {
                break;
            }
        }
        match found {
            Some(p) => p,
            None => return Err(format!("路径不存在: {}", expanded)),
        }
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

/// 系统/用户级大文件夹名称，不应视为"存档存在"
const GENERIC_DIRS: &[&str] = &[
    "Documents", "文档", "My Documents",
    "AppData", "Local", "Roaming", "LocalLow",
    "ProgramData", "Program Files", "Program Files (x86)",
    "Users", "Windows", "System32",
    "Saved Games", "AppDataLocal", "AppDataRoaming",
];

/// 检查路径或其游戏级父文件夹是否存在（向上查找直到遇到系统级大文件夹为止）
fn save_path_effectively_exists(path: &str) -> bool {
    let p = std::path::Path::new(path);
    let mut current = p;
    loop {
        if current.exists() {
            return true;
        }
        match current.parent() {
            Some(parent) if parent != current => {
                if let Some(name) = current.file_name().and_then(|n| n.to_str()) {
                    if GENERIC_DIRS.iter().any(|g| g.eq_ignore_ascii_case(name)) {
                        return false;
                    }
                }
                current = parent;
            }
            _ => return false,
        }
    }
}

/// 检查所有游戏的存档路径是否存在
/// 返回 game_id -> bool 的映射，true 表示至少有一条存档路径（或其游戏级父文件夹）存在
#[tauri::command]
pub fn check_save_paths(
    db: State<'_, Arc<Mutex<Database>>>,
) -> Result<std::collections::HashMap<String, bool>, String> {
    use crate::models::GameFilter;
    use crate::utils::path::expand_env_vars;

    let db_guard = lock_or_recover(&db);
    let games = db_guard.get_games(&GameFilter::default()).map_err(|e| e.to_string())?;

    let mut result = std::collections::HashMap::new();

    for game in games {
        let exists = if game.save_paths.is_empty() {
            false
        } else {
            game.save_paths.iter().any(|p| {
                let expanded = expand_env_vars(p);
                save_path_effectively_exists(&expanded)
            })
        };
        result.insert(game.id, exists);
    }

    Ok(result)
}

/// 检查单个游戏的存档路径是否存在（编辑存档路径后局部刷新用，避免全量检查）
#[tauri::command]
pub fn check_save_paths_for_game(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
) -> Result<bool, String> {
    use crate::utils::path::expand_env_vars;

    let db_guard = lock_or_recover(&db);
    let game = db_guard.get_game_by_id(&game_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "游戏不存在".to_string())?;

    if game.save_paths.is_empty() {
        return Ok(false);
    }

    let exists = game.save_paths.iter().any(|p| {
        let expanded = expand_env_vars(p);
        save_path_effectively_exists(&expanded)
    });
    Ok(exists)
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
            .compression_method(zip::CompressionMethod::Deflated)
            .large_file(true); // 存档目录常有 >4GB 文件，必须启用 zip64
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

    // ZIP 打包是大文件密集 I/O，放阻塞线程池执行，避免长时间占死 async 运行时线程
    tauri::async_runtime::spawn_blocking(move || export_saves_backup_sync(&games, &export_path))
        .await
        .map_err(|e| format!("导出任务执行失败: {}", e))?
}

/// 导出存档备份的同步实现（在阻塞线程池中运行）
fn export_saves_backup_sync(
    games: &[Game],
    export_path: &str,
) -> Result<serde_json::Value, String> {
    let file = std::fs::File::create(export_path)
        .map_err(|e| format!("创建 ZIP 文件失败: {}", e))?;
    let buf_writer = std::io::BufWriter::new(file);
    let mut zip = zip::ZipWriter::new(buf_writer);

    let mut manifest: Vec<serde_json::Value> = Vec::new();
    let mut exported_count = 0u32;
    let mut errors: Vec<String> = Vec::new();

    for game in games {
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
            // 追加 game_id 前 8 位做去重，防止同名游戏（或安全化后同名）在 ZIP 内互相覆盖
            let safe_name = game.name.chars()
                .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
                .collect::<String>();
            let id_hint = game.id.chars().take(8).collect::<String>();
            let zip_prefix = if game.save_paths.len() > 1 {
                format!("{}_{}_{}", safe_name, idx + 1, id_hint)
            } else {
                format!("{}_{}", safe_name, id_hint)
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
    let options = FileOptions::default().large_file(true);
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
    // 解压同样是文件密集 I/O，放阻塞线程池执行
    tauri::async_runtime::spawn_blocking(move || import_saves_backup_sync(&zip_path))
        .await
        .map_err(|e| format!("导入任务执行失败: {}", e))?
}

/// 导入存档备份的同步实现（在阻塞线程池中运行）
fn import_saves_backup_sync(zip_path: &str) -> Result<serde_json::Value, String> {
    let file = std::fs::File::open(zip_path)
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
    // errors 提前声明：manifest 解析阶段的建目录失败也要对前端可见，不能静默跳过
    let mut errors: Vec<String> = Vec::new();
    let mut extract_plan: Vec<(String, std::path::PathBuf, String)> = Vec::new();
    for entry in &manifest {
        let original_path = entry["original_path"].as_str().unwrap_or("");
        let zip_prefix = entry["zip_prefix"].as_str().unwrap_or("");

        if original_path.is_empty() || zip_prefix.is_empty() {
            continue;
        }

        let expanded = utils::path::expand_env_vars(original_path);
        let target_path = std::path::PathBuf::from(&expanded);

        // 创建目标目录本身（原存档目录不存在时也要能恢复——换机/存档丢失场景）。
        // 必须建出 target_path，否则下方 is_safe 的 canonicalize 校验会因目录不存在而误判"不安全"，
        // 导致所有文件被跳过、restored 却虚报成功。
        // 注意：save_path 可能指向单个文件（add_path_to_zip 支持 is_file），目标已存在且是文件时
        // create_dir_all 必然失败——此时跳过建目录（下方 is_safe 走 dest.canonicalize() 直比分支）；
        // 其余建目录失败记入 errors 后跳过，保持错误对前端可见。
        if !target_path.is_file() {
            if let Err(e) = std::fs::create_dir_all(&target_path) {
                errors.push(format!("创建存档目标目录失败 {}: {}", target_path.display(), e));
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
                // 路径不存在（新文件），先创建其父目录再做安全检查，
                // 否则嵌套子目录尚未创建时 canonicalize 会失败，误判"不安全"而跳过文件
                let parent = dest.parent().unwrap_or(&dest);
                if std::fs::create_dir_all(parent).is_err() {
                    false
                } else if let Ok(canonical_parent) = parent.canonicalize() {
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
            extract_plan.push((file_name.to_string(), dest, zip_prefix.to_string()));
        }
    }

    // 一次性遍历计划，逐个从 archive 中提取（archive 只打开一次）
    // 成功恢复的存档路径（zip_prefix）集合，用于精确计数而非按 manifest 条目数虚报
    let mut restored_prefixes: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (file_name, dest, zip_prefix) in extract_plan {
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
            } else {
                restored_prefixes.insert(zip_prefix);
            }
        }
    }

    // 恢复计数 = 实际成功写出文件的存档路径数
    let restored_count = restored_prefixes.len() as u32;

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
    let fetcher = match LlmFetcher::new() {
        Ok(f) => f,
        Err(e) => return Err(format!("创建 LLM 客户端失败: {}", e)),
    };
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
        match fetcher.fetch_game_meta(&config, &game.name).await {
            Ok(meta) => {
                // 将获取的信息更新到游戏数据（只更新非空字段，保留用户已有的数据）
                // 使用闭包限制 ? 传播：单个游戏失败不应中断整个批量处理
                let update_result: Result<(), String> = (|| {
                    let db_guard = lock_or_recover(&db);

                    // 重新从数据库读取最新数据，避免覆盖 LLM 请求期间用户的并发修改
                    let mut updated = db_guard.get_game_by_id(&game.id)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| "游戏不存在".to_string())?;
                    apply_llm_meta(&mut updated, &meta);

                    db_guard.update_game(&updated).map_err(|e| e.to_string())?;
                    // LLM 补全计数（成就 G-13）
                    let _ = db_guard.increment_llm_filled_count();
                    Ok(())
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
