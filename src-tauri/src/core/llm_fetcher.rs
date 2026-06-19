use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use reqwest::Client;
use crate::models::settings::{DEFAULT_LLM_BASE_URL, DEFAULT_LLM_MODEL};

/// LLM 协议类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LlmProtocol {
    Openai,
    Anthropic,
}

/// LLM 提供商
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LlmProvider {
    Xiaomi,
    Deepseek,
}

/// LLM 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub enabled: bool,
    pub provider: LlmProvider,
    pub protocol: LlmProtocol,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: LlmProvider::Xiaomi,
            protocol: LlmProtocol::Openai,
            api_key: String::new(),
            base_url: DEFAULT_LLM_BASE_URL.to_string(),
            model: DEFAULT_LLM_MODEL.to_string(),
        }
    }
}

/// LLM 返回的游戏元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmGameMeta {
    /// LLM 纠正后的完整游戏名称（与原始输入语言一致）
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub developer: Option<String>,
    #[serde(default)]
    pub publisher: Option<String>,
    #[serde(default)]
    pub release_date: Option<String>,
    /// LLM 可能返回 null 或 []，统一用 deserialize_with 处理
    #[serde(default, deserialize_with = "deserialize_nullable_vec")]
    pub genres: Vec<String>,
    /// HLTB 主线时长（分钟）
    #[serde(default)]
    pub hltb_main_story: Option<u32>,
    /// HLTB 主线+支线时长（分钟）
    #[serde(default)]
    pub hltb_main_extra: Option<u32>,
    /// HLTB 完美通关时长（分钟）
    #[serde(default)]
    pub hltb_completionist: Option<u32>,
}

/// 反序列化：null / [] / ["a","b"] 都能正确处理
fn deserialize_nullable_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<Vec<String>>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

/// 构建 system prompt
fn build_system_prompt() -> String {
    "你是一个游戏信息查询助手。用户会给你一个游戏名称，你需要提供该游戏的结构化信息。\n\
     \n\
     【重要】你的回复必须且只能是一个合法的 JSON 对象，不要添加任何其他文字、解释、前缀或 markdown 标记。\n\
     \n\
     JSON 格式如下：\n\
     {\n\
       \"name\": \"游戏的完整正式名称\",\n\
       \"description\": \"游戏的简短描述（中文，100字以内）\",\n\
       \"developer\": \"开发商名称\",\n\
       \"publisher\": \"发行商名称\",\n\
       \"release_date\": \"发售日期（格式：YYYY-MM-DD）\",\n\
       \"genres\": [\"类型1\", \"类型2\"],\n\
       \"hltb_main_story\": 主线通关时长（分钟，整数，无法确定填 null）,\n\
       \"hltb_main_extra\": 主线+支线时长（分钟，整数，无法确定填 null）,\n\
       \"hltb_completionist\": 完美通关时长（分钟，整数，无法确定填 null）\n\
     }\n\
     \n\
     注意事项：\n\
     - 如果用户输入的名称是缩写、不完整或有误，请返回该游戏最正确、最完整的正式名称，语言与用户输入保持一致\n\
     - 如果名称已经是正确的，也请返回完整的正式名称\n\
     - 某项信息确实无法确定时，填 null\n\
     - genres 不确定时填空数组 []\n\
     - hltb 时长请根据 HowLongToBeat 数据或你的知识估算，单位为分钟。如《塞尔达传说：旷野之息》主线约50小时则填 3000\n\
     - 不要用 markdown 代码块包裹，直接返回 JSON"
        .to_string()
}

/// 构建 user prompt
fn build_user_prompt(game_name: &str) -> String {
    format!("请提供游戏《{}》的信息。", game_name)
}

/// 从 LLM 响应文本中提取 JSON
fn extract_json(text: &str) -> Result<LlmGameMeta> {
    // 预处理：如果模型返回了工具调用文本，去掉它再重试
    let cleaned = strip_tool_call_tags(text);
    let working_text = cleaned.as_deref().unwrap_or(text);

    // 尝试直接解析
    match serde_json::from_str::<LlmGameMeta>(working_text) {
        Ok(meta) => return Ok(meta),
        Err(e) => tracing::debug!("直接 JSON 解析失败: {}", e),
    }

    // 尝试提取 ```json ... ``` 代码块
    if let Some(start) = working_text.find("```json") {
        let json_start = start + 7;
        if let Some(end) = working_text[json_start..].find("```") {
            let json_str = working_text[json_start..json_start + end].trim();
            tracing::info!("从 markdown 代码块提取 JSON: {}", json_str);
            match serde_json::from_str::<LlmGameMeta>(json_str) {
                Ok(meta) => return Ok(meta),
                Err(e) => tracing::warn!("markdown 代码块 JSON 解析失败: {}，内容: {}", e, json_str),
            }
        }
    }

    // 尝试找到第一个 { 和最后一个 }
    if let Some(start) = working_text.find('{') {
        if let Some(end) = working_text.rfind('}') {
            if end > start {
                let json_str = &working_text[start..=end];
                tracing::info!("从花括号提取 JSON: {}", json_str);
                match serde_json::from_str::<LlmGameMeta>(json_str) {
                    Ok(meta) => return Ok(meta),
                    Err(e) => tracing::warn!("花括号 JSON 解析失败: {}，内容: {}", e, json_str),
                }
            }
        }
    }

    tracing::error!("所有 JSON 提取策略均失败，原始文本:\n{}", text);
    // 截取前 500 字符附在错误信息中，方便前端直接查看
    let preview = if text.len() > 500 { &text[..500] } else { text };
    anyhow::bail!("无法解析 LLM 返回的游戏信息。原始响应:\n{}", preview)
}

/// 去掉模型返回的工具调用标签（如果模型以文本形式返回了工具调用）
fn strip_tool_call_tags(text: &str) -> Option<String> {
    if !text.contains("<tool_call>") {
        return None;
    }
    // 去掉 <tool_call>...</tool_call> 部分
    let mut result = String::new();
    let mut remaining = text;
    while let Some(start) = remaining.find("<tool_call>") {
        result.push_str(&remaining[..start]);
        if let Some(end) = remaining[start..].find("</tool_call>") {
            remaining = &remaining[start + end + 12..]; // 12 = len("</tool_call>")
        } else {
            tracing::warn!("发现 </tool_call> 标签但缺少 </tool_call> 闭合标签，截断剩余内容");
            break;
        }
    }
    result.push_str(remaining);
    let result = result.trim();
    if result.is_empty() {
        None
    } else {
        Some(result.to_string())
    }
}

/// 发送 LLM 请求并获取游戏元数据（异步版本）
pub async fn fetch_game_meta(config: &LlmConfig, game_name: &str) -> Result<LlmGameMeta> {
    if config.api_key.is_empty() {
        anyhow::bail!("未配置 LLM API Key");
    }
    if config.base_url.is_empty() {
        anyhow::bail!("未配置 LLM Base URL");
    }

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .context("创建 HTTP 客户端失败")?;

    let system_prompt = build_system_prompt();
    let user_prompt = build_user_prompt(game_name);

    // 根据协议构建请求体
    let body = match config.protocol {
        LlmProtocol::Openai => build_openai_request(config, &system_prompt, &user_prompt),
        LlmProtocol::Anthropic => build_anthropic_request(config, &system_prompt, &user_prompt),
    };

    tracing::info!("LLM 请求 URL: {}, 体: {}", config.base_url, body);

    // 构建 URL 和 headers
    let url = match config.protocol {
        LlmProtocol::Openai => format!("{}/chat/completions", config.base_url.trim_end_matches('/')),
        LlmProtocol::Anthropic => {
            let base = config.base_url.trim_end_matches('/');
            if base.ends_with("/v1") {
                format!("{}/messages", base)
            } else {
                format!("{}/v1/messages", base)
            }
        }
    };

    let mut req = client.post(&url).header("Content-Type", "application/json");

    // 认证头：Xiaomi 用 api-key，DeepSeek/OpenAI 用 Authorization: Bearer，Anthropic 用 x-api-key
    req = match (&config.protocol, &config.provider) {
        (LlmProtocol::Openai, LlmProvider::Xiaomi) => {
            req.header("api-key", &config.api_key)
        }
        (LlmProtocol::Openai, _) => {
            req.header("Authorization", format!("Bearer {}", config.api_key))
        }
        (LlmProtocol::Anthropic, _) => {
            req.header("x-api-key", &config.api_key)
                .header("anthropic-version", "2023-06-01")
        }
    };

    let resp = req
        .json(&body)
        .send()
        .await
        .context("发送 LLM 请求失败")?;

    let status = resp.status();
    if !status.is_success() {
        let err_text = resp.text().await.unwrap_or_default();
        anyhow::bail!("LLM API 返回错误 {}: {}", status, err_text);
    }

    let resp_json: serde_json::Value = resp.json().await.context("解析 LLM 响应失败")?;

    tracing::info!("LLM 原始响应: {}", resp_json);

    // 根据协议提取文本内容
    let text = extract_response_text(&config.protocol, &resp_json)?;

    tracing::info!("LLM 响应文本: {}", text);

    extract_json(&text)
}

/// 构建 OpenAI 格式请求体
fn build_openai_request(
    config: &LlmConfig,
    system_prompt: &str,
    user_prompt: &str,
) -> serde_json::Value {
    let body = serde_json::json!({
        "model": config.model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_prompt}
        ],
        "max_completion_tokens": 1024,
        "temperature": 0.3,
        "stream": false
    });

    // 注：Xiaomi MiMo 的自定义 web_search 工具会导致模型以文本形式输出工具调用
    // 而非真正执行搜索，故不在 OpenAI 协议下使用工具。模型自身知识足以覆盖常见游戏。
    // DeepSeek 同理，使用模型自身知识即可。

    body
}

/// 构建 Anthropic 格式请求体
fn build_anthropic_request(
    config: &LlmConfig,
    system_prompt: &str,
    user_prompt: &str,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": config.model,
        "max_tokens": 1024,
        "system": system_prompt,
        "messages": [
            {"role": "user", "content": user_prompt}
        ]
    });

    // Anthropic 原生 web_search 工具（Brave Search 后端）
    // 类型为 web_search_20250305，由 Anthropic 服务端执行搜索
    body["tools"] = serde_json::json!([
        {
            "type": "web_search_20250305",
            "name": "web_search",
            "max_uses": 5
        }
    ]);

    body
}

/// 从响应 JSON 中提取文本内容
fn extract_response_text(protocol: &LlmProtocol, resp: &serde_json::Value) -> Result<String> {
    match protocol {
        LlmProtocol::Openai => {
            // OpenAI: choices[0].message.content
            let message = &resp["choices"][0]["message"];

            // 优先取 content 字段
            if let Some(content) = message["content"].as_str() {
                if !content.is_empty() {
                    return Ok(content.to_string());
                }
            }

            // 备选：reasoning_content 字段（某些模型使用）
            if let Some(content) = message["reasoning_content"].as_str() {
                if !content.is_empty() {
                    return Ok(content.to_string());
                }
            }

            // 如果有 tool_calls 但没有 content，说明模型在调用工具但没返回文本
            if message.get("tool_calls").is_some() && !message["tool_calls"].is_null() {
                tracing::warn!("模型返回了 tool_calls 但没有 content: {}", message);
                anyhow::bail!("模型返回了工具调用但没有文本内容，请重试")
            }

            tracing::error!("OpenAI 响应中找不到有效 content，完整响应: {}", resp);
            anyhow::bail!("OpenAI 响应中找不到 content 字段")
        }
        LlmProtocol::Anthropic => {
            // Anthropic: content[0].text (content 是数组，取第一个 text 类型)
            let content_arr = resp["content"]
                .as_array()
                .context("Anthropic 响应中找不到 content 数组")?;

            for block in content_arr {
                if let Some(text) = block["text"].as_str() {
                    if !text.is_empty() {
                        return Ok(text.to_string());
                    }
                }
            }

            tracing::error!("Anthropic 响应中找不到 text 内容块，完整响应: {}", resp);
            anyhow::bail!("Anthropic 响应中找不到 text 内容块")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_direct() {
        let text = r#"{"description":"测试游戏","developer":"测试开发商","publisher":"测试发行商","release_date":"2024-01-01","genres":["RPG"]}"#;
        let meta = extract_json(text).unwrap();
        assert_eq!(meta.description.unwrap(), "测试游戏");
        assert_eq!(meta.genres.len(), 1);
    }

    #[test]
    fn test_extract_json_from_markdown() {
        let text = "这是游戏信息：\n```json\n{\"description\":\"测试\",\"developer\":null,\"publisher\":null,\"release_date\":null,\"genres\":[]}\n```\n以上是信息。";
        let meta = extract_json(text).unwrap();
        assert_eq!(meta.description.unwrap(), "测试");
    }
}
