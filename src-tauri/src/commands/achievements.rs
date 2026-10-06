use tauri::{AppHandle, Emitter, State};
use std::sync::{Arc, Mutex};
use crate::core::{Database, AchievementEngine};
use crate::models::*;
use super::lock_or_recover;

/// 获取成就汇总（全局 + 每游戏成就状态）
///
/// **`(async)`（2026-10-05 补）**：与同文件的 `check_achievements` 干的是同一件事
/// （持 DB 锁走 `AchievementEngine::get_summary`），成就页首屏必调；留在主线程会
/// 在锁被后台线程占用时卡住整个 IPC 队列。
#[tauri::command(async)]
pub fn get_achievements(
    db: State<'_, Arc<Mutex<Database>>>,
) -> Result<AchievementSummary, String> {
    let db = lock_or_recover(&db);
    AchievementEngine::get_summary(&db).map_err(|e| e.to_string())
}

/// 手动触发成就检测（前端在增删改等操作后调用）
/// 返回本次新解锁的事件列表，由前端弹通知展示
///
/// **`(async)`（2026-10-03 修）**：本命令持 DB 锁评估全部成就（全局 + 每游戏），
/// 留在主线程会阻塞整个 IPC 队列。
#[tauri::command(async)]
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
