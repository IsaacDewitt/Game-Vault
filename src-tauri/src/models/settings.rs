use serde::{Deserialize, Serialize};

// LLM 默认配置常量（统一来源，避免多处硬编码不一致）
pub const DEFAULT_LLM_PROVIDER: &str = "xiaomi";
pub const DEFAULT_LLM_PROTOCOL: &str = "openai";
pub const DEFAULT_LLM_BASE_URL: &str = "https://api.xiaomimimo.com/v1";
pub const DEFAULT_LLM_MODEL: &str = "mimo-v2.5-pro";

/// 应用设置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub theme: String,
    pub language: String,
    pub steamgriddb_api_key: String,
    /// LLM 提供商：xiaomi / deepseek
    #[serde(default)]
    pub llm_provider: String,
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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            language: "zh-CN".to_string(),
            steamgriddb_api_key: String::new(),
            llm_provider: DEFAULT_LLM_PROVIDER.to_string(),
            llm_protocol: DEFAULT_LLM_PROTOCOL.to_string(),
            llm_api_key: String::new(),
            llm_base_url: DEFAULT_LLM_BASE_URL.to_string(),
            llm_model: DEFAULT_LLM_MODEL.to_string(),
            llm_enabled: false,
        }
    }
}
