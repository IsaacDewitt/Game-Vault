use tauri::State;
use std::sync::{Arc, Mutex};
use crate::core::Database;
use crate::models::*;
use super::lock_or_recover;

/// 统计查询参数的业务上界（防御性钳位）。
///
/// 这些参数经 IPC 直达数据库层。前端只会传 30 / 365 / 50 这类小值，但命令层
/// 不该假设调用方乖巧：`get_daily_stats` 会按 `days` 逐日补零，且开工就是
/// `Vec::with_capacity(days as usize)`（见 `core/database.rs` 同名方法）——
/// 传进一个 4e9 级的 days 会申请数百 GB 内存并直接 abort 进程。
///
/// **只封上界、不设下界**：加下界会改变 `days = 0` 时的既有语义（当前返回 1 条），
/// 那属于功能变化，不在本次改动范围内。
const STATS_MAX_DAYS: u32 = 3650; // 10 年，远超热力图默认 365 天的实际用量
const STATS_MAX_LIMIT: u32 = 1000; // 远超排行榜 / 明细页的单页需求
const STATS_MAX_OFFSET: u32 = 1_000_000;

/// 获取游戏时长排行榜
#[tauri::command]
pub fn get_play_stats(
    db: State<'_, Arc<Mutex<Database>>>,
    limit: Option<u32>,
) -> Result<Vec<GamePlayStats>, String> {
    let db = lock_or_recover(&db);
    let limit = limit.unwrap_or(20).min(STATS_MAX_LIMIT);
    db.get_play_stats(limit).map_err(|e| e.to_string())
}

/// 获取每日游玩统计
#[tauri::command]
pub fn get_daily_stats(
    db: State<'_, Arc<Mutex<Database>>>,
    days: Option<u32>,
) -> Result<Vec<DailyStats>, String> {
    let db = lock_or_recover(&db);
    let days = days.unwrap_or(30).min(STATS_MAX_DAYS);
    db.get_daily_stats(days).map_err(|e| e.to_string())
}

/// 获取概览统计
#[tauri::command]
pub fn get_overview_stats(
    db: State<'_, Arc<Mutex<Database>>>,
) -> Result<serde_json::Value, String> {
    let db = lock_or_recover(&db);

    let game_count = db.get_game_count().map_err(|e| e.to_string())?;
    let total_play_time = db.get_total_play_time().map_err(|e| e.to_string())?;

    // 本月时长：直接按本地时区的年月聚合（不依赖 30 天窗口的隐含假设）
    let monthly_seconds = db.get_monthly_play_time().map_err(|e| e.to_string())?;

    // 今日时长：直接按本地日期查日汇总（不再为取一条数据拉全 30 天）
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let today_seconds = db.get_day_play_time(&today).unwrap_or(0);

    Ok(serde_json::json!({
        "game_count": game_count,
        "total_play_time": total_play_time,
        "monthly_play_time": monthly_seconds,
        "today_play_time": today_seconds,
    }))
}

/// 获取游戏类型统计
#[tauri::command]
pub fn get_genre_stats(
    db: State<'_, Arc<Mutex<Database>>>,
) -> Result<Vec<GenreStats>, String> {
    let db = lock_or_recover(&db);
    db.get_genre_stats().map_err(|e| e.to_string())
}

/// 获取热力图数据
#[tauri::command]
pub fn get_heatmap_stats(
    db: State<'_, Arc<Mutex<Database>>>,
    days: Option<u32>,
) -> Result<Vec<HeatmapDay>, String> {
    let db = lock_or_recover(&db);
    let days = days.unwrap_or(365).min(STATS_MAX_DAYS);
    db.get_heatmap_stats(days).map_err(|e| e.to_string())
}

/// 获取游玩时段分布
#[tauri::command]
pub fn get_hourly_stats(
    db: State<'_, Arc<Mutex<Database>>>,
) -> Result<Vec<HourlyStats>, String> {
    let db = lock_or_recover(&db);
    db.get_hourly_stats().map_err(|e| e.to_string())
}

/// 获取游戏状态统计
#[tauri::command]
pub fn get_status_stats(
    db: State<'_, Arc<Mutex<Database>>>,
) -> Result<StatusStats, String> {
    let db = lock_or_recover(&db);
    db.get_status_stats().map_err(|e| e.to_string())
}

/// 获取游玩会话历史
#[tauri::command]
pub fn get_play_sessions(
    db: State<'_, Arc<Mutex<Database>>>,
    game_id: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<PlaySessionDetail>, String> {
    let db = lock_or_recover(&db);
    let limit = limit.unwrap_or(50).min(STATS_MAX_LIMIT);
    let offset = offset.unwrap_or(0).min(STATS_MAX_OFFSET);
    db.get_play_sessions(game_id.as_deref(), limit, offset).map_err(|e| e.to_string())
}
