use serde::{Deserialize, Serialize};

/// 游戏数据模型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Game {
    pub id: String,
    pub name: String,
    pub install_path: Option<String>,
    pub exe_path: Option<String>,
    pub exe_name: Option<String>,
    /// 从 exe 文件读取的版本号
    pub exe_version: Option<String>,
    pub cover_local: Option<String>,
    /// 实际存储的是本地封面缓存文件路径（如 covers/{uuid}.jpg），而非远程 URL
    pub cover_url: Option<String>,
    pub description: Option<String>,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub release_date: Option<String>,
    pub genres: Vec<String>,
    pub play_time_seconds: u64,
    pub last_played: Option<String>,
    pub play_count: u32,
    pub is_favorite: bool,
    /// 游戏状态: "unplayed", "playing", "completed", "abandoned"
    pub status: String,
    pub added_at: String,
    pub updated_at: Option<String>,
    /// HLTB 主线时长（分钟）
    pub hltb_main_story: Option<u32>,
    /// HLTB 主线+支线时长（分钟）
    pub hltb_main_extra: Option<u32>,
    /// HLTB 完美通关时长（分钟）
    pub hltb_completionist: Option<u32>,
    /// 游戏存档路径列表
    #[serde(default)]
    pub save_paths: Vec<String>,
}

impl Game {
    /// 创建新的游戏实例
    pub fn new(name: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            install_path: None,
            exe_path: None,
            exe_name: None,
            exe_version: None,
            cover_local: None,
            cover_url: None,
            description: None,
            developer: None,
            publisher: None,
            release_date: None,
            genres: Vec::new(),
            play_time_seconds: 0,
            last_played: None,
            play_count: 0,
            is_favorite: false,
            status: "unplayed".to_string(),
            added_at: chrono::Utc::now().to_rfc3339(),
            updated_at: None,
            hltb_main_story: None,
            hltb_main_extra: None,
            hltb_completionist: None,
            save_paths: Vec::new(),
        }
    }

    /// 格式化游戏时长
    pub fn formatted_play_time(&self) -> String {
        let hours = self.play_time_seconds / 3600;
        let minutes = (self.play_time_seconds % 3600) / 60;
        if hours > 0 {
            format!("{}h {}m", hours, minutes)
        } else {
            format!("{}m", minutes)
        }
    }
}

/// 游戏筛选条件
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GameFilter {
    pub search: Option<String>,
    pub favorites_only: bool,
    /// 按状态筛选: "unplayed", "playing", "completed", "abandoned"
    pub status: Option<String>,
    /// 按类型标签筛选（模糊匹配 genres JSON 字段）
    pub genre: Option<String>,
    pub sort_by: String,       // "name", "last_played", "play_time", "added_at"
    pub sort_order: String,    // "asc" or "desc"
}

impl Default for GameFilter {
    fn default() -> Self {
        Self {
            search: None,
            favorites_only: false,
            status: None,
            genre: None,
            sort_by: "last_played".to_string(),
            sort_order: "desc".to_string(),
        }
    }
}
