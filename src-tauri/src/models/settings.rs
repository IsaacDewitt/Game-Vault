use serde::{Deserialize, Serialize};

// LLM 默认配置常量（统一来源，避免多处硬编码不一致）
pub const DEFAULT_LLM_PROTOCOL: &str = "openai";
pub const DEFAULT_LLM_BASE_URL: &str = "https://api.xiaomimimo.com/v1";
pub const DEFAULT_LLM_MODEL: &str = "mimo-v2.5-pro";

/// 应用设置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub theme: String,
    pub language: String,
    pub steamgriddb_api_key: String,
    /// LLM 协议：openai / anthropic
    #[serde(default = "default_llm_protocol")]
    pub llm_protocol: String,
    /// LLM API Key
    #[serde(default)]
    pub llm_api_key: String,
    /// LLM Base URL
    #[serde(default)]
    pub llm_base_url: String,
    /// LLM 模型名称
    #[serde(default)]
    pub llm_model: String,
    /// 是否启用 LLM 获取游戏信息
    #[serde(default)]
    pub llm_enabled: bool,
    /// 主题色（hex）
    #[serde(default = "default_accent_color")]
    pub accent_color: String,
    /// 窗口宽度
    #[serde(default = "default_window_width")]
    pub window_width: u32,
    /// 窗口高度
    #[serde(default = "default_window_height")]
    pub window_height: u32,
    /// 截图保存目录（默认 %USERPROFILE%\Videos，与 NVIDIA App 一致）
    #[serde(default = "default_screenshot_dir")]
    pub screenshot_dir: String,
    /// 截图快捷键（默认 "F12"，与 Steam 一致）
    #[serde(default = "default_screenshot_hotkey")]
    pub screenshot_hotkey: String,
    /// 手柄截图键（组合键，形如 `LB+A`；空串 = 未设置）
    ///
    /// 与键盘热键的区别：手柄没有"全局热键"这种系统机制，只能靠 XInput 轮询
    /// （见 `core/gamepad.rs`）。故它既不需要注册、也不可能与别的程序冲突。
    #[serde(default)]
    pub screenshot_gamepad_hotkey: String,
}

fn default_accent_color() -> String {
    "#6366f1".to_string()
}

fn default_llm_protocol() -> String {
    DEFAULT_LLM_PROTOCOL.to_string()
}

fn default_window_width() -> u32 {
    1400
}

fn default_window_height() -> u32 {
    900
}

pub fn default_screenshot_dir() -> String {
    // 与 NVIDIA App / GeForce Experience 默认截图目录保持一致：
    // %USERPROFILE%\Videos（Windows 保留的英文物理目录名，中文系统下同样有效）
    dirs::home_dir()
        .map(|p| p.join("Videos").to_string_lossy().to_string())
        .unwrap_or_else(|| "%USERPROFILE%\\Videos".to_string())
}

fn default_screenshot_hotkey() -> String {
    "F12".to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            language: "zh-CN".to_string(),
            steamgriddb_api_key: String::new(),
            llm_protocol: DEFAULT_LLM_PROTOCOL.to_string(),
            llm_api_key: String::new(),
            llm_base_url: DEFAULT_LLM_BASE_URL.to_string(),
            llm_model: DEFAULT_LLM_MODEL.to_string(),
            llm_enabled: false,
            accent_color: default_accent_color(),
            window_width: default_window_width(),
            window_height: default_window_height(),
            screenshot_dir: default_screenshot_dir(),
            screenshot_hotkey: default_screenshot_hotkey(),
            screenshot_gamepad_hotkey: String::new(),
        }
    }
}

impl Settings {
    /// 从数据库加载设置
    pub fn load_from_db(db: &crate::core::Database) -> anyhow::Result<Self> {
        let get = |key: &str, default: &str| -> anyhow::Result<String> {
            Ok(db.get_setting(key)?.unwrap_or_else(|| default.to_string()))
        };

        Ok(Self {
            theme: get("theme", "dark")?,
            language: get("language", "zh-CN")?,
            steamgriddb_api_key: get("steamgriddb_api_key", "")?,
            llm_protocol: get("llm_protocol", DEFAULT_LLM_PROTOCOL)?,
            llm_api_key: get("llm_api_key", "")?,
            llm_base_url: get("llm_base_url", DEFAULT_LLM_BASE_URL)?,
            llm_model: get("llm_model", DEFAULT_LLM_MODEL)?,
            llm_enabled: get("llm_enabled", "false")? == "true",
            accent_color: get("accent_color", &default_accent_color())?,
            window_width: get("window_width", &default_window_width().to_string())?
                .parse()
                .unwrap_or(default_window_width()),
            window_height: get("window_height", &default_window_height().to_string())?
                .parse()
                .unwrap_or(default_window_height()),
            screenshot_dir: get("screenshot_dir", &default_screenshot_dir())?,
            screenshot_hotkey: get("screenshot_hotkey", &default_screenshot_hotkey())?,
            screenshot_gamepad_hotkey: get("screenshot_gamepad_hotkey", "")?,
        })
    }

    /// 保存设置到数据库。
    ///
    /// 走**单个事务**批量写入（2026-09-23 起）：此前是 14 次独立的 `set_setting`，
    /// 每条各自提交，中途失败（磁盘满、权限、进程被杀）会留下半套配置——
    /// 典型症状是"API Key 明明填了却不生效"，因为同一批里的 Base URL 没写进去。
    ///
    /// 实现上把全部字段收进**一次** `set_settings_batch` 调用（数值型也先转成字符串），
    /// 拆成两次调用就变成两个事务了，达不到原子性。
    pub fn save_to_db(&self, db: &crate::core::Database) -> anyhow::Result<()> {
        let owned: Vec<(String, String)> = vec![
            ("theme".to_string(), self.theme.clone()),
            ("language".to_string(), self.language.clone()),
            (
                "steamgriddb_api_key".to_string(),
                self.steamgriddb_api_key.clone(),
            ),
            ("llm_protocol".to_string(), self.llm_protocol.clone()),
            ("llm_api_key".to_string(), self.llm_api_key.clone()),
            ("llm_base_url".to_string(), self.llm_base_url.clone()),
            ("llm_model".to_string(), self.llm_model.clone()),
            (
                "llm_enabled".to_string(),
                if self.llm_enabled { "true" } else { "false" }.to_string(),
            ),
            ("accent_color".to_string(), self.accent_color.clone()),
            ("window_width".to_string(), self.window_width.to_string()),
            ("window_height".to_string(), self.window_height.to_string()),
            (
                "screenshot_dir".to_string(),
                self.screenshot_dir.clone(),
            ),
            (
                "screenshot_hotkey".to_string(),
                self.screenshot_hotkey.clone(),
            ),
            (
                "screenshot_gamepad_hotkey".to_string(),
                self.screenshot_gamepad_hotkey.clone(),
            ),
        ];
        let refs: Vec<(&str, &str)> = owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        db.set_settings_batch(&refs)?;
        Ok(())
    }
}
