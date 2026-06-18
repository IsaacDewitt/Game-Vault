use serde::{Deserialize, Serialize};

/// 游戏会话记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaySession {
    pub id: i64,
    pub game_id: String,
    pub start_time: String,
    pub end_time: Option<String>,
    pub duration_seconds: u64,
}

/// 活跃的游戏会话（内存中）
#[derive(Debug, Clone)]
pub struct ActiveSession {
    pub game_id: String,
    pub exe_name: String,
    /// 游戏可执行文件的完整路径，用于精确匹配进程
    pub exe_path: Option<String>,
    pub start_time: chrono::DateTime<chrono::Utc>,
}

/// 每日游玩统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyStats {
    pub date: String,
    pub total_seconds: u64,
    pub sessions_count: u32,
}

/// 游戏时长排行榜
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GamePlayStats {
    pub game_id: String,
    pub game_name: String,
    pub total_seconds: u64,
    pub play_count: u32,
    pub last_played: Option<String>,
}

/// 游戏类型统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenreStats {
    pub genre: String,
    pub total_seconds: u64,
    pub game_count: u32,
}

/// 热力图日期数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeatmapDay {
    pub date: String,
    pub total_seconds: u64,
}

/// 游玩时段统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HourlyStats {
    pub hour: u32,
    pub weekday: u32,
    pub total_seconds: u64,
}

/// 游戏状态统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusStats {
    pub unplayed: u32,
    pub playing: u32,
    pub completed: u32,
    pub abandoned: u32,
}
