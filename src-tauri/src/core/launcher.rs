use anyhow::Result;
use std::path::Path;
use crate::models::*;

/// 游戏启动器
pub struct GameLauncher;

impl GameLauncher {
    /// 启动游戏，返回进程 PID 用于后续进程树追踪
    pub fn launch(game: &Game) -> Result<u32> {
        let exe_path = game.exe_path.as_ref()
            .ok_or_else(|| anyhow::anyhow!("游戏没有可执行文件路径"))?;

        if !Path::new(exe_path).exists() {
            anyhow::bail!("游戏可执行文件不存在: {}", exe_path);
        }

        let mut cmd = std::process::Command::new(exe_path);

        if let Some(ref install_path) = game.install_path {
            cmd.current_dir(install_path);
        }

        let child = cmd.spawn()?;
        let pid = child.id();

        tracing::info!("启动游戏: {} (PID: {})", game.name, pid);
        Ok(pid)
    }
}
