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

    let theme = db.get_setting("theme").map_err(|e| e.to_string())?.unwrap_or_else(|| "dark".to_string());
    let language = db.get_setting("language").map_err(|e| e.to_string())?.unwrap_or_else(|| "zh-CN".to_string());
    let api_key = db.get_setting("steamgriddb_api_key").map_err(|e| e.to_string())?.unwrap_or_default();

    let llm_provider = db.get_setting("llm_provider").map_err(|e| e.to_string())?.unwrap_or_else(|| DEFAULT_LLM_PROVIDER.to_string());
    let llm_protocol = db.get_setting("llm_protocol").map_err(|e| e.to_string())?.unwrap_or_else(|| DEFAULT_LLM_PROTOCOL.to_string());
    let llm_api_key = db.get_setting("llm_api_key").map_err(|e| e.to_string())?.unwrap_or_default();
    let llm_base_url = db.get_setting("llm_base_url").map_err(|e| e.to_string())?.unwrap_or_else(|| DEFAULT_LLM_BASE_URL.to_string());
    let llm_model = db.get_setting("llm_model").map_err(|e| e.to_string())?.unwrap_or_else(|| DEFAULT_LLM_MODEL.to_string());
    let llm_enabled = db.get_setting("llm_enabled").map_err(|e| e.to_string())?.unwrap_or_else(|| "false".to_string()) == "true";

    Ok(Settings {
        theme,
        language,
        steamgriddb_api_key: api_key,
        llm_provider,
        llm_protocol,
        llm_api_key,
        llm_base_url,
        llm_model,
        llm_enabled,
    })
}

/// 保存设置
#[tauri::command]
pub fn save_settings(
    db: State<'_, Arc<Mutex<Database>>>,
    settings: Settings,
) -> Result<(), String> {
    let db = lock_or_recover(&db);

    db.set_setting("theme", &settings.theme).map_err(|e| e.to_string())?;
    db.set_setting("language", &settings.language).map_err(|e| e.to_string())?;
    db.set_setting("steamgriddb_api_key", &settings.steamgriddb_api_key).map_err(|e| e.to_string())?;

    db.set_setting("llm_provider", &settings.llm_provider).map_err(|e| e.to_string())?;
    db.set_setting("llm_protocol", &settings.llm_protocol).map_err(|e| e.to_string())?;
    db.set_setting("llm_api_key", &settings.llm_api_key).map_err(|e| e.to_string())?;
    db.set_setting("llm_base_url", &settings.llm_base_url).map_err(|e| e.to_string())?;
    db.set_setting("llm_model", &settings.llm_model).map_err(|e| e.to_string())?;
    db.set_setting("llm_enabled", &settings.llm_enabled.to_string()).map_err(|e| e.to_string())?;

    Ok(())
}
