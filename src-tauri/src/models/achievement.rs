use serde::{Deserialize, Serialize};

/// 成就定义（展平后的独立成就记录，多级成就拆分为多条）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AchievementDef {
    /// 展平后的唯一 ID，如 "g02_2"（藏书家二级）
    pub id: String,
    /// 基础成就 ID，用于前端分组，如 "G-02"
    pub base_id: String,
    /// 成就名称，如 "藏书家"
    pub name: String,
    /// 作用域: "global"（全局） | "pergame"（单游戏，每个游戏独立结算）
    pub scope: String,
    /// 分类: "progress" | "collect" | "fun" | "challenge"
    pub category: String,
    /// 解锁条件描述
    pub desc: String,
    /// 图标（emoji）
    pub icon: String,
    /// 当前等级（从 1 开始）
    pub tier: u32,
    /// 总等级数（1 表示非多级成就）
    pub tier_total: u32,
    /// 目标值（秒/次数/个数等），用于进度条展示
    pub target: u64,
}

/// 解锁记录（对应数据库 achievement_unlocks 表）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AchievementUnlock {
    pub achievement_id: String,
    /// 全局成就为 ""，单游戏成就为游戏 ID
    pub game_id: String,
    pub unlocked_at: String,
}

/// 全局成就状态（含进度，供前端展示）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalAchievementStatus {
    pub def: AchievementDef,
    pub unlocked: bool,
    pub unlocked_at: Option<String>,
    /// 当前进度值
    pub progress: u64,
    /// 目标值
    pub target: u64,
}

/// 单游戏成就状态（某个游戏下的某条成就）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameAchievementStatus {
    pub def: AchievementDef,
    pub unlocked: bool,
    pub unlocked_at: Option<String>,
    pub progress: u64,
    pub target: u64,
}

/// 某个游戏的全部单游戏成就
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameAchievements {
    pub game_id: String,
    pub game_name: String,
    pub achievements: Vec<GameAchievementStatus>,
}

/// 成就系统汇总（前端成就页一次性拉取）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AchievementSummary {
    pub global: Vec<GlobalAchievementStatus>,
    pub per_game: Vec<GameAchievements>,
    pub total_count: u32,
    pub unlocked_count: u32,
}

/// 新解锁事件（触发通知用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnlockEvent {
    pub def: AchievementDef,
    /// 全局成就为 None
    pub game_id: Option<String>,
}

/// 全局成就检测所需的聚合统计（内部使用，不直接序列化给前端）
#[derive(Debug, Clone, Default)]
pub struct AchievementGlobalStats {
    pub game_count: u64,
    pub total_play_time: u64,
    pub total_sessions: u64,
    pub completed_count: u64,
    pub favorite_count: u64,
    pub cover_count: u64,
    pub save_paths_count: u64,
    pub llm_filled_count: u64,
    pub max_session_duration: u64,
    pub longest_streak: u64,
    pub max_distinct_games_per_day: u64,
    pub night_session: bool,
    pub dawn_session: bool,
    pub weekend_total: u64,
    pub hltb_completed_count: u64,
    pub abandoned_replayed: u64,
    // ===== 第二批成就新增统计 =====
    /// 从未启动的游戏数（play_time_seconds = 0）
    pub unplayed_count: u64,
    /// 玩过但不足 1 小时的游戏数
    pub played_under_1h_count: u64,
    /// 入库超 1 年才首次启动的游戏数
    pub late_bloomer_count: u64,
    /// 超 HLTB 主线时长却仍标「未通关」的游戏数
    pub over_main_not_completed_count: u64,
    /// 发行于 20 年前的老游戏数
    pub old_game_count: u64,
    /// 去重后的游戏类型数
    pub distinct_genre_count: u64,
    /// 同一开发商拥有的最多游戏数
    pub max_dev_count: u64,
    /// 游戏 exe 文件总大小（GiB）
    pub total_exe_size_gb: u64,
    /// 全库单日最长游玩（秒）
    pub max_day_seconds: u64,
    /// 累计 100 小时以上的游戏数
    pub games_over_100h_count: u64,
    /// 单个周末（六日合计）最长游玩（秒）
    pub max_weekend_total: u64,
    /// 单周内玩过的最多不同游戏数
    pub max_distinct_games_per_week: u64,
    /// 连续凌晨（0–5 点）游玩天数
    pub night_streak: u64,
    /// 是否存在同一天既凌晨又白天游玩
    pub day_night_same_day: bool,
    /// 是否存在一周 7 天满勤
    pub has_full_week: bool,
    /// 是否存在某游戏间隔 ≥ 180 天后重玩
    pub long_gap_replay: bool,
    /// 库龄（首款游戏 added_at 至今的天数）
    pub days_since_first_add: u64,
    /// 距最后一次游玩的天数
    pub days_since_last_play: u64,
}

/// 单个游戏的成就检测统计（内部使用）
#[derive(Debug, Clone, Default)]
pub struct PerGameStats {
    pub game_id: String,
    pub game_name: String,
    pub status: String,
    pub play_time_seconds: u64,
    pub play_count: u64,
    pub sessions_count: u64,
    /// HLTB 主线时长（分钟）
    pub hltb_main_story: Option<u64>,
    /// HLTB 完美通关时长（分钟）
    pub hltb_completionist: Option<u64>,
    pub max_session_duration: u64,
    pub max_day_seconds: u64,
    pub distinct_days: u64,
    pub longest_streak: u64,
    pub night_session: bool,
    /// 弃坑后重新游玩（last_played > abandoned_at，abandoned_at 为最近一次弃坑时间）
    pub replayed_after_abandon: bool,
}
