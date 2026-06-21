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
    #[serde(default)]
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
}

fn default_accent_color() -> String {
    "#6366f1".to_string()
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
        })
    }

    /// 保存设置到数据库
    pub fn save_to_db(&self, db: &crate::core::Database) -> anyhow::Result<()> {
        db.set_setting("theme", &self.theme)?;
        db.set_setting("language", &self.language)?;
        db.set_setting("steamgriddb_api_key", &self.steamgriddb_api_key)?;
        db.set_setting("llm_protocol", &self.llm_protocol)?;
        db.set_setting("llm_api_key", &self.llm_api_key)?;
        db.set_setting("llm_base_url", &self.llm_base_url)?;
        db.set_setting("llm_model", &self.llm_model)?;
        db.set_setting("llm_enabled", &self.llm_enabled.to_string())?;
        db.set_setting("accent_color", &self.accent_color)?;
        Ok(())
    }
}
