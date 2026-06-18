use std::fmt;

/// 应用统一错误类型
#[derive(Debug)]
pub enum AppError {
    /// 数据库错误
    Database(anyhow::Error),
    /// 网络请求错误
    Network(reqwest::Error),
    /// IO 错误
    Io(std::io::Error),
    /// JSON 序列化/反序列化错误
    Serde(serde_json::Error),
    /// 业务逻辑错误
    Business(String),
    /// 配置错误
    Config(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Database(e) => write!(f, "数据库错误: {}", e),
            AppError::Network(e) => write!(f, "网络错误: {}", e),
            AppError::Io(e) => write!(f, "IO 错误: {}", e),
            AppError::Serde(e) => write!(f, "序列化错误: {}", e),
            AppError::Business(msg) => write!(f, "{}", msg),
            AppError::Config(msg) => write!(f, "配置错误: {}", msg),
        }
    }
}

impl std::error::Error for AppError {}

// 从各种错误类型转换为 AppError
impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::Database(e)
    }
}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        AppError::Network(e)
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e)
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Serde(e)
    }
}

// Tauri 命令需要返回 String 错误
impl From<AppError> for String {
    fn from(e: AppError) -> Self {
        e.to_string()
    }
}

/// 便捷的 Result 类型别名
pub type AppResult<T> = Result<T, AppError>;

/// 为 anyhow::Result 添加扩展方法
pub trait ResultExt<T> {
    /// 转换为 AppError::Database
    fn into_db_error(self) -> AppResult<T>;
}

impl<T> ResultExt<T> for anyhow::Result<T> {
    fn into_db_error(self) -> AppResult<T> {
        self.map_err(AppError::Database)
    }
}

/// 创建业务错误的便捷函数
pub fn business_error(msg: impl Into<String>) -> AppError {
    AppError::Business(msg.into())
}

/// 创建配置错误的便捷函数
pub fn config_error(msg: impl Into<String>) -> AppError {
    AppError::Config(msg.into())
}
