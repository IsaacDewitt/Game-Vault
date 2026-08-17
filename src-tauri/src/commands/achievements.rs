use tauri::{AppHandle, Emitter, State};
use std::sync::{Arc, Mutex};
use crate::core::{Database, AchievementEngine};
use crate::models::*;
use super::lock_or_recover;

/// 获取成就汇总（全局 + 每游戏成就状态）
#[tauri::command]
pub fn get_achievements(
    db: State<'_, Arc<Mutex<Database>>>,
) -> Result<AchievementSummary, String> {
    let db = lock_or_recover(&db);
    AchievementEngine::get_summary(&db).map_err(|e| e.to_string())
}

/// 手动触发成就检测（前端在增删改等操作后调用）
/// 返回本次新解锁的事件列表，由前端弹通知展示
#[tauri::command]
pub fn check_achievements(
    app: AppHandle,
    db: State<'_, Arc<Mutex<Database>>>,
) -> Result<Vec<UnlockEvent>, String> {
    let db = lock_or_recover(&db);
    let events = AchievementEngine::evaluate(&db).map_err(|e| e.to_string())?;
    if !events.is_empty() {
        // 通知前端弹出解锁动画
        let _ = app.emit("achievement-unlocked", &events);
    }
    Ok(events)
}
