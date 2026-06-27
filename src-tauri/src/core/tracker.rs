use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use sysinfo::{Pid, System};
use crate::models::*;
use crate::core::Database;  // 用于 persist_finished_sessions 的参数类型

/// 已结束的游戏会话数据，用于外部持久化
#[derive(Debug, Clone)]
pub struct FinishedSession {
    pub game_id: String,
    pub start_time: String,  // RFC 3339
    pub duration_seconds: u64,
}

/// 游戏时长追踪器
pub struct PlayTimeTracker {
    active_sessions: HashMap<String, ActiveSession>,
    /// 复用 sysinfo System 实例，避免每10秒全量扫描
    sys: System,
}

impl PlayTimeTracker {
    pub fn new() -> Self {
        Self {
            active_sessions: HashMap::new(),
            sys: System::new_all(),
        }
    }

    /// 开始追踪游戏
    /// 如果已有同 ID 的活跃会话，先结束旧会话并返回其数据供外部持久化
    pub fn start_tracking(
        &mut self,
        game_id: &str,
        exe_name: &str,
        exe_path: Option<&str>,
        spawned_pid: Option<u32>,
        install_path: Option<&str>,
    ) -> Option<FinishedSession> {
        let finished = if self.active_sessions.contains_key(game_id) {
            self.stop_tracking_internal(game_id)
        } else {
            None
        };

        self.active_sessions.insert(
            game_id.to_string(),
            ActiveSession {
                game_id: game_id.to_string(),
                exe_name: exe_name.to_string(),
                exe_path: exe_path.map(|s| s.to_string()),
                spawned_pid,
                install_path: install_path.map(|s| s.to_string()),
                start_time: chrono::Utc::now(),
            },
        );

        tracing::info!(
            "开始追踪游戏: {} (exe: {}, path: {:?}, pid: {:?}, install: {:?})",
            game_id, exe_name, exe_path, spawned_pid, install_path
        );
        finished
    }

    /// 停止追踪游戏，返回结束的会话数据
    pub fn stop_tracking(&mut self, game_id: &str) -> Option<FinishedSession> {
        self.stop_tracking_internal(game_id)
    }

    /// 内部停止追踪，仅从 HashMap 中移除并返回数据，不获取 DB 锁
    fn stop_tracking_internal(&mut self, game_id: &str) -> Option<FinishedSession> {
        if let Some(session) = self.active_sessions.remove(game_id) {
            let duration_secs = chrono::Utc::now()
                .signed_duration_since(session.start_time)
                .num_seconds();

            // 防御：系统时钟回退时 duration 可能为负值，
            // 负值 as u64 会溢出为巨大正数，导致 play_time_seconds 清零
            if duration_secs > 0 {
                let duration = duration_secs as u64;
                tracing::info!("游戏 {} 结束，时长: {}秒", game_id, duration);
                return Some(FinishedSession {
                    game_id: game_id.to_string(),
                    start_time: session.start_time.to_rfc3339(),
                    duration_seconds: duration,
                });
            }
        }
        None
    }

    /// 构建进程树：parent → children 映射
    /// 返回 (parent_to_children, pid_to_exe_path)
    fn build_process_tree(sys: &System) -> (HashMap<Pid, Vec<Pid>>, HashMap<Pid, String>) {
        let mut parent_to_children: HashMap<Pid, Vec<Pid>> = HashMap::new();
        let mut pid_to_exe: HashMap<Pid, String> = HashMap::new();

        for (pid, process) in sys.processes() {
            if let Some(exe_path) = process.exe() {
                pid_to_exe.insert(
                    *pid,
                    exe_path.to_string_lossy().to_lowercase(),
                );
            }
            if let Some(parent_pid) = process.parent() {
                parent_to_children
                    .entry(parent_pid)
                    .or_default()
                    .push(*pid);
            }
        }

        (parent_to_children, pid_to_exe)
    }

    /// 递归收集指定 PID 的所有子孙进程
    fn collect_descendants(root_pid: u32, parent_to_children: &HashMap<Pid, Vec<Pid>>) -> HashSet<Pid> {
        let root = Pid::from(root_pid as usize);
        let mut result = HashSet::new();
        let mut stack = vec![root];
        while let Some(pid) = stack.pop() {
            if let Some(children) = parent_to_children.get(&pid) {
                for &child in children {
                    if result.insert(child) {
                        stack.push(child);
                    }
                }
            }
        }
        result
    }

    /// 检查活跃会话（定期调用）
    /// 返回已结束的会话数据，由调用方负责持久化到数据库
    pub fn check_active_sessions(&mut self) -> Vec<FinishedSession> {
        let mut finished_sessions = Vec::new();

        // 增量刷新进程列表，而非全量重建
        self.sys.refresh_processes();

        // 构建进程树供所有 session 复用
        let (parent_to_children, pid_to_exe) = Self::build_process_tree(&self.sys);

        // 收集需要检查的会话信息，避免借用冲突
        let sessions_to_check: Vec<(
            String,
            String,
            Option<String>,
            Option<u32>,
            Option<String>,
        )> = self
            .active_sessions
            .iter()
            .map(|(id, session)| {
                (
                    id.clone(),
                    session.exe_name.clone(),
                    session.exe_path.clone(),
                    session.spawned_pid,
                    session.install_path.clone(),
                )
            })
            .collect();

        for (game_id, exe_name, exe_path, spawned_pid, install_path) in sessions_to_check {
            let mut still_running = false;

            // ============================================
            // 策略 1: 进程树检测 (最可靠)
            // ============================================
            if let Some(pid) = spawned_pid {
                let root_pid = Pid::from(pid as usize);

                // 检查原始 PID 是否还活着（用 sys.processes() 而非 pid_to_exe，
                // 因为 pid_to_exe 依赖 GetModuleFileNameExW，对 32-bit 老游戏可能失败）
                let root_alive = self.sys.processes().contains_key(&root_pid);

                // 收集所有子孙进程
                let descendants = Self::collect_descendants(pid, &parent_to_children);

                // 检查是否有子孙进程还在运行（同上，用 sys.processes() ）
                let descendants_alive = descendants.iter().any(|d| self.sys.processes().contains_key(d));

                if root_alive || descendants_alive {
                    still_running = true;
                    if !root_alive {
                        let alive_count = descendants.iter()
                            .filter(|d| self.sys.processes().contains_key(d))
                            .count();
                        tracing::info!(
                            "游戏 {} 原始进程 PID {} 已退出，但检测到 {} 个子孙进程仍在运行",
                            game_id, pid, alive_count
                        );
                    }
                }
            }

            // ============================================
            // 策略 2: 安装目录检测 (回退)
            // ============================================
            if !still_running {
                if let Some(ref install) = install_path {
                    // 仅当 install_path 足够具体时才启用此策略
                    if install.len() >= 4 {
                        let install_lower = install.to_lowercase();

                        // 两层检查：先查 pid_to_exe（快速，exe 路径缓存），
                        // 再直接遍历 sys.processes()（覆盖 exe() 失败的 32-bit 老游戏）
                        let found_in_install = pid_to_exe.values().any(|exe| {
                            exe.starts_with(&install_lower)
                        }) || self.sys.processes().values().any(|p| {
                            p.exe().map_or(false, |exe| {
                                exe.to_string_lossy().to_lowercase()
                                    .starts_with(&install_lower)
                            })
                        });

                        if found_in_install {
                            still_running = true;
                            tracing::info!(
                                "游戏 {} 通过安装目录检测到进程仍然活跃: {}",
                                game_id, install
                            );
                        }
                    }
                }
            }

            // ============================================
            // 策略 3: exe 文件名/路径匹配 (兼容旧数据)
            // ============================================
            if !still_running {
                let exe_lower = exe_name.to_lowercase();

                if let Some(ref expected_path) = exe_path {
                    let expected_lower = expected_path.to_lowercase();
                    still_running = self.sys.processes().values().any(|p| {
                        p.exe().map_or(false, |exe| {
                            exe.to_string_lossy().to_lowercase() == expected_lower
                        })
                    });
                } else {
                    still_running = self.sys.processes().values().any(|p| {
                        p.name().to_lowercase() == exe_lower
                    });
                }
            }

            if !still_running {
                let descendant_count = spawned_pid
                    .map(|pid| Self::collect_descendants(pid, &parent_to_children).len())
                    .unwrap_or(0);
                tracing::info!(
                    "游戏 {} 已退出 (spawned_pid: {:?}, descendants_in_tree: {}, \
                     install_path: {:?}, strategies_exhausted: all)",
                    game_id, spawned_pid, descendant_count, install_path
                );
                if let Some(session) = self.stop_tracking_internal(&game_id) {
                    finished_sessions.push(session);
                }
            }
        }

        finished_sessions
    }

    /// 获取当前活跃的游戏
    pub fn get_active_games(&self) -> Vec<String> {
        self.active_sessions.keys().cloned().collect()
    }

    /// 检查某个游戏是否正在运行
    pub fn is_game_running(&self, game_id: &str) -> bool {
        self.active_sessions.contains_key(game_id)
    }

    /// 持久化已结束的会话到数据库（在 Tracker 锁释放后调用）
    /// 一次性获取锁并批量插入，避免逐条获取/释放锁的开销
    pub fn persist_finished_sessions(
        db: &Arc<Mutex<Database>>,
        sessions: &[FinishedSession],
    ) {
        if sessions.is_empty() {
            return;
        }
        let db = db.lock().unwrap_or_else(|e| e.into_inner());
        for session in sessions {
            match db.add_play_session(
                &session.game_id,
                &session.start_time,
                session.duration_seconds,
            ) {
                Ok(_) => {
                    tracing::info!("已保存游戏会话: {}", session.game_id);
                }
                Err(e) => {
                    tracing::error!("保存游戏会话失败: {}", e);
                }
            }
        }
    }
}
