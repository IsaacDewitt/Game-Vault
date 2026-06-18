use tauri::State;
use std::sync::{Arc, Mutex};
use crate::core::Database;
use crate::models::*;
use super::lock_or_recover;

/// 获取游戏时长排行榜
#[tauri::command]
pub fn get_play_stats(
    db: State<'_, Arc<Mutex<Database>>>,
    limit: Option<u32>,
) -> Result<Vec<GamePlayStats>, String> {
    let db = lock_or_recover(&db);
    let limit = limit.unwrap_or(20);
    db.get_play_stats(limit).map_err(|e| e.to_string())
}

/// 获取每日游玩统计
#[tauri::command]
pub fn get_daily_stats(
    db: State<'_, Arc<Mutex<Database>>>,
    days: Option<u32>,
) -> Result<Vec<DailyStats>, String> {
    let db = lock_or_recover(&db);
    let days = days.unwrap_or(30);
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

    // 使用本地时间计算今日日期
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();

    // 只查询近30天的统计数据
    let daily_stats = db.get_daily_stats(30).map_err(|e| e.to_string())?;
    let monthly_seconds: u64 = daily_stats.iter().map(|s| s.total_seconds).sum();

    let today_seconds = daily_stats.iter()
        .find(|s| s.date == today)
        .map(|s| s.total_seconds)
        .unwrap_or(0);

    Ok(serde_json::json!({
        "game_count": game_count,
        "total_play_time": total_play_time,
        "monthly_play_time": monthly_seconds,
        "today_play_time": today_seconds,
    }))
}
