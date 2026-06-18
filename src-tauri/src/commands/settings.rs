use tauri::State;
use std::sync::{Arc, Mutex};
use crate::core::Database;
use crate::models::settings::*;
use super::lock_or_recover;

/// 获取所有设置
#[tauri::command]
pub fn get_settings(
    db: State<'_, Arc<Mutex<Database>>>,
) -> Result<Settings, String> {
    let db = lock_or_recover(&db);
    Settings::load_from_db(&db).map_err(|e| e.to_string())
}

/// 保存设置
#[tauri::command]
pub fn save_settings(
    db: State<'_, Arc<Mutex<Database>>>,
    settings: Settings,
) -> Result<(), String> {
    let db = lock_or_recover(&db);
    settings.save_to_db(&db).map_err(|e| e.to_string())
}
