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
