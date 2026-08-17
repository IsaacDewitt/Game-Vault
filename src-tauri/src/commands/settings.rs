use tauri::State;
use std::sync::{Arc, Mutex};
use crate::core::Database;
use crate::models::settings::*;
use super::lock_or_recover;
use tauri_plugin_autostart::AutoLaunchManager;
use tauri::Manager;

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

/// 设置窗口大小并持久化到数据库
#[tauri::command]
pub fn set_window_size(
    app: tauri::AppHandle,
    db: State<'_, Arc<Mutex<Database>>>,
    width: u32,
    height: u32,
) -> Result<(), String> {
    // 调整窗口大小
    if let Some(window) = app.get_webview_window("main") {
        let size = tauri::PhysicalSize::new(width, height);
        window.set_size(tauri::Size::Physical(size)).map_err(|e| e.to_string())?;
    }

    // 持久化到数据库
    let db = lock_or_recover(&db);
    db.set_setting("window_width", &width.to_string()).map_err(|e| e.to_string())?;
    db.set_setting("window_height", &height.to_string()).map_err(|e| e.to_string())?;

    Ok(())
}
