use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use reqwest::Client;
use crate::models::settings::{DEFAULT_LLM_BASE_URL, DEFAULT_LLM_MODEL};
use crate::utils::constants::*;

/// LLM 协议类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LlmProtocol {
    Openai,
    Anthropic,
}

/// LLM 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub enabled: bool,
    pub protocol: LlmProtocol,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
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
    #[serde(default, deserialize_with = "deserialize_flexible_u32")]
    pub hltb_main_story: Option<u32>,
    /// HLTB 主线+支线时长（分钟）
    #[serde(default, deserialize_with = "deserialize_flexible_u32")]
    pub hltb_main_extra: Option<u32>,
    /// HLTB 完美通关时长（分钟）
    #[serde(default, deserialize_with = "deserialize_flexible_u32")]
    pub hltb_completionist: Option<u32>,
    /// 游戏存档路径列表
    #[serde(default, deserialize_with = "deserialize_nullable_vec")]
    pub save_paths: Vec<String>,
}

/// 反序列化：null / [] / ["a","b"] 都能正确处理
fn deserialize_nullable_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<Vec<String>>::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

/// 反序列化：兼容整数、浮点数（如 1560.0）、字符串数字、null
fn deserialize_flexible_u32<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // 先用 Value 接收任意类型
    let val = serde_json::Value::deserialize(deserializer)?;
    match val {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Number(n) => {
            if let Some(v) = n.as_u64() {
                Ok(Some(v as u32))
            } else if let Some(v) = n.as_f64() {
                // 处理 LLM 返回 1560.0 这种浮点数
                if v >= 0.0 && v <= u32::MAX as f64 {
                    Ok(Some(v as u32))
                } else {
                    Ok(None)
                }
            } else {
                Ok(None)
            }
        }
        serde_json::Value::String(s) => {
            // 处理 LLM 返回 "3000" 这种字符串
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            // 尝试解析 "1560" 或 "1560.0"
            if let Ok(v) = trimmed.parse::<u64>() {
                Ok(Some(v as u32))
            } else if let Ok(v) = trimmed.parse::<f64>() {
                if v >= 0.0 && v <= u32::MAX as f64 {
                    Ok(Some(v as u32))
                } else {
                    Ok(None)
                }
            } else {
                Ok(None)
            }
        }
        _ => Ok(None),
    }
}

/// LLM 游戏信息获取器（复用 HTTP Client）
pub struct LlmFetcher {
    client: Client,
}

impl LlmFetcher {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(LLM_REQUEST_TIMEOUT_SECS))
            .build()
            .context("创建 HTTP 客户端失败")?;
        Ok(Self { client })
    }

    /// 发送 LLM 请求并获取游戏元数据
    pub async fn fetch_game_meta(&self, config: &LlmConfig, game_name: &str) -> Result<LlmGameMeta> {
        if config.api_key.is_empty() {
            anyhow::bail!("未配置 LLM API Key");
        }
        if config.base_url.is_empty() {
            anyhow::bail!("未配置 LLM Base URL");
        }

        let system_prompt = build_system_prompt();
        let user_prompt = build_user_prompt(game_name);

        // 根据协议构建请求体
        let body = match config.protocol {
            LlmProtocol::Openai => build_openai_request(config, &system_prompt, &user_prompt),
            LlmProtocol::Anthropic => build_anthropic_request(config, &system_prompt, &user_prompt),
        };

        tracing::debug!("LLM 请求 URL: {}, 体: {}", config.base_url, body);

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

        let mut req = self.client.post(&url).header("Content-Type", "application/json");

        // 认证头：统一按协议区分
        req = match config.protocol {
            LlmProtocol::Openai => {
                req.header("Authorization", format!("Bearer {}", config.api_key))
            }
            LlmProtocol::Anthropic => {
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

        tracing::debug!("LLM 原始响应: {}", resp_json);

        // 根据协议提取文本内容
        let text = match config.protocol {
            LlmProtocol::Openai => {
                let message = &resp_json["choices"][0]["message"];
                if let Some(tool_calls) = message["tool_calls"].as_array() {
                    if !tool_calls.is_empty() {
                        self.handle_openai_tool_calls(config, resp_json, &system_prompt, &user_prompt).await?
                    } else {
                        extract_response_text(&config.protocol, &resp_json)?
                    }
                } else {
                    extract_response_text(&config.protocol, &resp_json)?
                }
            }
            LlmProtocol::Anthropic => {
                extract_response_text(&config.protocol, &resp_json)?
            }
        };

        tracing::debug!("LLM 响应文本: {}", text);

        let meta = extract_json(&text)?;
        Ok(sanitize_meta(meta))
    }

    /// 处理 OpenAI 工具调用循环
    async fn handle_openai_tool_calls(
        &self,
        config: &LlmConfig,
        initial_response: serde_json::Value,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<String> {
        let mut messages = serde_json::json!([
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_prompt}
        ]);

        let mut current_response = initial_response;

        for _ in 0..LLM_MAX_TOOL_ITERATIONS {
            let message = &current_response["choices"][0]["message"];

            // 检查是否有 tool_calls
            if let Some(tool_calls) = message["tool_calls"].as_array() {
                if !tool_calls.is_empty() {
                    // 把助手消息（含 tool_calls）加入 messages
                    if let Some(arr) = messages.as_array_mut() {
                        arr.push(message.clone());
                    }

                    // 处理每个工具调用
                    for tool_call in tool_calls {
                        let call_id = tool_call["id"].as_str().unwrap_or("");
                        let function_name = tool_call["function"]["name"].as_str().unwrap_or("");
                        let arguments_str = tool_call["function"]["arguments"].as_str().unwrap_or("{}");

                        let arguments: serde_json::Value =
                            serde_json::from_str(arguments_str).unwrap_or(serde_json::json!({}));

                        tracing::debug!("执行工具调用: {}({})", function_name, arguments);

                        // 执行搜索
                        let result = if function_name == "web_search" {
                            let query = arguments["query"].as_str().unwrap_or("");
                            self.execute_web_search(query).await.unwrap_or_else(|e| {
                                format!("搜索失败: {}", e)
                            })
                        } else {
                            format!("未知工具: {}", function_name)
                        };

                        tracing::debug!("工具结果: {}", result);

                        // 添加工具结果到 messages
                        if let Some(arr) = messages.as_array_mut() {
                            arr.push(serde_json::json!({
                                "role": "tool",
                                "tool_call_id": call_id,
                                "content": result
                            }));
                        }
                    }

                    // 重新发送请求（保持相同的工具定义）
                    let body = serde_json::json!({
                        "model": config.model,
                        "messages": messages,
                        "max_completion_tokens": LLM_MAX_TOKENS,
                        "temperature": 0.3,
                        "stream": false,
                        "tools": [
                            {
                                "type": "function",
                                "function": {
                                    "name": "web_search",
                                    "description": "搜索网络获取实时信息",
                                    "parameters": {
                                        "type": "object",
                                        "properties": {
                                            "query": {
                                                "type": "string",
                                                "description": "搜索关键词"
                                            }
                                        },
                                        "required": ["query"]
                                    }
                                }
                            }
                        ]
                    });

                    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));

                    let resp = self.client
                        .post(&url)
                        .header("Content-Type", "application/json")
                        .header("Authorization", format!("Bearer {}", config.api_key))
                        .json(&body)
                        .send()
                        .await
                        .context("发送后续 LLM 请求失败")?;

                    let status = resp.status();
                    if !status.is_success() {
                        let err_text = resp.text().await.unwrap_or_default();
                        anyhow::bail!("LLM API 返回错误 {}: {}", status, err_text);
                    }

                    current_response = resp.json().await.context("解析后续 LLM 响应失败")?;
                    continue;
                }
            }

            // 没有 tool_calls，提取 content
            if let Some(content) = message["content"].as_str() {
                if !content.is_empty() {
                    return Ok(content.to_string());
                }
            }

            // content 为空时，检查 finish_reason 判断是否被截断
            let finish_reason = current_response["choices"][0]["finish_reason"]
                .as_str()
                .unwrap_or("unknown");
            if finish_reason == "length" {
                anyhow::bail!("LLM 响应被截断（token 不足），请重试");
            }

            anyhow::bail!("模型未返回有效内容，请重试");
        }

        anyhow::bail!("工具调用循环超过最大次数")
    }

    /// 执行网络搜索（使用 SearXNG JSON API，多实例 fallback）
    async fn execute_web_search(&self, query: &str) -> Result<String> {
        let search_query = if !query.to_lowercase().contains("game") {
            format!("{} game", query)
        } else {
            query.to_string()
        };

        let encoded_query = urlencoding::encode(&search_query);

        // 依次尝试 SearXNG 公开实例
        let mut last_error = String::new();
        for instance in SEARCH_ENGINE_INSTANCES {
            let url = format!(
                "{}/search?q={}&format=json&categories=general&language=zh-CN",
                instance.trim_end_matches('/'),
                encoded_query
            );

            let result = self.try_search_request(&url).await;
            match result {
                Ok(text) if !text.is_empty() => return Ok(text),
                Ok(_) => {
                    tracing::debug!("SearXNG 实例 {} 返回空结果，尝试下一个", instance);
                    last_error = format!("{}: 返回空结果", instance);
                }
                Err(e) => {
                    tracing::debug!("SearXNG 实例 {} 请求失败: {}，尝试下一个", instance, e);
                    last_error = format!("{}: {}", instance, e);
                }
            }
        }

        anyhow::bail!("所有搜索引擎实例均不可用，最后错误: {}", last_error)
    }

    /// 向单个 SearXNG 实例发送搜索请求
    async fn try_search_request(&self, url: &str) -> Result<String> {
        let resp = self.client
            .get(url)
            .timeout(std::time::Duration::from_secs(WEB_SEARCH_TIMEOUT_SECS))
            .send()
            .await
            .context("发送搜索请求失败")?;

        if !resp.status().is_success() {
            anyhow::bail!("搜索 API 返回错误: {}", resp.status());
        }

        let json: serde_json::Value = resp.json().await.context("解析搜索结果失败")?;

        // 从 results 数组提取搜索结果
        let mut results = Vec::new();

        if let Some(items) = json["results"].as_array() {
            for (i, item) in items.iter().take(SEARCH_RESULT_LIMIT).enumerate() {
                let title = item["title"].as_str().unwrap_or("");
                let content = item["content"].as_str().unwrap_or("");
                let url = item["url"].as_str().unwrap_or("");

                if title.is_empty() && content.is_empty() {
                    continue;
                }

                let mut entry = format!("{}. {}", i + 1, title);
                if !content.is_empty() {
                    entry.push_str(&format!(": {}", content));
                }
                if !url.is_empty() {
                    entry.push_str(&format!(" [{}]", url));
                }
                results.push(entry);
            }
        }

        // 如果有 answer 字段（SearXNG 的即时回答），加在最前面
        if let Some(answer) = json["answers"].as_array().and_then(|a| a.first()) {
            if let Some(text) = answer["answer"].as_str() {
                if !text.is_empty() {
                    results.insert(0, format!("即时回答: {}", text));
                }
            }
        }

        if results.is_empty() {
            Ok(String::new())
        } else {
            Ok(results.join("\n"))
        }
    }
}

/// 构建 system prompt（含 few-shot 示例）
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
       \"hltb_completionist\": 完美通关时长（分钟，整数，无法确定填 null）,\n\
       \"save_paths\": [\"存档路径1\", \"存档路径2\"]\n\
     }\n\
     \n\
     示例输入：空洞骑士\n\
     示例输出：\n\
     {\n\
       \"name\": \"空洞骑士\",\n\
       \"description\": \"Team Cherry 开发的 2D 类银河战士恶魔城游戏，玩家将探索庞大的地下昆虫王国，对抗被感染的生物，揭开远古秘密。\",\n\
       \"developer\": \"Team Cherry\",\n\
       \"publisher\": \"Team Cherry\",\n\
       \"release_date\": \"2017-02-24\",\n\
       \"genres\": [\"类银河战士恶魔城\", \"动作\", \"独立\"],\n\
       \"hltb_main_story\": 1560,\n\
       \"hltb_main_extra\": 2340,\n\
       \"hltb_completionist\": 3780,\n\
       \"save_paths\": [\"%%USERPROFILE%%\\AppData\\LocalLow\\Team Cherry\\Hollow Knight\"]\n\
     }\n\
     \n\
     注意事项：\n\
     - 如果用户输入的名称是缩写、不完整或有误，请返回该游戏最正确、最完整的正式名称，语言与用户输入保持一致\n\
     - 如果名称已经是正确的，也请返回完整的正式名称\n\
     - 某项信息确实无法确定时，填 null\n\
     - genres 不确定时填空数组 []\n\
     - release_date 必须严格使用 YYYY-MM-DD 格式\n\
     - hltb 时长根据 HowLongToBeat 数据或你的知识估算，单位为分钟（整数），如《塞尔达传说：旷野之息》主线约50小时则填 3000\n\
     - save_paths 为该游戏存档文件或存档文件夹的常见路径。支持 %%APPDATA%%、%%USERPROFILE%%、%%LOCALAPPDATA%% 等 Windows 环境变量\n\
     - 如果无法确定存档路径，save_paths 填空数组 []\n\
     - 如果搜索到的信息与你的知识冲突，以搜索结果为准\n\
     - 不要用 markdown 代码块包裹，直接返回 JSON"
        .to_string()
}

/// 构建 user prompt
fn build_user_prompt(game_name: &str) -> String {
    format!("请提供游戏《{}》的信息。", game_name)
}

/// 对 LLM 返回的元数据做后处理校验
fn sanitize_meta(mut meta: LlmGameMeta) -> LlmGameMeta {
    // 1. 所有字符串字段 trim
    if let Some(ref mut name) = meta.name {
        *name = name.trim().to_string();
        if name.is_empty() {
            meta.name = None;
        }
    }
    if let Some(ref mut desc) = meta.description {
        *desc = desc.trim().to_string();
        if desc.is_empty() {
            meta.description = None;
        }
    }
    if let Some(ref mut dev) = meta.developer {
        *dev = dev.trim().to_string();
        if dev.is_empty() {
            meta.developer = None;
        }
    }
    if let Some(ref mut pub_) = meta.publisher {
        *pub_ = pub_.trim().to_string();
        if pub_.is_empty() {
            meta.publisher = None;
        }
    }

    // 2. release_date 格式化：尝试从常见格式转换为 YYYY-MM-DD
    if let Some(ref date) = meta.release_date {
        let trimmed = date.trim().to_string();
        if trimmed.is_empty() {
            meta.release_date = None;
        } else {
            meta.release_date = normalize_date(&trimmed).or(Some(trimmed));
        }
    }

    // 3. hltb 异常值过滤：超过 100000 分钟（约 69 天）视为异常
    meta.hltb_main_story = meta.hltb_main_story.filter(|&v| v > 0 && v < 100_000);
    meta.hltb_main_extra = meta.hltb_main_extra.filter(|&v| v > 0 && v < 100_000);
    meta.hltb_completionist = meta.hltb_completionist.filter(|&v| v > 0 && v < 100_000);

    // 4. genres 去重 + trim
    meta.genres = meta.genres
        .iter()
        .map(|g| g.trim().to_string())
        .filter(|g| !g.is_empty())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    // 5. save_paths trim
    meta.save_paths = meta.save_paths
        .iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();

    meta
}

/// 尝试将常见日期格式标准化为 YYYY-MM-DD
fn normalize_date(date: &str) -> Option<String> {
    // 已经是 YYYY-MM-DD 格式
    if date.len() == 10 && date.chars().nth(4) == Some('-') && date.chars().nth(7) == Some('-') {
        return Some(date.to_string());
    }

    // 尝试 "YYYY年M月D日" 格式
    if let Some(captures) = parse_chinese_date(date) {
        return Some(captures);
    }

    // 尝试 chrono 解析常见英文格式
    if let Ok(dt) = chrono::NaiveDate::parse_from_str(date, "%B %d, %Y") {
        return Some(dt.format("%Y-%m-%d").to_string());
    }
    if let Ok(dt) = chrono::NaiveDate::parse_from_str(date, "%b %d, %Y") {
        return Some(dt.format("%Y-%m-%d").to_string());
    }
    if let Ok(dt) = chrono::NaiveDate::parse_from_str(date, "%d/%m/%Y") {
        return Some(dt.format("%Y-%m-%d").to_string());
    }
    if let Ok(dt) = chrono::NaiveDate::parse_from_str(date, "%m/%d/%Y") {
        return Some(dt.format("%Y-%m-%d").to_string());
    }
    if let Ok(dt) = chrono::NaiveDate::parse_from_str(date, "%Y/%m/%d") {
        return Some(dt.format("%Y-%m-%d").to_string());
    }

    None
}

/// 解析 "2024年1月1日" 格式的日期
fn parse_chinese_date(date: &str) -> Option<String> {
    let date = date.trim();
    let year_end = date.find('年')?;
    let year: i32 = date[..year_end].parse().ok()?;

    let month_start = year_end + '年'.len_utf8();
    let month_end = date[month_start..].find('月')? + month_start;
    let month: u32 = date[month_start..month_end].parse().ok()?;

    let day_start = month_end + '月'.len_utf8();
    let day_end = date[day_start..].find('日')? + day_start;
    let day: u32 = date[day_start..day_end].parse().ok()?;

    if year > 0 && month >= 1 && month <= 12 && day >= 1 && day <= 31 {
        Some(format!("{:04}-{:02}-{:02}", year, month, day))
    } else {
        None
    }
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
    // 截取前 500 个字符（而非字节）附在错误信息中，避免多字节 UTF-8 截断 panic
    let preview: String = text.chars().take(500).collect();
    anyhow::bail!("无法解析 LLM 返回的游戏信息。原始响应:\n{}", preview)
}

/// 去掉模型返回的工具调用标签（如果模型以文本形式返回了工具调用）
fn strip_tool_call_tags(text: &str) -> Option<String> {
    if !text.contains("<tool_call>") {
        return None;
    }
    let mut result = String::new();
    let mut remaining = text;
    while let Some(start) = remaining.find("<tool_call>") {
        result.push_str(&remaining[..start]);
        if let Some(end) = remaining[start..].find("</tool_call>") {
            remaining = &remaining[start + end + 12..]; // 12 = len("</tool_call>")
        } else {
            tracing::warn!("发现 <tool_call> 标签但缺少 </tool_call> 闭合标签，截断剩余内容");
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

/// 构建 OpenAI 格式请求体
fn build_openai_request(
    config: &LlmConfig,
    system_prompt: &str,
    user_prompt: &str,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": config.model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_prompt}
        ],
        "max_completion_tokens": LLM_MAX_TOKENS,
        "temperature": 0.3,
        "stream": false
    });

    // OpenAI 协议工具定义（web_search）
    body["tools"] = serde_json::json!([
        {
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "搜索网络获取实时信息",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "搜索关键词"
                        }
                    },
                    "required": ["query"]
                }
            }
        }
    ]);

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
        "max_tokens": LLM_MAX_TOKENS,
        "system": system_prompt,
        "messages": [
            {"role": "user", "content": user_prompt}
        ]
    });

    // Anthropic 原生 web_search 工具（Brave Search 后端）
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
            let message = &resp["choices"][0]["message"];

            // 优先取 content 字段
            if let Some(content) = message["content"].as_str() {
                if !content.is_empty() {
                    return Ok(content.to_string());
                }
            }

            // 如果有 tool_calls 但没有 content，说明模型在调用工具但没返回文本
            if message.get("tool_calls").is_some() && !message["tool_calls"].is_null() {
                tracing::warn!("模型返回了 tool_calls 但没有 content: {}", message);
                anyhow::bail!("模型返回了工具调用但没有文本内容，请重试")
            }

            // 检查 finish_reason
            let finish_reason = resp["choices"][0]["finish_reason"]
                .as_str()
                .unwrap_or("unknown");
            if finish_reason == "length" {
                anyhow::bail!("LLM 响应被截断（token 不足），请重试");
            }

            tracing::error!("OpenAI 响应中找不到有效 content，完整响应: {}", resp);
            anyhow::bail!("OpenAI 响应中找不到 content 字段")
        }
        LlmProtocol::Anthropic => {
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

    #[test]
    fn test_normalize_chinese_date() {
        assert_eq!(normalize_date("2024年1月1日"), Some("2024-01-01".to_string()));
        assert_eq!(normalize_date("2017年2月24日"), Some("2017-02-24".to_string()));
        assert_eq!(normalize_date("2024-01-01"), Some("2024-01-01".to_string()));
    }

    #[test]
    fn test_deserialize_flexible_u32() {
        // 整数
        let json = r#"{"hltb_main_story": 1560}"#;
        let meta: LlmGameMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.hltb_main_story, Some(1560));

        // 浮点数
        let json = r#"{"hltb_main_story": 1560.0}"#;
        let meta: LlmGameMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.hltb_main_story, Some(1560));

        // 字符串数字
        let json = r#"{"hltb_main_story": "3000"}"#;
        let meta: LlmGameMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.hltb_main_story, Some(3000));

        // null
        let json = r#"{"hltb_main_story": null}"#;
        let meta: LlmGameMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.hltb_main_story, None);
    }

    #[test]
    fn test_sanitize_meta() {
        let meta = LlmGameMeta {
            name: Some("  测试游戏  ".to_string()),
            description: Some("  一个测试  ".to_string()),
            developer: Some("Dev".to_string()),
            publisher: Some("Pub".to_string()),
            release_date: Some("2024年3月15日".to_string()),
            genres: vec!["RPG".to_string(), "RPG".to_string(), "动作".to_string()],
            hltb_main_story: Some(1560),
            hltb_main_extra: Some(0),     // 异常：0 分钟
            hltb_completionist: Some(200_000), // 异常：超大值
            save_paths: vec!["  path1  ".to_string(), "".to_string()],
        };
        let meta = sanitize_meta(meta);
        assert_eq!(meta.name.unwrap(), "测试游戏");
        assert_eq!(meta.release_date.unwrap(), "2024-03-15");
        assert_eq!(meta.hltb_main_story, Some(1560));
        assert_eq!(meta.hltb_main_extra, None); // 0 被过滤
        assert_eq!(meta.hltb_completionist, None); // 超大值被过滤
        assert_eq!(meta.save_paths.len(), 1);
        // genres 去重后应该只有 2 个
        assert_eq!(meta.genres.len(), 2);
    }
}
