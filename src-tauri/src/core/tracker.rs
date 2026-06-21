use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use sysinfo::System;
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
    pub fn start_tracking(&mut self, game_id: &str, exe_name: &str, exe_path: Option<&str>) -> Option<FinishedSession> {
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
                start_time: chrono::Utc::now(),
            },
        );

        tracing::info!("开始追踪游戏: {} (exe: {}, path: {:?})", game_id, exe_name, exe_path);
        finished
    }

    /// 停止追踪游戏，返回结束的会话数据
    pub fn stop_tracking(&mut self, game_id: &str) -> Option<FinishedSession> {
        self.stop_tracking_internal(game_id)
    }

    /// 内部停止追踪，仅从 HashMap 中移除并返回数据，不获取 DB 锁
    fn stop_tracking_internal(&mut self, game_id: &str) -> Option<FinishedSession> {
        if let Some(session) = self.active_sessions.remove(game_id) {
            let duration = chrono::Utc::now()
                .signed_duration_since(session.start_time)
                .num_seconds() as u64;

            if duration > 0 {
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

    /// 检查活跃会话（定期调用）
    /// 返回已结束的会话数据，由调用方负责持久化到数据库
    pub fn check_active_sessions(&mut self) -> Vec<FinishedSession> {
        let mut finished_sessions = Vec::new();

        // 增量刷新进程列表，而非全量重建
        self.sys.refresh_processes();

        // 收集需要检查的会话信息，避免借用冲突
        let sessions_to_check: Vec<(String, String, Option<String>)> = self.active_sessions
            .iter()
            .map(|(id, session)| (id.clone(), session.exe_name.clone(), session.exe_path.clone()))
            .collect();

        for (game_id, exe_name, exe_path) in sessions_to_check {
            let exe_lower = exe_name.to_lowercase();

            // 优先用完整路径匹配，回退到文件名匹配
            let still_running = if let Some(ref expected_path) = exe_path {
                let expected_lower = expected_path.to_lowercase();
                self.sys.processes().values().any(|p| {
                    p.exe().map_or(false, |exe| {
                        exe.to_string_lossy().to_lowercase() == expected_lower
                    })
                })
            } else {
                self.sys.processes().values().any(|p| {
                    p.name().to_lowercase() == exe_lower
                })
            };

            if !still_running {
                tracing::info!("游戏 {} 已退出", game_id);
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
