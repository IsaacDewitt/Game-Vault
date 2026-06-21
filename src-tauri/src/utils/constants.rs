/// 全局常量 — 集中管理所有魔法值
/// 修改超时时间、阈值等只需改这里

// ==================== 封面文件 ====================

/// 封面图片最小有效文件大小（字节）
/// 低于此值视为损坏/占位文件
pub const COVER_MIN_FILE_SIZE: u64 = 100;

// ==================== 进程监控 ====================

/// 后台进程检测轮询间隔（秒）
pub const PROCESS_POLL_INTERVAL_SECS: u64 = 10;

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

/// LLM 最大输出 token 数
pub const LLM_MAX_TOKENS: u32 = 1024;

// ==================== 文件 I/O ====================

/// 读取 EXE 版本号时的最大读取字节数（1MB）
/// PE 头 + 节表 + 资源目录通常在前几百 KB 内
pub const EXE_VERSION_READ_LIMIT: u64 = 1024 * 1024;
