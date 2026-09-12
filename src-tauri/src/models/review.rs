use serde::{Deserialize, Serialize};

/// 手账条目数据模型（与游戏库完全隔离的独立模块）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Review {
    pub id: String,
    pub name: String,
    /// 官方英文名称（LLM 拉取或手动填写；用于 SteamGridDB 检索，展示时优先于 name）
    pub name_en: Option<String>,
    /// 本地封面缓存文件路径（如 covers/{uuid}.jpg）
    pub cover_local: Option<String>,
    pub cover_url: Option<String>,
    pub description: Option<String>,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub release_date: Option<String>,
    pub genres: Vec<String>,
    /// HLTB 主线时长（分钟）
    pub hltb_main_story: Option<u32>,
    /// HLTB 主线+支线时长（分钟）
    pub hltb_main_extra: Option<u32>,
    /// HLTB 完美通关时长（分钟）
    pub hltb_completionist: Option<u32>,
    /// 我的评分（0-10 分制，null 表示未评分）
    pub rating: Option<u32>,
    /// 我的评价
    pub review: Option<String>,
    /// 状态: "wishlist"(想玩) | "playing"(游玩中) | "completed"(已通关) | "abandoned"(已弃坑)
    pub status: String,
    pub added_at: String,
    pub updated_at: Option<String>,
    /// 截图库定位：截图根目录下的**子目录名**（如 "MafiaTheOldCountry"），None/空 = 自动推断。
    ///
    /// 手账是独立库、不存 exe，早先靠「同名游戏」从游戏库借 exe_name 定位截图目录 ——
    /// 游戏一旦从游戏库删除（只留墓碑），手账的截图库就跟着失效（2026-09-12 修）。
    /// 现在手账自己持有该字段，解析优先级：
    /// `screenshot_dir` > 同名活条目 exe > 同名墓碑 exe。
    pub screenshot_dir: Option<String>,
}

impl Review {
    /// 创建新的手账条目（初始只有名字，元数据/封面后续由 LLM/SteamGridDB 填充）
    pub fn new(name: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            name_en: None,
            cover_local: None,
            cover_url: None,
            description: None,
            developer: None,
            publisher: None,
            release_date: None,
            genres: Vec::new(),
            hltb_main_story: None,
            hltb_main_extra: None,
            hltb_completionist: None,
            rating: None,
            review: None,
            status: "wishlist".to_string(),
            added_at: chrono::Utc::now().to_rfc3339(),
            updated_at: None,
            screenshot_dir: None,
        }
    }
}

/// 手账条目筛选条件
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewFilter {
    pub search: Option<String>,
    /// 按状态筛选: "wishlist", "playing", "completed", "abandoned"
    pub status: Option<String>,
    /// 按类型标签筛选（模糊匹配 genres JSON 字段）
    pub genre: Option<String>,
    pub sort_by: String,       // "name", "rating", "added_at", "updated_at"
    pub sort_order: String,    // "asc" or "desc"
}

impl Default for ReviewFilter {
    fn default() -> Self {
        Self {
            search: None,
            status: None,
            genre: None,
            sort_by: "added_at".to_string(),
            sort_order: "desc".to_string(),
        }
    }
}
