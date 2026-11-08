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

/// 会话状态快照（供前端在挂载 / 重连时对齐后端真相源）
///
/// 三态与前端 `LaunchState` 一一对应：
/// - `playing`：正式计时中；
/// - `launching`：已委托客户端、等进程现身（尚未计时）；
/// - `updating`：客户端明确报告正在更新（Steam `Updating=1`）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub game_id: String,
    pub state: String,
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
    /// 单 PID 点查被证明不可信时的「全表确认冷却」截止时刻。
    ///
    /// 背景：计时期改走单 PID 点查（`refresh_process_specifics`，约 0.4 微秒）后，
    /// 极少数受保护进程（内核级反作弊保护的游戏）句柄打不开，sysinfo 会恒返回
    /// `false`——**假死**。此时只有全表三级判定才是权威口径。
    /// 一旦「点查说死、全表说活」，就说明该会话的点查不可信，在冷却期内不再理会
    /// 点查结果，避免在这种会话上每拍都做一次全表刷新（退化成旧行为）。
    pub poll_cooldown_until: Option<std::time::Instant>,
    /// 挂死观察器（P1，2026-10-20）：进程树还在但 CPU 时间不再增长时兜底结算
    pub hang: HangWatch,
    /// 诊断日志去重键：同一状态只打一次日志，避免「全表说活」的会话每轮刷屏
    pub last_note: Option<String>,
}

/// 挂死观察器状态（见 `constants::HANG_CPU_IDLE_SECS`）
///
/// 只观察**进程树是否还有 CPU 活动**，不看占用率高低：
/// - 挂死/假活的进程树，采样窗内毫无活动（占用恒 0）；
/// - 正常游玩的进程树（哪怕挂在暂停菜单、最小化在后台）必然有活动。
///
/// 采样读不到 CPU（受保护进程）时**不据此判死**——读不到不等于没在跑，这一侧必须保守。
#[derive(Debug, Clone, Default)]
pub struct HangWatch {
    /// 下一次采样时刻（None = 本轮就该采）
    pub next_sample_at: Option<std::time::Instant>,
    /// 上一次采样到的进程树 CPU 占用合计（sysinfo 口径，仅用于日志留证）
    pub last_usage: f32,
    /// 合计值开始「毫无活动」的时刻（None = 未进入静默观察）
    pub idle_since: Option<std::time::Instant>,
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

// 【2026-11-08】移除 `StatusStats`：状态分布统计已下线，三态口径只保留在
// 前端 `src/lib/gameFilter.ts`；这里不再维护第二份结构，避免两边各自漂移。
