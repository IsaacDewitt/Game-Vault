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
    /// 状态字段（2026-10-28 起停用）：手账只记玩过的游戏，四态分类已从界面移除。
    ///
    /// 列与字段保留是为了不多做一次 DB 迁移，新条目一律写 "completed"，
    /// 前端不再读写。日后若要恢复分类，只需在 UI 侧重新接线即可。
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
            // 手账记的都是玩过的游戏，故新条目直接记为已通关（不再有想玩/游玩中/弃坑）
            status: "completed".to_string(),
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
    /// 按类型标签筛选（模糊匹配 genres JSON 字段）
    pub genre: Option<String>,
    pub sort_by: String,       // "name", "rating", "added_at", "updated_at"
    pub sort_order: String,    // "asc" or "desc"
}

/// 游玩时长候选行（手账「我的游玩时长」按名匹配游戏库用）。
///
/// 两路来源与 `get_play_stats` 同口径：在库游戏取 `games` 卡片累计；
/// 已删游戏取 `play_stats_daily` 孤儿历史（墓碑还原名字）。
/// 墓碑也存 `name_en`（2026-10-09 起），故已删条目同样能按英文名被认领。
#[derive(Debug, Clone)]
pub struct PlayTimeCandidate {
    pub name: String,
    pub name_en: Option<String>,
    pub play_time_seconds: u64,
    pub last_played: Option<String>,
}

/// 手账详情「我的游玩时长」展示数据（按名匹配游戏库得来，含已删游戏历史）。
/// 只在有游玩历史时返回——纯库内条目、零时长的不进结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewPlayTime {
    pub play_time_seconds: u64,
    pub last_played: Option<String>,
}

impl Default for ReviewFilter {
    fn default() -> Self {
        Self {
            search: None,
            genre: None,
            sort_by: "added_at".to_string(),
            sort_order: "desc".to_string(),
        }
    }
}
