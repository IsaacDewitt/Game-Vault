use serde::{Deserialize, Serialize};

/// 游戏会话详情（联表查询，含游戏名）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaySessionDetail {
    pub id: i64,
    pub game_id: String,
    pub game_name: String,
    pub start_time: String,
    pub end_time: Option<String>,
    pub duration_seconds: u64,
}

/// 活跃的游戏会话（内存中；game_id 即 HashMap 的 key，不重复存字段）
#[derive(Debug, Clone)]
pub struct ActiveSession {
    pub exe_name: String,
    /// 游戏可执行文件的完整路径，用于精确匹配进程
    pub exe_path: Option<String>,
    /// 启动时得到的进程 PID，用于进程树追踪
    pub spawned_pid: Option<u32>,
    /// 游戏安装目录，用于回退检测（进程不在进程树中但仍在同目录运行）
    pub install_path: Option<String>,
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
///
/// `is_removed`：该条目的历史来自 play_stats_daily 中已从库内移除的游戏
/// （删除游戏不删记录，条目仍在榜上，仅标记为已移除）。库内活条目恒为 false。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GamePlayStats {
    pub game_id: String,
    pub game_name: String,
    pub total_seconds: u64,
    pub play_count: u32,
    pub last_played: Option<String>,
    #[serde(default)]
    pub is_removed: bool,
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

/// 游戏状态统计（三态：未游玩 / 已游玩 / 已通关，2026-09-06 起与前端筛选口径对齐，
/// 不再含 playing / abandoned 桶 —— 收藏是独立维度不参与状态互斥）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusStats {
    pub unplayed: u32,
    pub played: u32,
    pub completed: u32,
}
