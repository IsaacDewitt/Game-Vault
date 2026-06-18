use std::path::{Path, PathBuf};

/// 获取应用数据目录
pub fn get_app_data_dir() -> PathBuf {
    let mut path = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push("GameVault");
    path
}

/// 获取应用配置目录
pub fn get_app_config_dir() -> PathBuf {
    let mut path = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push("GameVault");
    path
}

/// 获取数据库路径
pub fn get_database_path() -> PathBuf {
    get_app_data_dir().join("gamevault.db")
}

/// 获取封面缓存目录
pub fn get_covers_dir() -> PathBuf {
    get_app_data_dir().join("covers")
}

/// 确保目录存在
pub fn ensure_dir_exists(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}
