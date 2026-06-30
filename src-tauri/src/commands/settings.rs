use tauri::State;
use std::sync::{Arc, Mutex};
use crate::core::Database;
use crate::models::settings::*;
use super::lock_or_recover;
use tauri_plugin_autostart::AutoLaunchManager;

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

/// 获取开机自启动状态
#[tauri::command]
pub fn get_autostart_enabled(
    manager: State<'_, AutoLaunchManager>,
) -> Result<bool, String> {
    manager.is_enabled().map_err(|e| e.to_string())
}

/// 设置开机自启动
#[tauri::command]
pub fn set_autostart_enabled(
    manager: State<'_, AutoLaunchManager>,
    enabled: bool,
) -> Result<(), String> {
    if enabled {
        manager.enable().map_err(|e| e.to_string())
    } else {
        manager.disable().map_err(|e| e.to_string())
    }
}
