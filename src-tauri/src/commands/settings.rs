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

/// 截图热键变更后重新注册全局热键（save_settings / save_settings_partial 共用）。
/// 当前注册值由 hotkey_state 内存态持有，与其不同才动手；注册失败写入 hotkey_error 供前端提示。
fn reapply_screenshot_hotkey(
    app: &tauri::AppHandle,
    db: &tauri::State<'_, Arc<Mutex<Database>>>,
    hotkey_state: &tauri::State<'_, Arc<Mutex<String>>>,
    hotkey_error: &tauri::State<'_, Arc<Mutex<Option<String>>>>,
    new_hotkey: &str,
) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let old_hotkey = hotkey_state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if old_hotkey == new_hotkey {
        return;
    }
    if !old_hotkey.trim().is_empty() {
        let _ = app.global_shortcut().unregister(old_hotkey.as_str());
    }
    let tracker = app
        .state::<Arc<Mutex<crate::core::PlayTimeTracker>>>()
        .inner()
        .clone();
    let reg_result = crate::commands::screenshots::register_screenshot_hotkey(
        app,
        new_hotkey,
        db.inner().clone(),
        tracker,
    );
    // 同步更新热键注册错误状态（前端设置页可据此提示）
    let reg_ok = reg_result.is_ok();
    let mut err_state = hotkey_error.lock().unwrap_or_else(|e| e.into_inner());
    match reg_result {
        Ok(()) => *err_state = None,
        Err(e) => {
            tracing::error!("重新注册截图热键失败: {e}");
            *err_state = Some(e);
        }
    }
    drop(err_state);
    // 键盘钩子通道同步换键：不换的话新旧两个键都会触发截图（钩子仍按老键位响应）。
    // 注册失败（该键被别的程序占用）时一并停用，避免替别的程序响应按键。
    let parsed = if reg_ok {
        crate::core::hotkey_hook::parse_key_spec(new_hotkey)
    } else {
        None
    };
    crate::core::hotkey_hook::set_spec(parsed);
    // 回读实际生效值（而不是回显解析结果），日志才可信
    match crate::core::hotkey_hook::current_spec() {
        Some(spec) => tracing::info!(
            "截图键盘钩子键位已更新: vk=0x{:02X}（热键 {new_hotkey}）",
            spec.vk
        ),
        None if reg_ok => tracing::info!("截图热键 {new_hotkey} 无对应钩子键位，钩子通道停用"),
        None => tracing::info!("截图热键 {new_hotkey} 注册失败，钩子通道一并停用"),
    }
    let mut state = hotkey_state.lock().unwrap_or_else(|e| e.into_inner());
    *state = new_hotkey.to_string();
}

/// 保存设置（整对象全量写库）——设置页「保存设置」按钮路径，覆盖全部字段
#[tauri::command]
pub fn save_settings(
    app: tauri::AppHandle,
    db: State<'_, Arc<Mutex<Database>>>,
    hotkey_state: State<'_, Arc<Mutex<String>>>,
    hotkey_error: State<'_, Arc<Mutex<Option<String>>>>,
    settings: Settings,
) -> Result<(), String> {
    {
        let db = lock_or_recover(&db);
        settings.save_to_db(&db).map_err(|e| e.to_string())?;
    }

    // 截图热键变更则重新注册
    reapply_screenshot_hotkey(
        &app,
        &db,
        &hotkey_state,
        &hotkey_error,
        &settings.screenshot_hotkey,
    );

    Ok(())
}

/// 自动保存（设置页改动即生效的字段：主题 / 主题色 / 截图目录 / 截图快捷键）。
///
/// 与 save_settings 的区别：patch 里的键经白名单过滤后才写入，其余设置以库中现值保持原样——
/// 不会把用户改了但没点「保存设置」的密钥 / LLM / 窗口尺寸等手动字段顺带落盘。
/// 合并语义（读库现值 → patch 覆盖 → 整写）也天然规避前端 settings 对象旧值污染。
#[tauri::command]
pub fn save_settings_partial(
    app: tauri::AppHandle,
    db: State<'_, Arc<Mutex<Database>>>,
    hotkey_state: State<'_, Arc<Mutex<String>>>,
    hotkey_error: State<'_, Arc<Mutex<Option<String>>>>,
    patch: serde_json::Value,
) -> Result<(), String> {
    // 只允许「即时生效」类字段走自动保存（与 SettingsView 注释约定一致）；
    // 手动字段（steamgriddb/llm_*/window_*）只能经 save_settings 全量提交。
    const AUTO_KEYS: &[&str] = &[
        "theme",
        "accent_color",
        "screenshot_dir",
        "screenshot_hotkey",
        "language",
    ];

    let mut new_hotkey: Option<String> = None;
    {
        let db = lock_or_recover(&db);
        let mut merged = Settings::load_from_db(&db).map_err(|e| e.to_string())?;
        for key in AUTO_KEYS {
            let Some(v) = patch.get(key).and_then(|v| v.as_str()) else {
                continue;
            };
            match *key {
                "theme" => merged.theme = v.to_string(),
                "accent_color" => merged.accent_color = v.to_string(),
                "screenshot_dir" => merged.screenshot_dir = v.to_string(),
                "screenshot_hotkey" => {
                    merged.screenshot_hotkey = v.to_string();
                    new_hotkey = Some(v.to_string());
                }
                "language" => merged.language = v.to_string(),
                _ => unreachable!("AUTO_KEYS 与 match 分支应一一对应"),
            }
        }
        merged.save_to_db(&db).map_err(|e| e.to_string())?;
    }

    // patch 里带了快捷键才做热键重注册（其余字段变化无需动热键）
    if let Some(hk) = new_hotkey {
        reapply_screenshot_hotkey(&app, &db, &hotkey_state, &hotkey_error, &hk);
    }
    Ok(())
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
    // 调整窗口大小。
    // 用逻辑尺寸：前端输入的是逻辑像素（与 tauri.conf.json 的 width/height 同口径），
    // 直接当物理像素用会在高 DPI 缩放屏上得到比预期小的窗口。
    if let Some(window) = app.get_webview_window("main") {
        let size = tauri::LogicalSize::new(width as f64, height as f64);
        window.set_size(tauri::Size::Logical(size)).map_err(|e| e.to_string())?;
    }

    // 持久化到数据库
    let db = lock_or_recover(&db);
    db.set_setting("window_width", &width.to_string()).map_err(|e| e.to_string())?;
    db.set_setting("window_height", &height.to_string()).map_err(|e| e.to_string())?;

    Ok(())
}
