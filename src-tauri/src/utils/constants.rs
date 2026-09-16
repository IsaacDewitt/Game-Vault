/// 全局常量 — 集中管理所有魔法值
/// 修改超时时间、阈值等只需改这里
// ==================== 封面文件 ====================
/// 封面图片最小有效文件大小（字节）
/// 低于此值视为损坏/占位文件
pub const COVER_MIN_FILE_SIZE: u64 = 100;

// ==================== 进程监控 ====================

/// 后台进程检测轮询间隔（秒）
///
/// 注意：该间隔**不影响时长精度**——会话起始时刻取"点击启动那一刻"，
/// 轮询间隔只决定"游戏退出后多久被察觉"，即结束检测滞后 0~10 秒（时长偏大而非偏小，
/// 且不跨会话累积）。对以分钟/小时计的游戏时长完全可忽略，故不做加密轮询。
pub const PROCESS_POLL_INTERVAL_SECS: u64 = 10;

/// 平台游戏「待命窗口」（秒）
///
/// 从 Game Vault 发起 `steam://` / `com.epicgames.launcher://` 启动请求后，
/// 游戏进程由客户端异步拉起（还要过 DRM 校验、云存档同步，慢加载数见不鲜），
/// 因此不能立即计时，而要在此期限内等待进程出现：
/// 命中即转正式会话（起始时刻回溯到点击那一刻），超时视为未启动
/// （可能正在更新 / 用户取消了 / 客户端未登录）。
pub const PLATFORM_ARM_WINDOW_SECS: i64 = 300;


// ==================== HTTP 超时 ====================

/// 封面下载 HTTP 超时（秒）
pub const COVER_FETCH_TIMEOUT_SECS: u64 = 30;

/// LLM 请求超时（秒）— 工具调用循环可能需要更长时间
pub const LLM_REQUEST_TIMEOUT_SECS: u64 = 120;

/// 网络搜索超时（秒）
pub const WEB_SEARCH_TIMEOUT_SECS: u64 = 15;

// ==================== LLM ====================

/// LLM 工具调用最大循环次数（防止无限循环）
pub const LLM_MAX_TOOL_ITERATIONS: u32 = 5;

/// LLM 最大输出 token 数（需要容纳完整游戏信息 + 存档路径）
pub const LLM_MAX_TOKENS: u32 = 2048;

// ==================== 网络搜索 ====================

/// SearXNG 公开实例列表（按优先级排序，依次尝试）
pub const SEARCH_ENGINE_INSTANCES: &[&str] = &[
    "https://searxng.site",
    "https://search.sapti.me",
    "https://searx.tiekoetter.com",
];

/// 搜索结果最大返回条数
pub const SEARCH_RESULT_LIMIT: usize = 5;

// ==================== 文件 I/O ====================

/// 读取 EXE 版本号时的最大读取字节数（1MB）
/// PE 头 + 节表 + 资源目录通常在前几百 KB 内
pub const EXE_VERSION_READ_LIMIT: u64 = 1024 * 1024;

// ==================== 游玩会话记录 ====================

/// 会话独立成条的最短时长（秒）
/// 低于此值不生成明细行、不计入 play_count，仅把时长累加到总时长与日汇总
pub const SESSION_MIN_DURATION_SECS: u64 = 600;

/// 会话合并窗口（秒）
/// 短会话距「同一游戏上一条会话结束」不超过此值时，并入上一条而非丢弃
pub const SESSION_MERGE_WINDOW_SECS: i64 = 300;

/// 明细保留期（天）：超过此天数的 play_sessions 行会被清理
/// 统计已改由 play_stats_daily / play_stats_hourly 供数，删明细不影响历史统计
pub const SESSION_RETENTION_DAYS: i64 = 365;

/// 启动后延迟执行明细清理的秒数（避开启动高峰，避免拖慢首屏）
pub const SESSION_CLEANUP_DELAY_SECS: u64 = 30;

// ==================== 启动守卫（WebView 黑屏自愈） ====================

/// 前端「报到」超时（秒）。
///
/// 窗口创建后，正常启动会在数百毫秒内完成前端挂载并报到（含 cold start）。
/// 超过此值仍无报到，判定 `WebView` 创建失败（0x8007139F 一类静默失败），
/// 触发自动重启。放得足够宽（正常值的数十倍）以免误杀慢盘/开机高峰。
pub const FRONTEND_READY_TIMEOUT_SECS: u64 = 20;

/// 连续自动重启的最大次数。超过则不再重启（避免无限重启循环），
/// 改为弹窗告知用户手动处理。计数在「前端成功报到」时清零。
pub const MAX_BOOT_RESTART_ATTEMPTS: u32 = 2;

/// 看门狗轮询间隔（毫秒）。采用轮询而非一次性 sleep，
/// 以便前端报到后立即退出线程，不空占资源。
pub const BOOT_WATCHDOG_POLL_MS: u64 = 250;
