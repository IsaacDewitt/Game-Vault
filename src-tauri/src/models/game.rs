use serde::{Deserialize, Serialize};

/// 游戏数据模型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Game {
    pub id: String,
    pub name: String,
    pub install_path: Option<String>,
    pub exe_path: Option<String>,
    pub exe_name: Option<String>,
    pub cover_local: Option<String>,
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
    pub added_at: String,
    pub updated_at: Option<String>,
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
            added_at: chrono::Utc::now().to_rfc3339(),
            updated_at: None,
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
    pub sort_by: String,       // "name", "last_played", "play_time", "added_at"
    pub sort_order: String,    // "asc" or "desc"
}

impl Default for GameFilter {
    fn default() -> Self {
        Self {
            search: None,
            favorites_only: false,
            sort_by: "last_played".to_string(),
            sort_order: "desc".to_string(),
        }
    }
}
