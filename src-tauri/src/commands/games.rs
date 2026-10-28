use tauri::{State, Emitter};
use tauri_plugin_opener::OpenerExt;
use std::sync::{Arc, Mutex};
use crate::core::{Database, PlayTimeTracker, GameLauncher};
use crate::core::launcher::LaunchOutcome;
use crate::core::cover_fetcher::CoverFetcher;
use crate::core::cover_store;
use crate::models::cover::{OWNER_GAME, OWNER_REVIEW};
use crate::core::llm_fetcher::{LlmFetcher, LlmConfig, LlmProtocol, LlmGameMeta};
use crate::models::*;
use crate::models::settings::Settings;
use crate::utils;
use crate::utils::constants::{COVER_BATCH_INTERVAL_MS, COVER_MIN_FILE_SIZE};
use super::lock_or_recover;

/// 获取游戏列表
#[tauri::command(async)]
pub fn get_games(
    db: State<'_, Arc<Mutex<Database>>>,
    filter: Option<GameFilter>,
) -> Result<Vec<Game>, String> {
    let db = lock_or_recover(&db);
    let filter = filter.unwrap_or_default();
    db.get_games(&filter).map_err(|e| e.to_string())
}

/// 获取单个游戏详情
#[tauri::command(async)]
pub fn get_game_detail(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
) -> Result<Option<Game>, String> {
    let db = lock_or_recover(&db);
    db.get_game_by_id(&game_id).map_err(|e| e.to_string())
}

/// 启动游戏
#[tauri::command(async)]
pub fn launch_game(
    db: State<'_, Arc<Mutex<Database>>>,
    tracker: State<'_, Arc<Mutex<PlayTimeTracker>>>,
    wakeup: State<'_, Arc<crate::PollWakeup>>,
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

    // 叫醒后台轮询线程（2026-09-19）。
    // 空闲期它正阻塞在条件变量上；不叫醒就要等最多 PROCESS_POLL_IDLE_SECS（10 秒）
    // 才会开始找进程——平台游戏那段等待最明显：点了启动，界面十几秒没反应。
    // 本地游戏虽已即时计时，但退出检测同样要等它醒来才开始。
    wakeup.notify();

    Ok(())
}

/// 切换收藏状态
#[tauri::command(async)]
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
#[tauri::command(async)]
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
        // （名字 + 官方英文名 + exe 文件名），重装再入库时按名字认领、复用旧 id，孤儿历史统计即可自动续接。
        // 路径刻意不存——换盘/挪目录重装照样能认领。墓碑写失败不应阻断删除，仅告警。
        // name_en 必须一起留档（2026-10-09）：`name` 会被 LLM 刷新/就地编辑改成中文，
        // 而重装导入的物品名是英文原名，只靠 `name` 认领会断链（见 database.rs::find_tombstone_for_reclaim）。
        if let Some(g) = &game {
            if let Err(e) = db_guard.insert_tombstone(
                &g.id,
                &g.name,
                g.name_en.as_deref(),
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
#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
pub fn set_game_cover(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
    cover_path: String,
) -> Result<(), String> {
    let src = std::path::Path::new(&cover_path);
    if !src.exists() {
        return Err("选择的图片文件不存在".to_string());
    }

    // 先校验主体存在（2026-09-23 加）：否则会给一个已经删掉的 game_id 写进封面索引与
    // 文件，而那条"孤儿"索引没有任何界面能读到、也没有任何清理逻辑会碰它 ——
    // 它自己就是那条引用，引用计数永远 ≥1，对应的文件永远删不掉。
    {
        let db_guard = lock_or_recover(&db);
        db_guard
            .get_game_by_id(&game_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "游戏不存在".to_string())?;
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
#[tauri::command(async)]
pub fn remove_game_cover(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
) -> Result<(), String> {
    let db_guard = lock_or_recover(&db);
    cover_store::purge(&db_guard, OWNER_GAME, &game_id).map_err(|e| e.to_string())?;
    Ok(())
}

/// 获取所有主体（游戏 + 手账）的有效封面路径（供前端通过 asset 协议加载）
///
/// 返回 `{ "<owner_kind>:<owner_id>": { main, thumb, updated_at } }`：
/// - **含已移除游戏的留档封面**（state=archived）——时长排行里已删条目照样显示图片；
/// - `thumb` 供卡片网格/排行等小尺寸场景使用，避免整张原图参与解码；
/// - `updated_at` 是主图**最后一次被替换**的时刻，前端拼进 asset URL 当破缓存参数
///   （换封面原地覆盖同名文件，URL 不变会被 WebView 图片缓存吃掉）。
/// - 索引缺失但 `cover_local` 指向的文件确实存在时兜底返回（迁移未跑完/外部放入）。
///
/// **为什么带上手账**（2026-09-24）：原先只返回 game，手账侧取图靠 `review.cover_local`
/// + `review.updated_at` 拼参数，而 `updated_at` 是**条目**改动时刻（改评分/改名都会动），
/// 会让 URL 无谓抖动。统一从这里取「封面行」的时间戳，两侧语义才一致。
#[tauri::command(async)]
pub fn get_all_covers(
    db: State<'_, Arc<Mutex<Database>>>,
) -> Result<std::collections::HashMap<String, crate::models::CoverSet>, String> {
    let db_guard = lock_or_recover(&db);

    /// 拼键：与前端 `coverKey()` 必须一致
    fn key(owner_kind: &str, owner_id: &str) -> String {
        format!("{}:{}", owner_kind, owner_id)
    }

    let mut covers: std::collections::HashMap<String, crate::models::CoverSet> =
        std::collections::HashMap::new();

    // 游戏走旧口径的裸 id 键（前端已有大量 `coverPaths[gameId]` 消费点，不动它）；
    // 手账走 `review:<id>` 命名键，避免与游戏 id 撞键（两者都是 uuid，撞的概率极低
    // 但语义上本就该分开）。
    for row in db_guard.all_cover_rows().map_err(|e| e.to_string())? {
        let abs = cover_store::abs_path(&row.rel_path);
        if !abs.exists() {
            continue;
        }
        let path = abs.to_string_lossy().to_string();
        let map_key = match row.owner_kind.as_str() {
            OWNER_GAME => row.owner_id.clone(),
            OWNER_REVIEW => key(OWNER_REVIEW, &row.owner_id),
            _ => continue,
        };
        let entry = covers.entry(map_key).or_default();
        match row.kind.as_str() {
            crate::models::KIND_MAIN => {
                entry.main = Some(path);
                // 时间戳跟随主图行
                entry.updated_at = Some(row.updated_at.clone());
            }
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
                    // 无索引行可取时间戳 —— 交给前端回退到条目自身的 updated_at
                    updated_at: None,
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

        // 批量抓取时给 SteamGridDB 留出间隔：免费层限流约 1 req/s，而每个游戏
        // 要打 2 次 API + 1 次图床，连发很容易叠出 429（2026-10-18 加）。
        if index > 0 {
            crate::utils::net::sleep_ms(COVER_BATCH_INTERVAL_MS).await;
        }

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
                // 走到这里说明 SteamGridDB 给了**明确答复**（未收录），
                // 网络失败会走下面的 Err 分支——两种结局不再混在一句话里。
                tracing::warn!("未找到封面: {} (SteamGridDB 未收录该游戏)", game.name);
                errors.push(format!("{}: SteamGridDB 未收录，请检查游戏名称或手动设置封面", game.name));
            }
            Err(e) => {
                tracing::warn!("获取封面失败 {}: {}", game.name, e);
                errors.push(format!("{}: {}", game.name, e));
                // `fetch_cover` 只在「一次明确答复都没拿到」时返回 Err（网络/接口全灭），
                // 后面的游戏大概率同样失败 —— 直接收工，别让用户对着转圈等半小时。
                let remaining = total_missing.saturating_sub(index as u32 + 1);
                if remaining > 0 {
                    tracing::warn!("网络异常，中止批量抓取，剩余 {} 个未尝试", remaining);
                    errors.push(format!(
                        "网络异常，已中止：剩余 {} 个游戏未尝试，请检查网络或代理后重试",
                        remaining
                    ));
                }
                break;
            }
        }
    }

    Ok(serde_json::json!({
        "fetched": fetched_count,
        "total": total_missing,
        "errors": errors,
    }))
}

/// 获取游戏的所有可选封面（仅 SteamGridDB；Steam 官方封面走独立命令）
#[tauri::command]
pub async fn fetch_cover_options(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
) -> Result<Vec<CoverOption>, String> {
    let (game_name, name_en, install_path, api_key) = {
        let db_guard = lock_or_recover(&db);
        let game = db_guard.get_game_by_id(&game_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "游戏不存在".to_string())?;
        let api_key = db_guard.get_setting("steamgriddb_api_key")
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        (game.name, game.name_en, game.install_path, api_key)
    };

    if api_key.is_empty() {
        return Err("未配置 SteamGridDB API Key，请在设置中填写".to_string());
    }

    let cache_dir = utils::path::get_app_data_dir().join("covers");
    let fetcher = CoverFetcher::new(cache_dir, api_key).map_err(|e| e.to_string())?;

    fetcher
        .fetch_cover_options(&game_name, name_en.as_deref(), install_path.as_deref())
        .await
        .map_err(|e| e.to_string())
}

/// 把已下载到 covers 目录的中间文件入库（下载 → 入库 → 清理中间文件）
///
/// 抽出来给「从 URL 设封面」与「用 Steam 官方封面」共用：两条路的差别只在
/// 下载来源，入库规则（引用计数、去重、清理）必须完全一致。
fn ingest_downloaded_cover(
    db: &State<'_, Arc<Mutex<Database>>>,
    game_id: &str,
    actual_path: &std::path::Path,
) -> Result<(), String> {
    // 拆锁（2026-09-12）：解码/编码在无锁段完成，只有写索引时短暂持锁
    let prepared =
        cover_store::prepare_path(OWNER_GAME, game_id, actual_path).map_err(|e| e.to_string())?;
    let db_guard = lock_or_recover(db);
    let set = cover_store::commit_prepared(&db_guard, OWNER_GAME, game_id, prepared)
        .map_err(|e| e.to_string())?;
    // 中间文件入库成功后清理（共享/原地替换的保护规则在函数内）
    let _ = cover_store::cleanup_ingest_source(&db_guard, actual_path, &set, true);
    Ok(())
}

/// 使用 Steam 官方封面（按 appid 直取 Valve 图床，不经过 SteamGridDB）
#[tauri::command]
pub async fn set_game_cover_from_steam(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
) -> Result<(), String> {
    let (steam_url, api_key) = {
        let db_guard = lock_or_recover(&db);
        let game = db_guard.get_game_by_id(&game_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "游戏不存在".to_string())?;
        let steam_url = crate::core::cover_fetcher::steam_cover_url_for(&game).ok_or_else(|| {
            "未能识别 Steam AppID：该条目不是 Steam 游戏，安装目录里也没有 appid 线索文件"
                .to_string()
        })?;
        // 下载客户端要 API Key 只是为了构造，Steam 图床不需要它
        let api_key = db_guard.get_setting("steamgriddb_api_key")
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        (steam_url, api_key)
    };

    let covers_dir = utils::path::get_covers_dir();
    utils::path::ensure_dir_exists(&covers_dir).map_err(|e| e.to_string())?;
    let save_path = covers_dir.join(format!(".{}.steam", game_id));

    let fetcher = CoverFetcher::new(covers_dir.clone(), api_key).map_err(|e| e.to_string())?;
    let actual_path = fetcher
        .download_from_url(&steam_url, &save_path)
        .await
        .map_err(|e| format!("下载 Steam 官方封面失败: {e}"))?;

    ingest_downloaded_cover(&db, &game_id, &actual_path)
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

    ingest_downloaded_cover(&db, &game_id, &actual_path)
}

/// 将 LLM 返回的元数据合并到游戏对象（只更新非空字段，保留用户已有数据）
/// 供单游戏刷新与批量刷新共用
///
/// **仅 LLM 路径会走到这里**（手动改名走 `rename_game_inner`，不经过本函数），
/// 因此这里可以安全地加「LLM 名称语言闸门」而不影响用户显式改名的自由。
fn apply_llm_meta(updated: &mut Game, meta: &LlmGameMeta) {
    // 名称语言闸门（2026-10-09，与手账 reviews.rs::drop_english_name_for_chinese_entry 同口径）：
    // `name` 的契约是「中文主名」，但 LLM 面对英文原名的游戏常把官方英文名原样回填，
    // 一旦落库就会把已有的中文名顶成英文；紧接着下方「ASCII 名自动补 name_en」又会把
    // 两个字段填成同一个英文串 —— 界面上「中文名 / 英文名」双栏一起变英文，正是老爷报的症状。
    // 判据：**纯 ASCII 的 name 无权覆盖含非 ASCII 的现名**。反向不受限（中文名可以顶掉英文名，
    // 英文→英文的纠错也照常，如 'Gears of War Reloaded' → 'Gears of War: Reloaded'）。
    if let Some(name) = &meta.name {
        let trimmed = name.trim().to_string();
        if !trimmed.is_empty() && trimmed != updated.name {
            if trimmed.is_ascii() && !updated.name.is_ascii() {
                tracing::warn!(
                    "LLM 返回的 name 为纯 ASCII，拒绝覆盖现有中文名: '{}'（保留 {:?}）",
                    trimmed,
                    updated.name
                );
            } else {
                tracing::info!("LLM 纠正游戏名称: '{}' -> '{}'", updated.name, trimmed);
                updated.name = trimmed;
            }
        }
    }
    // 官方英文名（SteamGridDB 检索专用）：落库前 trim，空串按 None 归一。
    // 不能只靠 LLM 每次都给——纯英文名的游戏本来就等于 name，据此兜底一次，
    // 保证 name_en 在「名称为纯 ASCII」时也有值（与手账 normalize_review_name 同口径）。
    //
    // **语言闸门（2026-10-06，与手账 reviews.rs 同口径）**：`name_en` 的契约是「官方英文名」，
    // 含非 ASCII 字符（中文/日文等）一律拒收。LLM 常偷懒把中文名原样复制进 name_en
    // （如 name 与 name_en 都填「空洞骑士」），一旦落库：
    //   ① `cover_fetcher::pick_search_term` 会拿这个中文脏值去 SteamGridDB 检索 → 必然 0 命中，
    //      封面链路退化到"不如不填"；
    //   ② 卡片「英文名优先展示」会显示成中文，正是 2026-10-05 那条修复想消灭的错配。
    // 注意空串分支**保持原样不动**（不清除）——游戏侧没有手账那种"空串=显式清除"的语义，
    // 凭空清掉用户手填的英文名属于功能倒退。
    if let Some(en) = &meta.name_en {
        let trimmed = en.trim().to_string();
        if !trimmed.is_empty() {
            if !trimmed.is_ascii() {
                tracing::warn!(
                    "LLM 返回的 name_en 含非 ASCII 字符，已丢弃: '{}'（保留原值 {:?}）",
                    trimmed,
                    updated.name_en
                );
            } else if trimmed != updated.name_en.as_deref().unwrap_or("") {
                tracing::info!("更新游戏英文名: '{:?}' -> '{}'", updated.name_en, trimmed);
                updated.name_en = Some(trimmed);
            }
        }
    }
    if updated.name_en.is_none() && updated.name.is_ascii() {
        updated.name_en = Some(updated.name.clone());
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
///
/// 【清空语义（2026-09-22 修复，0.8.3 只修了设值这半边）】Tauri 命令参数层会把
/// JSON `null` 折叠成最外层 `None`（tauri-2.11.2 `src/ipc/command.rs` 的
/// `deserialize_option`：键存在时交给 serde_json，而 `Value::Null` 一律 `visit_none`），
/// 因此**清空不能用 null 表达**——前端传 `hltbMainStory: null` 得到的是"没提交"而非
/// "清空"。各字段的清空哨兵（必须是有效 JSON 值，且能穿过 serde）：
/// - 文本字段（description / developer / publisher / release_date）：**空串**；
/// - genres / save_paths：**空数组**；
/// - hltb 三项：**`0`**（0 分钟时长本身即无效值；输入框 `min=1`，0 只能由清空产生）；
/// - launch_args：空串（trim 后为空）。
/// 未提交的字段（`None`，键缺失或值为 null 都会归到这）保持现值不变。
// 参数对应前端 Tauri 命令调用契约，抽结构体重构会改变序列化签名、破坏前端调用，故此处放行。
#[allow(clippy::too_many_arguments)]
#[tauri::command(async)]
pub fn update_game_meta(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
    description: Option<String>,
    developer: Option<String>,
    publisher: Option<String>,
    release_date: Option<String>,
    genres: Option<Vec<String>>,
    hltb_main_story: Option<u32>,
    hltb_main_extra: Option<u32>,
    hltb_completionist: Option<u32>,
    save_paths: Option<Vec<String>>,
    launch_args: Option<String>,
    name_en: Option<String>,
) -> Result<Game, String> {
    apply_game_meta_update(
        db.inner(),
        &game_id,
        description,
        developer,
        publisher,
        release_date,
        genres,
        hltb_main_story,
        hltb_main_extra,
        hltb_completionist,
        save_paths,
        launch_args,
        name_en,
    )
}

/// `update_game_meta` 的函数体（命令层只负责剥 State 壳）。
/// 单独抽出是为了能直接用 `Arc<Mutex<Database>>` 做单元测试——清空语义
/// （见命令层注释）有 `game_meta_clear_and_sentinel_semantics` 测试守着。
#[allow(clippy::too_many_arguments)]
fn apply_game_meta_update(
    db: &Arc<Mutex<Database>>,
    game_id: &str,
    description: Option<String>,
    developer: Option<String>,
    publisher: Option<String>,
    release_date: Option<String>,
    genres: Option<Vec<String>>,
    hltb_main_story: Option<u32>,
    hltb_main_extra: Option<u32>,
    hltb_completionist: Option<u32>,
    save_paths: Option<Vec<String>>,
    launch_args: Option<String>,
    name_en: Option<String>,
) -> Result<Game, String> {
    let db_guard = lock_or_recover(db);
    let mut game = db_guard.get_game_by_id(game_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "游戏不存在".to_string())?;

    // 文本字段：空串 = 清空（Some(空串) 是有效 JSON，能穿过 Tauri 参数层）
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
        // 否则 NSelect 自由输入仍能造出「Action」这类非规范类型；
        // 空数组归一化后仍为空数组 → 等价清空（Game.genres 是 Vec，非 Option）
        game.genres = crate::core::genres::normalize_genres(&v);
    }
    // hltb 三项：0 = 清空（0 分钟时长本身即无效值；输入框 min=1，0 只能由
    // 前端把"清空的数字框"（null）转成 0 得到）。
    // 【不能改回 Option<Option<u32>>】Tauri 会把 JSON null 折叠成外层 None，
    // "内层 None 清空"在参数层根本不可达（0.8.3 的教训，详见命令层注释）。
    if let Some(v) = hltb_main_story {
        game.hltb_main_story = if v == 0 { None } else { Some(v) };
    }
    if let Some(v) = hltb_main_extra {
        game.hltb_main_extra = if v == 0 { None } else { Some(v) };
    }
    if let Some(v) = hltb_completionist {
        game.hltb_completionist = if v == 0 { None } else { Some(v) };
    }
    if let Some(v) = save_paths {
        game.save_paths = v;
    }
    // 启动参数：空串/纯空白视为清空（前端清空输入框走这条），与 description 同款口径。
    // 注意不能用 Option<Option<String>> 那套：serde 无法区分 JSON null 与字段缺失，
    // 前端传 null 只会得到 None（= 不处理），清空就失效了。
    if let Some(v) = launch_args {
        let trimmed = v.trim();
        game.launch_args = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
    }
    // 官方英文名：空串/纯空白 = 清空（前端清空输入框走这条，与 description 同口径）；
    // 非空按 trim 后落库。未提交（None）保持现值。
    if let Some(v) = name_en {
        let trimmed = v.trim();
        game.name_en = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
    }

    db_guard.update_game(&game).map_err(|e| e.to_string())?;
    Ok(game)
}

/// 重命名游戏。
///
/// 改名后要做一次**语言归位**（与手账 `normalize_review_name` 同口径），否则
/// 「中文名」与「英文名」会变成两条互不相干的通道，典型错配：
/// - 中文名改成中文（新名非 ASCII）→ 旧的英文名会跟着新中文名一起展示，语义错乱；
///   此时清空 `name_en`，请用户重新点「刷新信息」拉官方英文名。
/// - 中文名改成纯 ASCII（本身就是英文名）→ 自动补 `name_en`，
///   免得卡片显示中文名、也没法用英文名去 SteamGridDB 检索封面。
///
/// 返回更新后的 `Game`：前端要据此把 `name` 与 `name_en` 一并写回 store，
/// 只同步 `name` 会让卡片立刻显示成中文（2026-10-05 修复）。
#[tauri::command(async)]
pub fn rename_game(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
    new_name: String,
) -> Result<Game, String> {
    rename_game_inner(&db, &game_id, new_name)
}

/// `rename_game` 的函数体（命令层只负责剥 State 壳）。
/// 单独抽出是为了能直接用 `Arc<Mutex<Database>>` 做单元测试——语言归位语义
/// 有 `rename_game_normalizes_name_en` 测试守着。
fn rename_game_inner(
    db: &Arc<Mutex<Database>>,
    game_id: &str,
    new_name: String,
) -> Result<Game, String> {
    if new_name.trim().is_empty() {
        return Err("游戏名称不能为空".to_string());
    }
    let db = lock_or_recover(db);
    let mut game = db.get_game_by_id(game_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "游戏不存在".to_string())?;
    let trimmed = new_name.trim().to_string();
    game.name = trimmed.clone();
    // 语言归位：纯 ASCII 名自动补英文名（已有非空英文名则不动，用户手填优先）；
    // 中文名则清掉不再匹配的英文名（纯 ASCII ≠ 中文名，留着必然错配）。
    if trimmed.is_ascii() {
        if game.name_en.as_deref().is_none_or(|v| v.trim().is_empty()) {
            tracing::info!("名称为纯英文，同步填入英文名: '{}'", trimmed);
            game.name_en = Some(trimmed);
        }
    } else {
        game.name_en = None;
    }
    game.updated_at = Some(chrono::Utc::now().to_rfc3339());
    db.update_game(&game).map_err(|e| e.to_string())?;
    Ok(game)
}

/// 更新游戏可执行文件路径（同时刷新 exe_name、install_path、exe_version）
#[tauri::command(async)]
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
#[tauri::command(async)]
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
/// 导出存档备份的总字节上限（防御性闸门）。
///
/// 正常存档目录不可能接近这个量级；一旦触到，基本意味着 `save_paths` 被误填成了
/// 整个磁盘或用户目录。没有这道闸门时，一次手滑就能对着几 TB 的数据打 ZIP。
const SAVE_BACKUP_MAX_TOTAL_BYTES: u64 = 200 * 1024 * 1024 * 1024; // 200 GB

/// 导出存档备份的条目数上限
const SAVE_BACKUP_MAX_ENTRIES: u64 = 2_000_000;

/// 导出过程中的累计状态（跨递归层级共享）
struct ZipExportState {
    total_bytes: u64,
    entries: u64,
}

/// 判断路径本身是否 reparse point（junction / 符号链接 / 挂载点）。
///
/// 必须用 `symlink_metadata`：`metadata` 会**跟随**链接，永远看不到链接本体。
/// 导出时拒不跟随是必须的 —— 存档目录里一个指向别处的 junction，会把那整块盘卷进备份。
fn is_reparse_point(path: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        std::fs::symlink_metadata(path)
            .map(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        std::fs::symlink_metadata(path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    }
}

/// 原子写文件：先写**同目录**的唯一临时文件，落盘后再 rename 覆盖目标。
///
/// 直接 `std::fs::write(目标)` 在中途失败（磁盘满、U 盘拔出、进程被杀）时，会留下
/// **半成品覆盖掉原本完好的旧备份** —— 而导出恰恰是"给自己留后路"的操作，后路本身
/// 不该这么脆。改成先写临时文件后，失败只影响临时文件，原文件原封不动。
///
/// 临时文件必须与目标同目录：rename 跨卷会失败。
fn write_file_atomically(target: &str, contents: &[u8]) -> Result<(), String> {
    use std::io::Write as _;

    let target_path = std::path::Path::new(target);
    let file_name = target_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| format!("无效的导出路径: {}", target))?;
    let temp_path = target_path.with_file_name(format!(
        ".{}.{}.tmp",
        file_name,
        uuid::Uuid::new_v4().simple()
    ));

    let written = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temp_path)?;
        file.write_all(contents)?;
        file.flush()?;
        // sync_all 让内容真正落盘后再替换：否则断电时可能出现"rename 已生效、
        // 内容还在页缓存"的半空文件。
        file.sync_all()?;
        Ok(())
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&temp_path);
        return Err(format!("写入临时文件失败: {}", e));
    }

    // std::fs::rename 在 Windows 上是替换语义（MoveFileEx + MOVEFILE_REPLACE_EXISTING）
    if let Err(e) = std::fs::rename(&temp_path, target_path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(format!("替换目标文件失败: {}", e));
    }
    Ok(())
}

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
///
/// **锁内只取快照（2026-10-05）**：序列化全库 + 两次原子写盘（`sync_all` 真刷盘）
/// 都不该在全局库锁内做。用户把备份存到网盘 / 移动硬盘 / 慢 U 盘时单次可达数秒，
/// 这数秒里所有其它命令（含每秒的后台时长落库）全部排队；命令本身亦已移出主线程。
#[tauri::command(async)]
pub fn export_game_data(
    db: State<'_, Arc<Mutex<Database>>>,
    file_path: String,
) -> Result<serde_json::Value, String> {
    // 阶段 1：锁内只取快照（游戏列表 + 设置），随即释放
    let (games, settings) = {
        let db_guard = lock_or_recover(&db);
        let games = db_guard.get_games(&GameFilter::default()).map_err(|e| e.to_string())?;
        let settings = Settings::load_from_db(&db_guard).map_err(|e| e.to_string())?;
        (games, settings)
    };
    // 阶段 2：无锁状态下序列化与落盘

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

    // 序列化并**原子**写入主备份（先写同目录临时文件再替换，中途失败不破坏已有备份）
    let json = serde_json::to_string_pretty(&export_data)
        .map_err(|e| format!("序列化失败: {}", e))?;
    write_file_atomically(&file_path, json.as_bytes())
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
    write_file_atomically(&keys_path, keys_json.as_bytes())
        .map_err(|e| format!("写入密钥文件失败: {}", e))?;

    Ok(serde_json::json!({
        "main_path": file_path,
        "keys_path": keys_path,
    }))
}

/// 导入游戏库数据（从 JSON 备份文件恢复，自动识别同目录的密钥文件）。
///
/// 落库走**单个事务**（2026-09-23 起）。此前是逐条 upsert、再写设置、再写密钥，
/// 每步各自提交：中途失败会留下"只导进去一半"的库，而失败只写进日志，
/// 前端只看到计数偏少，用户无从察觉。
///
/// 现在分两阶段：
///   阶段 1 解析与校验（不碰数据库）—— 格式坏的条目在这里出局，不会污染库；
///   阶段 2 单事务落库 —— 要么全部生效，要么整体回滚，库保持原样。
#[tauri::command(async)]
pub fn import_game_data(
    db: State<'_, Arc<Mutex<Database>>>,
    file_path: String,
) -> Result<serde_json::Value, String> {
    let json_data = std::fs::read_to_string(&file_path)
        .map_err(|e| format!("读取备份文件失败: {}", e))?;

    let import_data: serde_json::Value = serde_json::from_str(&json_data)
        .map_err(|e| format!("JSON 解析失败: {}", e))?;

    // ===== 阶段 1：解析与校验（不碰数据库） =====
    let mut games_to_import: Vec<Game> = Vec::new();
    if let Some(games_array) = import_data["games"].as_array() {
        for game_json in games_array {
            match serde_json::from_value::<Game>(game_json.clone()) {
                Ok(game) => {
                    // 校验 game_id 格式（必须为合法 UUID，防止路径遍历）
                    if uuid::Uuid::parse_str(&game.id).is_err() {
                        tracing::warn!("跳过无效 game_id 的游戏: {}", game.id);
                        continue;
                    }
                    games_to_import.push(game);
                }
                Err(e) => {
                    tracing::warn!("解析游戏数据失败: {}", e);
                }
            }
        }
    }

    let settings_to_import: Option<Settings> = match import_data.get("settings") {
        Some(settings_json) => match serde_json::from_value::<Settings>(settings_json.clone()) {
            Ok(settings) => Some(settings),
            Err(e) => {
                tracing::warn!("解析设置数据失败: {}", e);
                None
            }
        },
        None => None,
    };

    // 密钥文件（敏感信息单独存放，若存在则一并恢复）：先读好，稍后连主数据入同一事务
    let keys_path = derive_keys_path(&file_path);
    let keys_json: Option<serde_json::Value> = std::fs::read_to_string(&keys_path)
        .ok()
        .and_then(|data| serde_json::from_str::<serde_json::Value>(&data).ok());

    // ===== 阶段 2：单个事务落库（全成功才生效） =====
    //
    // 锁说明：本命令与改动前一样**全程持有 DB 锁**。单事务把 N 次独立提交压成一次，
    // 持锁时间反而更短 —— 不会给截图关键路径里那次"主键查询(Steam 退让)"添堵。
    // 请勿改成"分批提交 + 中间放锁"，那才会引入新的卡顿面。
    let db_guard = lock_or_recover(&db);
    let (imported_games, settings_restored, keys_restored) = db_guard
        .with_transaction(|| {
            let mut imported = 0u32;
            for game in &games_to_import {
                // 封面策略：upsert_game 的 ON CONFLICT 对 cover 字段使用
                // COALESCE(excluded, games) —— 导入时若本地已有该游戏，保留现有封面；
                // 其他机器导出的封面路径不会生效（文件不存在时前端自动显示占位，
                // 可由"刷新封面"重新获取）。
                db_guard.upsert_game(game)?;
                imported += 1;
            }

            let mut settings_ok = false;
            if let Some(settings) = &settings_to_import {
                settings.save_to_db(&db_guard)?;
                settings_ok = true;
            }

            // 只恢复非空值，避免空值覆盖现有密钥
            let mut keys_ok = 0u32;
            for field in ["steamgriddb_api_key", "llm_api_key"] {
                if let Some(key) = keys_json.as_ref().and_then(|k| k[field].as_str()) {
                    if !key.is_empty() {
                        db_guard.set_setting(field, key)?;
                        keys_ok += 1;
                    }
                }
            }

            Ok((imported, settings_ok, keys_ok))
        })
        .map_err(|e| format!("导入失败，已整体回滚（数据库保持原样）：{}", e))?;

    Ok(serde_json::json!({
        "imported_games": imported_games,
        "settings_restored": settings_restored,
        "keys_restored": keys_restored,
    }))
}

/// 获取所有游戏类型（去重列表）
#[tauri::command(async)]
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
        while let Some(parent) = current.parent() {
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
#[tauri::command(async)]
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
///
/// **两段式 + `(async)`（2026-10-03 修）**：原实现全程持有 DB 互斥锁去做文件 I/O
/// （每个游戏多条 `exists()`，`save_path_effectively_exists` 还会逐级向上探测）——
/// 只要某条路径落在慢介质/离线位置，全应用所有命令都会被这把锁连带堵住。
/// 现改为「锁内只取快照 → 锁外做 I/O」，并把命令移出主线程。
#[tauri::command(async)]
pub fn check_save_paths(
    db: State<'_, Arc<Mutex<Database>>>,
) -> Result<std::collections::HashMap<String, bool>, String> {
    use crate::models::GameFilter;
    use crate::utils::path::expand_env_vars;

    // 阶段 1：锁内只取 (id, save_paths) 快照，随即释放
    let snapshot: Vec<(String, Vec<String>)> = {
        let db_guard = lock_or_recover(&db);
        db_guard
            .get_games(&GameFilter::default())
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|g| (g.id, g.save_paths))
            .collect()
    };

    // 阶段 2：无锁状态下做文件 I/O
    let mut result = std::collections::HashMap::new();
    for (id, save_paths) in snapshot {
        let exists = if save_paths.is_empty() {
            false
        } else {
            save_paths.iter().any(|p| {
                let expanded = expand_env_vars(p);
                save_path_effectively_exists(&expanded)
            })
        };
        result.insert(id, exists);
    }

    Ok(result)
}

/// 检查单个游戏的存档路径是否存在（编辑存档路径后局部刷新用，避免全量检查）
///
/// 与 `check_save_paths` 同构（2026-10-05 补齐）：**锁内只取 `save_paths` 快照 → 锁外做
/// 文件探测 → 命令本身移出主线程**。原实现全程持锁做 `exists()`（`save_path_effectively_exists`
/// 还会逐级向上探测），只要某条路径落在慢介质/离线位置，全应用所有命令都会被这把锁连带堵住。
#[tauri::command(async)]
pub fn check_save_paths_for_game(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: String,
) -> Result<bool, String> {
    use crate::utils::path::expand_env_vars;

    // 阶段 1：锁内只取快照（路径列表），随即释放
    let save_paths: Vec<String> = {
        let db_guard = lock_or_recover(&db);
        let game = db_guard.get_game_by_id(&game_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "游戏不存在".to_string())?;
        game.save_paths
    };

    if save_paths.is_empty() {
        return Ok(false);
    }

    // 阶段 2：无锁状态下做文件 I/O
    let exists = save_paths.iter().any(|p| {
        let expanded = expand_env_vars(p);
        save_path_effectively_exists(&expanded)
    });
    Ok(exists)
}

/// 将目录或文件添加到 ZIP 归档中。
///
/// 两道防护（2026-09-23 加）：
/// - **不跟随 reparse point**（junction / 符号链接）。`Path::is_dir()` 会跟随链接，
///   存档目录里一个指向别的盘的 junction 会把那块盘整个卷进备份；链接成环时递归还会
///   一直走下去。这里改成"跳过并回报一条错误"，不整体失败。
/// - **累计体积与条目数上限**：见 `SAVE_BACKUP_MAX_TOTAL_BYTES`。
fn add_path_to_zip(
    zip: &mut zip::ZipWriter<std::io::BufWriter<std::fs::File>>,
    base_path: &std::path::Path,
    current_path: &std::path::Path,
    zip_prefix: &str,
    state: &mut ZipExportState,
) -> Result<(), String> {
    use zip::write::FileOptions;

    // 入口先拦：无论它链接到目录还是文件，一律不跟随
    if is_reparse_point(current_path) {
        return Err(format!(
            "已跳过链接（junction/symlink），避免把链接目标整块卷入备份: {}",
            current_path.display()
        ));
    }

    if current_path.is_file() {
        let size = std::fs::metadata(current_path).map(|m| m.len()).unwrap_or(0);
        if state.total_bytes.saturating_add(size) > SAVE_BACKUP_MAX_TOTAL_BYTES {
            return Err(format!(
                "备份总大小将超过上限 {} GB，已中止该路径（请检查存档路径是否误填成了整个磁盘）",
                SAVE_BACKUP_MAX_TOTAL_BYTES / 1024 / 1024 / 1024
            ));
        }
        state.entries += 1;
        if state.entries > SAVE_BACKUP_MAX_ENTRIES {
            return Err(format!(
                "备份条目数将超过上限 {}，已中止该路径",
                SAVE_BACKUP_MAX_ENTRIES
            ));
        }
        state.total_bytes += size;

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
            add_path_to_zip(zip, base_path, &entry.path(), zip_prefix, state)?;
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

/// 导出存档备份的同步实现（在阻塞线程池中运行）。
///
/// 先写**同目录临时文件**，全部成功后再原子替换目标 —— 中途失败只影响临时文件，
/// 已有的旧备份原封不动。旧实现直接 `File::create(目标)`，失败会把旧备份覆盖成半成品。
fn export_saves_backup_sync(
    games: &[Game],
    export_path: &str,
) -> Result<serde_json::Value, String> {
    let target = std::path::Path::new(export_path);
    let target_name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| format!("无效的导出路径: {}", export_path))?;
    let temp_path = target.with_file_name(format!(
        ".{}.{}.tmp",
        target_name,
        uuid::Uuid::new_v4().simple()
    ));

    let built = build_save_zip(games, &temp_path);
    let (exported_count, errors) = match built {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::remove_file(&temp_path);
            return Err(e);
        }
    };

    // std::fs::rename 在 Windows 上是替换语义
    if let Err(e) = std::fs::rename(&temp_path, target) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(format!("替换备份文件失败: {}", e));
    }

    Ok(serde_json::json!({
        "exported": exported_count,
        "errors": errors,
    }))
}

/// 把各游戏的存档目录打进 `temp_path` 指向的 ZIP，返回 (成功条数, 错误列表)。
fn build_save_zip(
    games: &[Game],
    temp_path: &std::path::Path,
) -> Result<(u32, Vec<String>), String> {
    let file = std::fs::File::create(temp_path)
        .map_err(|e| format!("创建 ZIP 文件失败: {}", e))?;
    let buf_writer = std::io::BufWriter::new(file);
    let mut zip = zip::ZipWriter::new(buf_writer);

    let mut manifest: Vec<serde_json::Value> = Vec::new();
    let mut exported_count = 0u32;
    let mut errors: Vec<String> = Vec::new();
    // 体积/条目闸门跨所有游戏累计，避免"逐个都不超、加起来爆掉"
    let mut state = ZipExportState {
        total_bytes: 0,
        entries: 0,
    };

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

            match add_path_to_zip(&mut zip, &path, &path, &zip_prefix, &mut state) {
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

    Ok((exported_count, errors))
}

/// 一条落盘计划：ZIP 内条目名 → 最终目标路径。
struct ExtractEntry {
    zip_name: String,
    dest: std::path::PathBuf,
    zip_prefix: String,
    is_dir: bool,
}

/// 完整的恢复落盘计划（预览与实际导入共用）。
struct ExtractPlan {
    entries: Vec<ExtractEntry>,
    /// 去重后的目标根目录（绝对路径）
    target_dirs: Vec<String>,
    errors: Vec<String>,
}

/// 纯逻辑路径规范化：消解 `.` 与 `..`，**不访问文件系统**。
///
/// 用来取代原先的 `canonicalize` 方案做 containment 判定。后者有两个毛病：
/// ① 要求路径真实存在，逼得调用方在"预览"阶段就得把目录建出来（有副作用）；
/// ② 对不存在的嵌套路径只能退化成"检查父目录"，逻辑分叉、难维护。
fn normalize_logically(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

// ==================== 恢复落点冲突裁决（2026-10-06） ====================
//
// 背景：恢复循环是「逐条目：留档 .bak → 写入」。若**多条 ZIP 条目映射到同一落点**
// （save_paths 重复填写、或两条不同目标根在磁盘上重叠），旧实现会"谁在后面谁赢"地静默覆盖：
// 第二次留档把第一次的 .bak（即**原始本地存档**）顶掉，原始文件永久丢失，
// 而 restored 计数用的是 prefix 集合，照样报「恢复成功」—— 最危险的一种失败形态。
//
// 现在：预览把冲突摊给用户看，导入按用户裁决**只保留一条**（同一落点只写一次，
// 留档自然也就只做一次）。内容完全相同的重复条目自动去重，不打扰用户。

/// ZIP 档案类型别名（恢复相关函数都吃这个）
type SaveZipArchive = zip::ZipArchive<std::io::BufReader<std::fs::File>>;

/// 冲突条目的一条记录（大小与 CRC32 都直接来自 ZIP 中央目录，读取**无需解压**）
#[derive(serde::Serialize)]
pub struct SaveConflictEntry {
    pub zip_name: String,
    pub size: u64,
    pub crc32: u32,
}

/// 同落点冲突：多条 ZIP 条目映射到同一最终路径，且其中至少两条内容不同
#[derive(serde::Serialize)]
pub struct SaveIntraConflict {
    /// 冲突的落点（规范化 + 小写后的绝对路径，匹配 Windows 大小写不敏感语义）
    pub dest: String,
    /// 参与冲突的 ZIP 条目（按 ZIP 内顺序）
    pub entries: Vec<SaveConflictEntry>,
}

/// 落点归并键：纯逻辑规范化 + 小写（Windows 下 `C:\Saves\x.dat` 与 `c:\saves\X.DAT` 是同一文件）
fn dest_group_key(dest: &std::path::Path) -> String {
    normalize_logically(dest).to_string_lossy().to_lowercase()
}

/// 按落点把（非目录）条目分组，值为 `plan.entries` 里的下标
fn group_entries_by_dest(
    entries: &[ExtractEntry],
) -> std::collections::BTreeMap<String, Vec<usize>> {
    let mut groups: std::collections::BTreeMap<String, Vec<usize>> = std::collections::BTreeMap::new();
    for (i, e) in entries.iter().enumerate() {
        if e.is_dir {
            continue;
        }
        groups.entry(dest_group_key(&e.dest)).or_default().push(i);
    }
    groups
}

/// 读取一组条目的大小与 CRC32
fn read_entry_metas(
    archive: &mut SaveZipArchive,
    plan: &ExtractPlan,
    idxs: &[usize],
) -> Result<Vec<SaveConflictEntry>, String> {
    let mut metas = Vec::with_capacity(idxs.len());
    for &i in idxs {
        let e = &plan.entries[i];
        let f = archive
            .by_name(&e.zip_name)
            .map_err(|err| format!("读取 ZIP 条目失败 {}: {}", e.zip_name, err))?;
        metas.push(SaveConflictEntry {
            zip_name: e.zip_name.clone(),
            size: f.size(),
            crc32: f.crc32(),
        });
    }
    Ok(metas)
}

/// 一组条目是否内容全同（大小 + CRC32 双判据，zip 去重的标准做法；CRC 相同视为同一份）
fn all_same_content(metas: &[SaveConflictEntry]) -> bool {
    metas
        .iter()
        .all(|m| m.size == metas[0].size && m.crc32 == metas[0].crc32)
}

/// 找出**内容真冲突**的落点（同落点的多条条目里存在内容不同的组合）
fn detect_intra_conflicts(
    archive: &mut SaveZipArchive,
    plan: &ExtractPlan,
) -> Result<Vec<SaveIntraConflict>, String> {
    let mut out = Vec::new();
    for (dest, idxs) in group_entries_by_dest(&plan.entries) {
        if idxs.len() < 2 {
            continue;
        }
        let metas = read_entry_metas(archive, plan, &idxs)?;
        if !all_same_content(&metas) {
            out.push(SaveIntraConflict { dest, entries: metas });
        }
    }
    Ok(out)
}

/// 按裁决策略算出实际要写出的条目下标（同落点只保留一条，顺序还原为计划顺序）。
///
/// 策略取值：`"first"` 保留靠前一条 / `"last"` 保留靠后一条（等价旧行为）/
/// 其余（含 `"abort"` 与未知值）→ 拒绝恢复并回报冲突清单。
/// **内容相同的重复条目在任何策略下都自动去重**，不会因此中止。
fn select_entries(
    archive: &mut SaveZipArchive,
    plan: &ExtractPlan,
    policy: &str,
) -> Result<Vec<usize>, String> {
    // 目录条目照旧全部保留：它们只做 create_dir_all，重复执行无副作用
    let mut keep: Vec<usize> = plan
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.is_dir)
        .map(|(i, _)| i)
        .collect();
    let mut conflicts: Vec<SaveIntraConflict> = Vec::new();

    for (dest, idxs) in group_entries_by_dest(&plan.entries) {
        if idxs.len() == 1 {
            keep.push(idxs[0]);
            continue;
        }
        let metas = read_entry_metas(archive, plan, &idxs)?;
        if all_same_content(&metas) {
            keep.push(idxs[0]);
            continue;
        }
        match policy {
            "first" => keep.push(idxs[0]),
            "last" => keep.push(idxs[idxs.len() - 1]),
            _ => conflicts.push(SaveIntraConflict { dest, entries: metas }),
        }
    }

    if !conflicts.is_empty() {
        let brief: Vec<String> = conflicts.iter().take(5).map(|c| c.dest.clone()).collect();
        return Err(format!(
            "备份内有 {} 处「多个文件写到同一位置且内容不同」，已中止恢复（请在预览中裁决后重试）: {}{}",
            conflicts.len(),
            brief.join("；"),
            if conflicts.len() > brief.len() { " …" } else { "" }
        ));
    }

    keep.sort_unstable();
    Ok(keep)
}

/// 解析 ZIP 备份的 manifest，产出完整落盘计划。
///
/// **纯解析：不建目录、不写文件**，因此可安全用于"恢复前预览"。
///
/// 铁律：预览与导入必须共用这一份解析。一旦分裂成两份，"预览说要写 A、实际写进 B"
/// 这类错位几乎必然冒出来，且极难排查。
fn build_extract_plan(
    archive: &mut zip::ZipArchive<std::io::BufReader<std::fs::File>>,
) -> Result<ExtractPlan, String> {
    // 读取 manifest.json
    let manifest: Vec<serde_json::Value> = {
        let mut manifest_file = archive
            .by_name("manifest.json")
            .map_err(|_| "ZIP 文件中缺少 manifest.json".to_string())?;
        let mut content = String::new();
        std::io::Read::read_to_string(&mut manifest_file, &mut content)
            .map_err(|e| format!("读取 manifest.json 失败: {}", e))?;
        serde_json::from_str(&content).map_err(|e| format!("解析 manifest.json 失败: {}", e))?
    };

    // 预处理：一次性收集所有 ZIP 文件名，避免对每个 manifest 条目重复遍历 ZIP 目录
    let all_zip_names: Vec<String> = archive.file_names().map(|s| s.to_string()).collect();

    let mut errors: Vec<String> = Vec::new();
    let mut entries: Vec<ExtractEntry> = Vec::new();
    let mut target_dirs: Vec<String> = Vec::new();

    for entry in &manifest {
        let original_path = entry["original_path"].as_str().unwrap_or("");
        let zip_prefix = entry["zip_prefix"].as_str().unwrap_or("");

        if original_path.is_empty() || zip_prefix.is_empty() {
            continue;
        }

        let expanded = utils::path::expand_env_vars(original_path);
        let target_path = std::path::PathBuf::from(&expanded);

        // 目标根必须是绝对路径：相对路径会随进程工作目录漂移，containment 判定就没有意义了
        if !target_path.is_absolute() {
            errors.push(format!("跳过非绝对路径的存档目标: {}", expanded));
            continue;
        }
        let normalized_target = normalize_logically(&target_path);
        let target_display = target_path.display().to_string();
        if !target_dirs.contains(&target_display) {
            target_dirs.push(target_display);
        }

        let prefix_with_slash = format!("{}/", zip_prefix);
        let file_names: Vec<&str> = all_zip_names
            .iter()
            .filter(|name| name.as_str() == zip_prefix || name.starts_with(&prefix_with_slash))
            .map(|s| s.as_str())
            .collect();

        for file_name in file_names {
            let relative = file_name.strip_prefix(&prefix_with_slash).unwrap_or(file_name);
            let relative_path = std::path::Path::new(relative);

            // 安全检查：ZIP 条目名不得含 `..`
            if relative_path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                tracing::warn!("跳过包含路径遍历的 ZIP 条目: {}", file_name);
                continue;
            }
            // 安全检查：ZIP 条目名不得是绝对路径
            if relative_path.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::RootDir | std::path::Component::Prefix(_)
                )
            }) {
                tracing::warn!("跳过包含绝对路径的 ZIP 条目: {}", file_name);
                continue;
            }

            let dest = if relative.is_empty() {
                target_path.clone()
            } else {
                target_path.join(relative)
            };

            // 最终 containment：落点规范化后必须仍在目标根之内（纯逻辑，无副作用）
            if !normalize_logically(&dest).starts_with(&normalized_target) {
                tracing::warn!("跳过目标路径超出预期目录的 ZIP 条目: {}", file_name);
                continue;
            }

            entries.push(ExtractEntry {
                // ZIP 惯例：目录条目的名字以 '/' 结尾
                is_dir: file_name.ends_with('/'),
                zip_name: file_name.to_string(),
                dest,
                zip_prefix: zip_prefix.to_string(),
            });
        }
    }

    Ok(ExtractPlan {
        entries,
        target_dirs,
        errors,
    })
}

/// 覆盖前把已存在的目标文件挪成 `.bak` 留档，返回留下的备份路径（原本不存在则为 None）。
///
/// 用 rename 而非 copy：同目录移动是瞬时的、不额外占空间，也不会在大存档上卡住。
/// 目的只有一个 —— 恢复错了备份时，原来那份还躺在旁边。
fn backup_existing_file(dest: &std::path::Path) -> Result<Option<std::path::PathBuf>, String> {
    if !dest.is_file() {
        return Ok(None);
    }
    let Some(file_name) = dest.file_name() else {
        return Ok(None);
    };
    let backup = dest.with_file_name(format!("{}.bak", file_name.to_string_lossy()));
    // Windows 上 rename 是替换语义：重复恢复时旧 .bak 会被新版覆盖
    std::fs::rename(dest, &backup)
        .map_err(|e| format!("备份原文件失败 {}: {}", dest.display(), e))?;
    Ok(Some(backup))
}

/// 存档恢复预览：把"即将写入哪里、会新建哪些目录、会覆盖哪些文件"先摊给用户看。
#[derive(serde::Serialize)]
pub struct SavesRestorePreview {
    /// 去重后的目标根目录（最终落点）
    pub target_dirs: Vec<String>,
    /// 还不存在、恢复时会被新建的目标根目录
    pub dirs_to_create: Vec<String>,
    /// ZIP 内将被写出的文件总数（不含目录条目）
    pub total_files: u64,
    /// 会被覆盖的文件总数
    pub overwrite_count: u64,
    /// 将被覆盖的文件清单（最多列前 50 条，避免返回体过大）
    pub overwrite_files: Vec<String>,
    /// **ZIP 内部冲突**：多条条目映射到同一落点且内容不同。非空时前端必须让用户
    /// 裁决保留哪一份，否则恢复会静默互相覆盖（并可能顶掉原始文件的 .bak 留档）。
    pub intra_conflicts: Vec<SaveIntraConflict>,
    /// 解析过程中的问题（非致命）
    pub warnings: Vec<String>,
}

/// 预览存档备份的恢复落点（**不写入任何东西**）。
///
/// 前端在真正执行恢复前先调它：
/// - `overwrite_count > 0` → 弹确认框，让用户看清"这次恢复会覆盖掉哪些现有存档"；
/// - `intra_conflicts` 非空 → **必须**弹出选择窗口，让用户裁决每个落点保留备份内的哪一份
///   （同落点被静默互相覆盖是真实丢数据路径，见 `select_entries` 的说明）。
#[tauri::command]
pub async fn preview_saves_backup(zip_path: String) -> Result<SavesRestorePreview, String> {
    tauri::async_runtime::spawn_blocking(move || preview_saves_backup_sync(&zip_path))
        .await
        .map_err(|e| format!("预览任务执行失败: {}", e))?
}

fn preview_saves_backup_sync(zip_path: &str) -> Result<SavesRestorePreview, String> {
    /// 清单最多列这么多条，避免超大备份把返回体撑爆
    const MAX_LISTED: usize = 50;

    let file = std::fs::File::open(zip_path).map_err(|e| format!("打开 ZIP 文件失败: {}", e))?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| format!("读取 ZIP 文件失败: {}", e))?;

    let plan = build_extract_plan(&mut archive)?;
    let intra_conflicts = detect_intra_conflicts(&mut archive, &plan)?;

    let dirs_to_create: Vec<String> = plan
        .target_dirs
        .iter()
        .filter(|d| !std::path::Path::new(d).is_dir())
        .cloned()
        .collect();

    let total_files = plan.entries.iter().filter(|e| !e.is_dir).count() as u64;

    let mut overwrite_files: Vec<String> = Vec::new();
    let mut overwrite_count = 0u64;
    for entry in plan.entries.iter().filter(|e| !e.is_dir) {
        if entry.dest.is_file() {
            overwrite_count += 1;
            if overwrite_files.len() < MAX_LISTED {
                overwrite_files.push(entry.dest.display().to_string());
            }
        }
    }

    Ok(SavesRestorePreview {
        target_dirs: plan.target_dirs,
        dirs_to_create,
        total_files,
        overwrite_count,
        overwrite_files,
        intra_conflicts,
        warnings: plan.errors,
    })
}

/// 从 ZIP 备份文件导入存档（不需要数据库锁，仅做文件 I/O）。
///
/// `conflict_policy`：ZIP 内多条条目映射到同一落点且内容不同时的裁决策略，
/// 由前端在「备份内存在冲突条目」弹窗里选定：
/// - `"first"` 保留靠前的一条；`"last"` 保留靠后的一条（等价旧行为）；
/// - 缺省 / `"abort"` / 未知取值 → **拒绝恢复**（宁可停下让用户裁决，也不做静默覆盖）。
#[tauri::command]
pub async fn import_saves_backup(
    zip_path: String,
    conflict_policy: Option<String>,
) -> Result<serde_json::Value, String> {
    let policy = conflict_policy.unwrap_or_else(|| "abort".to_string());
    // 解压同样是文件密集 I/O，放阻塞线程池执行
    tauri::async_runtime::spawn_blocking(move || import_saves_backup_sync(&zip_path, &policy))
        .await
        .map_err(|e| format!("导入任务执行失败: {}", e))?
}

/// 导入存档备份的同步实现（在阻塞线程池中运行）。
///
/// 与预览共用 `build_extract_plan`（**同一份路径解析**），所以"预览看到的落点"
/// 与"实际写入的位置"必然一致。
///
/// 覆盖行为（2026-09-23 改）：目标文件已存在时，先把原文件改成 `.bak` 留档再写新的；
/// 新文件写入失败则把 `.bak` 挪回原位。旧实现是 `File::create` 直接截断覆盖 ——
/// 选错备份就是当场丢掉当前存档，一点退路都没有。
///
/// **同落点去重（2026-10-06）**：先按裁决只保留每个落点的一条条目。这同时修掉一个
/// 真实的丢数据路径 —— 同一落点被写两次时，第二次留档会把第一次的 `.bak`
/// （即原始本地存档）顶掉，原始文件永久消失，而计数照样报成功。
fn import_saves_backup_sync(
    zip_path: &str,
    conflict_policy: &str,
) -> Result<serde_json::Value, String> {
    let file = std::fs::File::open(zip_path)
        .map_err(|e| format!("打开 ZIP 文件失败: {}", e))?;
    let buf_reader = std::io::BufReader::new(file);
    let mut archive = zip::ZipArchive::new(buf_reader)
        .map_err(|e| format!("读取 ZIP 文件失败: {}", e))?;

    let plan = build_extract_plan(&mut archive)?;
    // 先裁决再动手：拒绝恢复的路径必须在**任何文件被改动之前**返回
    let keep = select_entries(&mut archive, &plan, conflict_policy)?;
    let mut errors = plan.errors.clone();

    // 建出目标根目录（换机 / 存档丢失场景下这些目录本来并不存在）。
    // 注意 save_path 也可能指向**单个文件**：那种情况 create_dir_all 必然失败，跳过即可
    // （文件级写入会各自兜住父目录）。
    for dir in &plan.target_dirs {
        let path = std::path::Path::new(dir);
        if path.is_file() {
            continue;
        }
        if let Err(e) = std::fs::create_dir_all(path) {
            errors.push(format!("创建存档目标目录失败 {}: {}", dir, e));
        }
    }

    // 成功恢复的存档路径（zip_prefix）集合，用于精确计数而非按 manifest 条目数虚报
    let mut restored_prefixes: std::collections::HashSet<String> = std::collections::HashSet::new();

    // 只写裁决后保留的条目（`keep` 已是计划顺序，同落点唯一）
    for i in keep {
        let entry = &plan.entries[i];
        let mut zip_file = match archive.by_name(&entry.zip_name) {
            Ok(f) => f,
            Err(e) => {
                errors.push(format!("读取 ZIP 条目失败 {}: {}", entry.zip_name, e));
                continue;
            }
        };

        if entry.is_dir {
            if let Err(e) = std::fs::create_dir_all(&entry.dest) {
                errors.push(format!("创建目录失败: {}", e));
            }
            continue;
        }

        if let Some(parent) = entry.dest.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                errors.push(format!("创建目录失败: {}", e));
                continue;
            }
        }

        // 覆盖前留档
        let backup = match backup_existing_file(&entry.dest) {
            Ok(b) => b,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };

        let copy_result = {
            let mut dest_file = match std::fs::File::create(&entry.dest) {
                Ok(f) => f,
                Err(e) => {
                    errors.push(format!("创建文件失败 {}: {}", entry.dest.display(), e));
                    // 建不出来就把留档挪回去
                    if let Some(b) = &backup {
                        let _ = std::fs::rename(b, &entry.dest);
                    }
                    continue;
                }
            };
            let copied = std::io::copy(&mut zip_file, &mut dest_file);
            // 句柄必须先释放：Windows 上文件被打开时不允许 rename
            drop(dest_file);
            copied
        };
        drop(zip_file);

        match copy_result {
            Ok(_) => {
                restored_prefixes.insert(entry.zip_prefix.clone());
            }
            Err(e) => {
                errors.push(format!("写入文件失败 {}: {}", entry.dest.display(), e));
                // 写失败就把留档挪回原位，别让用户两头空
                if let Some(b) = &backup {
                    let _ = std::fs::remove_file(&entry.dest);
                    if let Err(re) = std::fs::rename(b, &entry.dest) {
                        errors.push(format!("回滚留档失败 {}: {}", b.display(), re));
                    }
                }
            }
        }
    }

    for dir in &plan.target_dirs {
        tracing::info!("已恢复存档到: {}", dir);
    }

    Ok(serde_json::json!({
        "restored": restored_prefixes.len() as u32,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Arc<Mutex<Database>> {
        Arc::new(Mutex::new(
            Database::new(std::path::Path::new(":memory:")).expect("内存库初始化失败"),
        ))
    }

    fn seeded_game() -> crate::models::Game {
        let mut g = crate::models::Game::new("Test Game".to_string());
        g.description = Some("一段描述".to_string());
        g.developer = Some("DevCo".to_string());
        g.publisher = Some("PubCo".to_string());
        g.release_date = Some("2020-01-01".to_string());
        g.genres = vec!["动作".to_string()];
        g.hltb_main_story = Some(120);
        g.hltb_main_extra = Some(60);
        g.hltb_completionist = Some(300);
        g.save_paths = vec!["C:/Saves/TestGame".to_string()];
        g.launch_args = Some("-windowed".to_string());
        g
    }

    /// 改名必须做语言归位（2026-10-05 修复）：
    /// 纯 ASCII 新名自动补 name_en；改成中文名则清掉形同虚设的旧英文名。
    /// 这是「中文名/英文名两条通道互不相干」那个 bug 的回归防线。
    #[test]
    fn rename_game_normalizes_name_en() {
        let db = test_db();
        let id = {
            let mut g = crate::models::Game::new("空洞骑士".to_string());
            g.name_en = Some("Hollow Knight".to_string());
            let id = g.id.clone();
            db.lock().unwrap().upsert_game(&g).unwrap();
            id
        };

        // 改成中文名 → 旧英文名与中文名不匹配，必须清空（避免错配展示）
        let renamed = rename_game_inner(&db, &id, "空洞骑士：丝之歌".to_string())
            .expect("改名不应失败");
        assert_eq!(renamed.name, "空洞骑士：丝之歌");
        assert_eq!(renamed.name_en, None, "改成中文名必须清空 name_en（防错配）");

        // 改成纯 ASCII 名 → 自动补 name_en（卡片/封面检索都靠它）
        let renamed = rename_game_inner(&db, &id, "Hollow Knight: Silksong".to_string())
            .expect("改名不应失败");
        assert_eq!(renamed.name, "Hollow Knight: Silksong");
        assert_eq!(
            renamed.name_en.as_deref(),
            Some("Hollow Knight: Silksong"),
            "纯 ASCII 名必须自动补 name_en"
        );

        // 已有非空英文名时，改中文名清空；改回英文名时不得覆盖用户手工填的英文名
        db.lock().unwrap().upsert_game(&{
            let mut g = renamed.clone();
            g.name_en = Some("Official EN Name".to_string());
            g
        }).unwrap();
        let kept = rename_game_inner(&db, &id, "Another ASCII Name".to_string())
            .expect("改名不应失败");
        assert_eq!(
            kept.name_en.as_deref(),
            Some("Official EN Name"),
            "已有英文名不得被 ASCII 新名覆盖（用户手填优先）"
        );
    }

    /// 清空哨兵必须真正落成 None/空——这是 0.8.3「设值修了、清空仍静默失效」
    /// 那个 bug（R1，见 2026-09-22 审查报告）的回归防线。
    /// 哨兵语义：文本=空串 / genres=空数组 / hltb=0 / save_paths=空数组 / launch_args=空白。
    #[test]
    fn game_meta_clear_and_sentinel_semantics() {
        let db = test_db();
        let game = seeded_game();
        let id = game.id.clone();
        db.lock().unwrap().upsert_game(&game).unwrap();

        let updated = apply_game_meta_update(
            &db, &id,
            Some(String::new()),          // description 清空
            Some("  ".to_string()),        // developer 纯空白（文本字段不 trim，等价清空？——注意：文本哨兵是"空串"，这里故意传两个空格验证与 launch_args 的差异口径）
            Some(String::new()),          // publisher 清空
            Some(String::new()),          // release_date 清空
            Some(vec![]),                 // genres 清空
            Some(0),                       // hltb_main_story 清空
            Some(0),                       // hltb_main_extra 清空
            Some(0),                       // hltb_completionist 清空
            Some(vec![]),                 // save_paths 清空
            Some("   ".to_string()),       // launch_args 纯空白 → 清空
            Some(String::new()),            // name_en 空串 → 清空
        )
        .expect("更新不应失败");

        assert_eq!(updated.description, None, "空串必须清空 description");
        // developer 的哨兵是严格空串（与后端既有 is_empty 判断一致）；纯空白不会被当清空——
        // 这个口径与 launch_args（trim 后判空）不同，属既有行为，测试里锁死以免无意漂移。
        assert_eq!(updated.developer, Some("  ".to_string()), "文本字段不清 trim（哨兵是空串，非空白）");
        assert_eq!(updated.publisher, None, "空串必须清空 publisher");
        assert_eq!(updated.release_date, None, "空串必须清空 release_date");
        assert!(updated.genres.is_empty(), "空数组必须清空 genres");
        assert_eq!(updated.hltb_main_story, None, "0 必须清空 hltb_main_story");
        assert_eq!(updated.hltb_main_extra, None, "0 必须清空 hltb_main_extra");
        assert_eq!(updated.hltb_completionist, None, "0 必须清空 hltb_completionist");
        assert!(updated.save_paths.is_empty(), "空数组必须清空 save_paths");
        assert_eq!(updated.launch_args, None, "纯空白必须清空 launch_args（trim 口径）");
        assert_eq!(updated.name_en, None, "空串必须清空 name_en");
    }

    /// 设值与"未提交"（None = 字段缺失或 JSON null）保持现值
    #[test]
    fn game_meta_set_and_untouched_semantics() {
        let db = test_db();
        let game = seeded_game();
        let id = game.id.clone();
        db.lock().unwrap().upsert_game(&game).unwrap();

        let updated = apply_game_meta_update(
            &db, &id,
            Some("新描述".to_string()),   // 设值
            None,                          // 未提交 → 保持
            None,                          // 未提交（等价 JSON null 被框架折叠后的形态）
            Some("2024-06-01".to_string()),
            Some(vec![
                "Adventure".to_string(), // 英文别名 → 中文规范名
                "Action".to_string(),    // 同上
                "动作".to_string(),       // 与上一项归一化后重复 → 应被去掉
            ]),
            Some(90),
            None,
            Some(400),
            Some(vec!["D:/New/Path".to_string()]),
            Some("-fullscreen".to_string()),
            Some("Total War: Three Kingdoms".to_string()), // name_en 设值
        )
        .expect("更新不应失败");

        assert_eq!(updated.description, Some("新描述".to_string()));
        assert_eq!(updated.developer, Some("DevCo".to_string()), "None 必须保持现值");
        assert_eq!(updated.publisher, Some("PubCo".to_string()), "None 必须保持现值");
        assert_eq!(updated.release_date, Some("2024-06-01".to_string()));
        assert_eq!(
            updated.genres,
            vec!["冒险".to_string(), "动作".to_string()],
            "英文别名应归一化为中文规范名，重复项应被去掉"
        );
        assert_eq!(updated.hltb_main_story, Some(90));
        assert_eq!(updated.hltb_main_extra, Some(60), "None 必须保持现值");
        assert_eq!(updated.hltb_completionist, Some(400));
        assert_eq!(updated.save_paths, vec!["D:/New/Path".to_string()]);
        assert_eq!(updated.launch_args, Some("-fullscreen".to_string()));
        assert_eq!(
            updated.name_en.as_deref(),
            Some("Total War: Three Kingdoms"),
            "name_en 设值应落库"
        );
    }

    // ==================== 2026-09-23：存档备份加固的回归防线 ====================

    /// 纯逻辑路径规范化：`.` 丢弃、`..` 消解，且不破坏盘符前缀。
    ///
    /// 这是恢复落点 containment 判定的基石 —— 它错了，`..` 就能把文件写出目标目录。
    #[test]
    fn normalize_logically_handles_dot_and_dotdot() {
        use std::path::Path;
        let cases = [
            (r"C:\a\b", r"C:\a\b"),
            (r"C:\a\.\b", r"C:\a\b"),
            (r"C:\a\..\b", r"C:\b"),
            (r"C:\a\b\..\..\c", r"C:\c"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                normalize_logically(Path::new(input)),
                Path::new(expected),
                "规范化 {input} 应得 {expected}"
            );
        }
    }

    /// 构造一个临时 ZIP（含 manifest.json + 若干条目），返回其路径。
    fn write_test_zip(entries: &[&str], manifest: &serde_json::Value) -> std::path::PathBuf {
        use std::io::Write as _;
        let path = std::env::temp_dir().join(format!(
            "gv-restore-test-{}.zip",
            uuid::Uuid::new_v4().simple()
        ));
        let file = std::fs::File::create(&path).expect("建临时 ZIP 失败");
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::FileOptions::default();
        zip.start_file("manifest.json", opts).unwrap();
        zip.write_all(manifest.to_string().as_bytes()).unwrap();
        for name in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(b"data").unwrap();
        }
        zip.finish().unwrap();
        path
    }

    /// 恢复计划必须剔除 `..` 条目与非绝对路径的目标根，只放行落在目标目录内的条目。
    ///
    /// 预览与实际导入共用 `build_extract_plan`，所以这一条同时守住两侧 ——
    /// 也是"预览说写 A、实际写进 B"这类错位的防线。
    #[test]
    fn extract_plan_filters_traversal_and_relative_targets() {
        let target = std::env::temp_dir()
            .join(format!("gv-target-{}", uuid::Uuid::new_v4().simple()));
        let manifest = serde_json::json!([
            { "game_id": "g1", "original_path": target.display().to_string(), "zip_prefix": "p" },
            { "game_id": "g2", "original_path": "relative\\oops", "zip_prefix": "q" },
        ]);
        let zip_path = write_test_zip(
            &["p/save.dat", "p/../../escape.dat", "q/whatever.dat"],
            &manifest,
        );

        let file = std::fs::File::open(&zip_path).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file)).unwrap();
        let plan = build_extract_plan(&mut archive).unwrap();

        assert_eq!(plan.entries.len(), 1, "只该接受 p/save.dat");
        assert_eq!(plan.entries[0].zip_name, "p/save.dat");
        assert!(!plan.entries[0].is_dir, "文件条目的 is_dir 必须为 false");
        assert!(
            plan.entries[0].dest.starts_with(&target),
            "落点必须落在目标根之下"
        );
        assert_eq!(plan.target_dirs.len(), 1, "相对路径的目标根不该进入计划");
        assert!(
            !plan.errors.is_empty(),
            "被拒绝的目标根应留下可回报说明，不能静默丢弃"
        );

        let _ = std::fs::remove_file(&zip_path);
    }

    /// 原子写文件：目标已存在时必须**整体替换**（而非只覆盖前几字节），且不留临时文件。
    #[test]
    fn atomic_write_replaces_target_and_leaves_no_temp() {
        let dir = std::env::temp_dir()
            .join(format!("gv-atomic-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("backup.json");
        std::fs::write(&target, b"OLD-AND-MUCH-LONGER-CONTENT").unwrap();

        write_file_atomically(&target.display().to_string(), b"NEW").unwrap();

        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"NEW",
            "必须是整体替换，不能留下旧内容的尾巴"
        );
        let leftovers: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "不应残留临时文件: {leftovers:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 游戏侧 `name` 语言闸门（2026-10-09 补）：
    /// LLM 把官方英文名原样回填进 name 时，无权覆盖已有中文名；英文→英文的纠错照常。
    /// 病灶实证：2026-10-06 起 prompt 的 name 被写成「与用户输入一致」，God of War 这类
    /// 英文标题刷新后 name 与 name_en 双双变英文（详情页双栏全英文）。
    #[test]
    fn apply_llm_meta拒收纯英文名覆盖中文名() {
        // ① 现名中文 + LLM 回英文 → 拒收
        let mut g = seeded_game();
        g.name = "战神".to_string();
        g.name_en = Some("God of War".to_string());
        apply_llm_meta(
            &mut g,
            &LlmGameMeta {
                name: Some("God of War".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(g.name, "战神", "英文名不得覆盖中文名");

        // ② 现名英文 + LLM 回英文修正 → 照常采纳（不能被误伤）
        let mut g2 = seeded_game();
        g2.name = "Gears of War Reloaded".to_string();
        apply_llm_meta(
            &mut g2,
            &LlmGameMeta {
                name: Some("Gears of War: Reloaded".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(g2.name, "Gears of War: Reloaded");

        // ③ 现名英文 + LLM 回中文 → 采纳，且中文名不会再被自动填进 name_en
        let mut g3 = seeded_game();
        g3.name = "God of War".to_string();
        apply_llm_meta(
            &mut g3,
            &LlmGameMeta {
                name: Some("战神".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(g3.name, "战神");
        assert_eq!(g3.name_en, None, "中文名的条目不得把 name_en 填成中文");
    }

    /// 游戏侧 `name_en` 语言闸门（2026-10-06 补，此前只有手账侧有）：
    /// LLM 把中文名塞进 name_en 时必须拒收；合法英文名照常写入；空串维持"不动原值"的旧语义。
    /// 与 hand账 reviews.rs::llm返回的中文英文名被拒收 是同一条契约的两侧防线。
    #[test]
    fn apply_llm_meta拒收中文英文名() {
        // ① 已有英文名：中文脏值被拒收，原值安然无恙
        let mut g = seeded_game();
        g.name_en = Some("Hollow Knight".to_string());
        apply_llm_meta(
            &mut g,
            &LlmGameMeta {
                name_en: Some("空洞骑士".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(g.name_en.as_deref(), Some("Hollow Knight"));

        // ② 原本没有英文名：中文脏值不得落库
        let mut g2 = seeded_game();
        g2.name = "空洞骑士".to_string();
        apply_llm_meta(
            &mut g2,
            &LlmGameMeta {
                name_en: Some("空洞骑士".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(g2.name_en, None, "中文 name_en 不得落库");

        // ③ 合法英文名（含标点/冒号）正常写入
        let mut g3 = seeded_game();
        apply_llm_meta(
            &mut g3,
            &LlmGameMeta {
                name_en: Some("Mafia: The Old Country".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(g3.name_en.as_deref(), Some("Mafia: The Old Country"));

        // ④ 空串维持原语义：游戏侧没有手账那种"空串 = 显式清除"，不得清掉用户手填值
        let mut g4 = seeded_game();
        g4.name_en = Some("Keep Me".to_string());
        apply_llm_meta(
            &mut g4,
            &LlmGameMeta {
                name_en: Some("   ".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(g4.name_en.as_deref(), Some("Keep Me"));
    }

    // ==================== 2026-10-06：恢复落点冲突裁决的回归防线 ====================

    /// 构造带**自定义内容**的临时 ZIP（同 `write_test_zip`，但每条内容可不同，
    /// 冲突检测靠内容/CRC 判定，必须能造出"同一落点、两种内容"）。
    fn write_test_zip_with(
        entries: &[(&str, &[u8])],
        manifest: &serde_json::Value,
    ) -> std::path::PathBuf {
        use std::io::Write as _;
        let path = std::env::temp_dir().join(format!(
            "gv-conflict-test-{}.zip",
            uuid::Uuid::new_v4().simple()
        ));
        let file = std::fs::File::create(&path).expect("建临时 ZIP 失败");
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::FileOptions::default();
        zip.start_file("manifest.json", opts).unwrap();
        zip.write_all(manifest.to_string().as_bytes()).unwrap();
        for (name, content) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(content).unwrap();
        }
        zip.finish().unwrap();
        path
    }

    /// 同一落点被两条**内容不同**的 ZIP 条目写入时：
    /// ① 预览必须报出冲突；② abort 策略必须在动手之前返回、且**不改动任何文件**；
    /// ③ last 保留靠后一条，且 `.bak` 里躺的必须是**原始本地存档**（留档只做一次）。
    #[test]
    fn 同落点内容冲突被拦下且不静默覆盖() {
        let dir = std::env::temp_dir().join(format!("gv-cf-{}", uuid::Uuid::new_v4().simple()));
        let target = dir.join("saves");
        std::fs::create_dir_all(&target).unwrap();
        let local = target.join("save.dat");
        std::fs::write(&local, b"ORIGINAL-LOCAL").unwrap();

        let manifest = serde_json::json!([
            { "game_id": "g1", "original_path": target.display().to_string(), "zip_prefix": "a" },
            { "game_id": "g2", "original_path": target.display().to_string(), "zip_prefix": "b" },
        ]);
        // 两个 prefix 都映射到同一个 target/save.dat，内容不同
        let zip_path = write_test_zip_with(
            &[("a/save.dat", b"FROM-A"), ("b/save.dat", b"FROM-B")],
            &manifest,
        );
        let zp = zip_path.to_str().unwrap();

        // ① 预览报冲突
        let preview = preview_saves_backup_sync(zp).unwrap();
        assert_eq!(preview.intra_conflicts.len(), 1, "同落点不同内容必须报冲突");
        assert_eq!(preview.intra_conflicts[0].entries.len(), 2);

        // ② abort：动手之前返回，原始文件原地不动，连 .bak 都不该出现
        let err = import_saves_backup_sync(zp, "abort").unwrap_err();
        assert!(err.contains("已中止恢复"), "错误信息需说清已中止: {err}");
        assert_eq!(std::fs::read(&local).unwrap(), b"ORIGINAL-LOCAL");
        assert!(
            !target.join("save.dat.bak").exists(),
            "被拦下的恢复不得产生任何留档"
        );

        // ③ last：保留靠后一条；.bak 是原始本地存档（只留档一次，没被第二次写入顶掉）
        let r = import_saves_backup_sync(zp, "last").unwrap();
        assert_eq!(r["restored"], 1);
        assert_eq!(std::fs::read(&local).unwrap(), b"FROM-B");
        assert_eq!(
            std::fs::read(target.join("save.dat.bak")).unwrap(),
            b"ORIGINAL-LOCAL",
            "原始本地存档必须完整躺在 .bak 里"
        );

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&zip_path);
    }

    /// first 策略保留靠前一条；内容完全相同的重复条目在任何策略下都自动去重、不算冲突。
    #[test]
    fn 冲突策略first与同内容去重() {
        let dir = std::env::temp_dir().join(format!("gv-cf2-{}", uuid::Uuid::new_v4().simple()));
        let target = dir.join("saves");
        std::fs::create_dir_all(&target).unwrap();
        let local = target.join("save.dat");
        std::fs::write(&local, b"ORIGINAL").unwrap();

        let manifest = serde_json::json!([
            { "game_id": "g1", "original_path": target.display().to_string(), "zip_prefix": "a" },
            { "game_id": "g2", "original_path": target.display().to_string(), "zip_prefix": "b" },
        ]);

        // 不同内容 → first 保留靠前一条
        let zip_diff = write_test_zip_with(
            &[("a/save.dat", b"FROM-A"), ("b/save.dat", b"FROM-B")],
            &manifest,
        );
        import_saves_backup_sync(zip_diff.to_str().unwrap(), "first").unwrap();
        assert_eq!(std::fs::read(&local).unwrap(), b"FROM-A");

        // 相同内容 → 不算冲突，abort 策略也照常恢复（自动去重）
        std::fs::write(&local, b"ORIGINAL").unwrap();
        let zip_same = write_test_zip_with(
            &[("a/save.dat", b"SAME"), ("b/save.dat", b"SAME")],
            &manifest,
        );
        let zp = zip_same.to_str().unwrap();
        assert!(
            preview_saves_backup_sync(zp).unwrap().intra_conflicts.is_empty(),
            "内容相同的重复条目不该被报成冲突"
        );
        import_saves_backup_sync(zp, "abort").expect("内容相同应自动去重、不该被 abort 拦下");
        assert_eq!(std::fs::read(&local).unwrap(), b"SAME");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&zip_diff);
        let _ = std::fs::remove_file(&zip_same);
    }
}
