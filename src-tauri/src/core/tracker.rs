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

/// 待命会话：已发起启动请求，但尚未在进程列表中发现游戏进程
///
/// 平台游戏（Steam/Epic）的启动是"委托客户端"完成的，**拿不到 PID**，
/// 因此在进程现身之前不能确认游戏真的跑起来了——先待命，命中才转正。
/// 这样可避免"点了启动但游戏没起来（正在更新/取消/未登录）却记了一笔假时长"。
#[derive(Debug, Clone)]
struct PendingSession {
    exe_name: String,
    exe_path: Option<String>,
    install_path: Option<String>,
    /// 点击启动的时刻：会话转正后作为起始时刻（加载期计入游玩）
    armed_at: chrono::DateTime<chrono::Utc>,
    /// 待命截止时刻，超时视为未启动
    deadline: chrono::DateTime<chrono::Utc>,
}

/// 一轮扫描的产出
#[derive(Debug, Default)]
pub struct TrackerTick {
    /// 已结束的会话（调用方负责持久化）
    pub finished: Vec<FinishedSession>,
    /// 待命会话已转正式计时（前端可提示"开始计时"）
    pub activated: Vec<String>,
    /// 待命会话超时未发现进程（前端应提示"未检测到游戏启动"）
    pub arm_timeouts: Vec<String>,
}

/// 待检查活跃会话的快照（避免遍历时与 `&mut self` 借用冲突）
type SessionCheckInfo = (
    String,
    String,
    Option<String>,
    Option<u32>,
    Option<String>,
);

/// 游戏时长追踪器
pub struct PlayTimeTracker {
    active_sessions: HashMap<String, ActiveSession>,
    /// 待命会话：平台游戏委托客户端启动后，等待进程现身再转正
    pending_sessions: HashMap<String, PendingSession>,
    /// 复用 sysinfo System 实例，避免每10秒全量扫描。
    /// 用空实例 + 每次 check 时 refresh_processes 即可，无需启动时全量拉取（new_all 更重）
    sys: System,
}

impl PlayTimeTracker {
    pub fn new() -> Self {
        Self {
            active_sessions: HashMap::new(),
            pending_sessions: HashMap::new(),
            sys: System::new(),
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

        // 同一游戏若正处于待命状态，直接作废——已被显式启动，
        // 否则待命转正时会把本次会话覆盖、起始时刻被改回点击时刻
        self.pending_sessions.remove(game_id);

        self.active_sessions.insert(
            game_id.to_string(),
            ActiveSession {
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

    /// 待命启动：平台游戏经 URI 委托客户端启动后调用
    ///
    /// 与 `start_tracking` 的关键区别是**不立即计时**：平台游戏的进程由 Steam/Epic
    /// 客户端异步拉起（还要过 DRM 校验、云存档同步），在进程现身之前无法确认
    /// 游戏真的跑起来了。先登记待命，由 `check_active_sessions` 每轮尝试转正；
    /// 超过 `PLATFORM_ARM_WINDOW_SECS` 仍未现身则放弃并回报前端
    /// （避免"点了启动但游戏没起来"却记下一笔假时长）。
    ///
    /// 返回被顶掉的旧会话（语义与 `start_tracking` 一致）。
    pub fn arm_session(
        &mut self,
        game_id: &str,
        exe_name: &str,
        exe_path: Option<&str>,
        install_path: Option<&str>,
    ) -> Option<FinishedSession> {
        // 同一游戏重复点击启动：先结算正在计时的会话，再重新待命
        let finished = if self.active_sessions.contains_key(game_id) {
            self.stop_tracking_internal(game_id)
        } else {
            None
        };

        let now = chrono::Utc::now();
        let window = crate::utils::constants::PLATFORM_ARM_WINDOW_SECS;
        self.pending_sessions.insert(
            game_id.to_string(),
            PendingSession {
                exe_name: exe_name.to_string(),
                exe_path: exe_path.map(|s| s.to_string()),
                install_path: install_path.map(|s| s.to_string()),
                armed_at: now,
                deadline: now + chrono::Duration::seconds(window),
            },
        );

        tracing::info!(
            "待命启动游戏: {} (exe: {}, path: {:?}, install: {:?})，{} 秒内等待进程现身",
            game_id, exe_name, exe_path, install_path, window
        );
        finished
    }

    /// 在系统进程列表中查找目标游戏的进程，返回其 PID
    ///
    /// 三级匹配，优先级由高到低：
    /// 1. **exe 完整路径精确匹配** —— 最可靠，但依赖库里有准确的 exe_path；
    /// 2. **安装目录前缀匹配** —— 主力策略：Steam 的 acf 不记录 exe 名、
    ///    Epic 的 `LaunchExecutable` 可能是启动器壳（实测 `Launcher.exe` / `PlayRDR2.exe`），
    ///    这两类都只能靠"进程落在安装目录下"来识别；
    /// 3. **进程名匹配**（兜底，exe_name 非空时）。
    fn find_game_process(
        sys: &System,
        install_path: Option<&str>,
        exe_path: Option<&str>,
        exe_name: Option<&str>,
    ) -> Option<u32> {
        // 1. exe 完整路径精确匹配
        if let Some(target) = exe_path {
            let target = target.to_lowercase();
            for (pid, p) in sys.processes() {
                if let Some(exe) = p.exe() {
                    if exe.to_string_lossy().to_lowercase() == target {
                        return Some(pid.as_u32());
                    }
                }
            }
        }

        // 2. 安装目录前缀匹配（复用 exe_under_dir，避免 "Steam"/"SteamLibrary" 这类前缀误匹配）
        if let Some(dir) = install_path {
            if dir.len() >= 4 {
                let dir_lower = dir.to_lowercase();
                for (pid, p) in sys.processes() {
                    if let Some(exe) = p.exe() {
                        if Self::exe_under_dir(&exe.to_string_lossy().to_lowercase(), &dir_lower) {
                            return Some(pid.as_u32());
                        }
                    }
                }
            }
        }

        // 3. 进程名兜底
        if let Some(name) = exe_name {
            let name = name.to_lowercase();
            if !name.is_empty() {
                for (pid, p) in sys.processes() {
                    if p.name().to_lowercase() == name {
                        return Some(pid.as_u32());
                    }
                }
            }
        }

        None
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

    /// 判断进程 exe 路径是否位于安装目录下（精确分隔符匹配，避免 "D:\Games\Steam" 误匹配 "D:\Games\SteamLibrary"）
    fn exe_under_dir(exe_lower: &str, dir_lower: &str) -> bool {
        if exe_lower.len() <= dir_lower.len() {
            return exe_lower == dir_lower;
        }
        exe_lower.starts_with(dir_lower)
            && matches!(exe_lower.as_bytes().get(dir_lower.len()), Some(b'\\') | Some(b'/'))
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
    ///
    /// 返回本轮扫描产出（结束的会话 + 待命转正 + 待命超时），由调用方负责持久化与通知。
    pub fn check_active_sessions(&mut self) -> TrackerTick {
        let mut tick = TrackerTick::default();

        // 增量刷新进程列表，而非全量重建
        self.sys.refresh_processes();

        // 构建进程树供所有 session 复用
        let (parent_to_children, pid_to_exe) = Self::build_process_tree(&self.sys);

        // ============================================
        // 阶段 0: 待命会话转正 / 超时判定
        // ============================================
        self.process_pending_sessions(&mut tick);

        // 收集需要检查的会话信息，避免借用冲突
        let sessions_to_check: Vec<SessionCheckInfo> = self
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
                            Self::exe_under_dir(exe, &install_lower)
                        }) || self.sys.processes().values().any(|p| {
                            p.exe().is_some_and(|exe| {
                                Self::exe_under_dir(&exe.to_string_lossy().to_lowercase(), &install_lower)
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
                        p.exe().is_some_and(|exe| {
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
                    tick.finished.push(session);
                }
            }
        }

        tick
    }

    /// 处理待命会话：进程现身则转正，超窗则放弃
    ///
    /// 借用说明：先对 pending 做快照再遍历，避免在修改 `pending_sessions` 的同时持有其不可变借用。
    fn process_pending_sessions(&mut self, tick: &mut TrackerTick) {
        let now = chrono::Utc::now();
        let snapshot: Vec<(String, PendingSession)> = self
            .pending_sessions
            .iter()
            .map(|(id, ps)| (id.clone(), ps.clone()))
            .collect();

        for (game_id, ps) in snapshot {
            let hit = Self::find_game_process(
                &self.sys,
                ps.install_path.as_deref(),
                ps.exe_path.as_deref(),
                Some(ps.exe_name.as_str()),
            );

            if let Some(pid) = hit {
                self.pending_sessions.remove(&game_id);
                self.active_sessions.insert(
                    game_id.clone(),
                    ActiveSession {
                        exe_name: ps.exe_name.clone(),
                        exe_path: ps.exe_path.clone(),
                        spawned_pid: Some(pid),
                        install_path: ps.install_path.clone(),
                        // 起始时刻回溯到"点击启动那一刻"：加载期同样算作游玩，
                        // 因此轮询间隔不影响时长精度（误差只来自结束检测）。
                        start_time: ps.armed_at,
                    },
                );
                tracing::info!(
                    "待命转正：游戏 {} 命中进程 PID {}，起始时刻 {}",
                    game_id,
                    pid,
                    ps.armed_at.to_rfc3339()
                );
                tick.activated.push(game_id);
            } else if now >= ps.deadline {
                self.pending_sessions.remove(&game_id);
                tracing::warn!(
                    "待命超时：游戏 {} 在窗口期内未发现进程（可能在更新 / 被取消 / 客户端未登录），放弃计时",
                    game_id
                );
                tick.arm_timeouts.push(game_id);
            }
        }
    }

    /// 强制结束所有活跃会话（用于应用退出时的数据保全）
    ///
    /// 待命会话尚未产生任何时长，直接丢弃即可。
    pub fn force_finish_all(&mut self) -> Vec<FinishedSession> {
        self.pending_sessions.clear();
        let game_ids: Vec<String> = self.active_sessions.keys().cloned().collect();
        let mut finished = Vec::new();
        for game_id in game_ids {
            if let Some(session) = self.stop_tracking_internal(&game_id) {
                finished.push(session);
            }
        }
        finished
    }

    /// 把前台窗口进程反查回**待命会话**（返回 game_id）
    ///
    /// 场景：平台游戏经客户端拉起后，进程已经现身（甚至画面都出来了），但后台是
    /// 10 秒一轮的轮询，尚未把待命会话转正。用户这时候按截图键是完全合理的（游戏就在眼前），
    /// 若因为"还没转正"就忽略，就会表现为"从平台启动的游戏按截图键没反应"。
    /// 判定口径与 `find_active_game_by_process` 完全一致：进程名精确 → 安装目录前缀。
    ///
    /// 注意：这里**只用于放行截图**，不改变计时语义——转正仍由 `process_pending_sessions`
    /// 按轮询节奏负责，避免在截图路径里制造出起止时间不实的会话。
    pub fn find_pending_game_by_process(
        &self,
        exe_name: &str,
        full_path: Option<&str>,
    ) -> Option<String> {
        let needle = exe_name.to_lowercase();
        if !needle.is_empty() {
            if let Some((id, _)) = self
                .pending_sessions
                .iter()
                .find(|(_, s)| !s.exe_name.is_empty() && s.exe_name.to_lowercase() == needle)
            {
                return Some(id.clone());
            }
        }

        let full_lower = full_path?.to_lowercase();
        self.pending_sessions
            .iter()
            .find(|(_, s)| {
                s.install_path.as_deref().is_some_and(|dir| {
                    dir.len() >= 4 && Self::exe_under_dir(&full_lower, &dir.to_lowercase())
                })
            })
            .map(|(id, _)| id.clone())
    }

    /// 会话计数：(活跃, 待命)
    ///
    /// 供截图链路打日志用——「活跃 0 / 待命 1」与「活跃 1 / 待命 0」是完全不同的故障语义
    /// （前者是平台游戏尚未转正的空窗，后者才是真的匹配不上），日志里必须能分辨。
    pub fn session_counts(&self) -> (usize, usize) {
        (self.active_sessions.len(), self.pending_sessions.len())
    }

    /// 取某会话的 exe_name（活跃优先，其次待命），失败返回 None
    ///
    /// 供截图**归档命名**使用：手账与游戏详情页定位截图目录时用的都是
    /// `screenshot_dir_for_process(games.exe_name)`，所以截图也必须以同一个 exe_name 建目录，
    /// 否则 Epic 这类"启动器壳"（`LaunchExecutable` 常为 `Launcher.exe`/`PlayRDR2.exe`）
    /// 会按前台进程名建出壳名目录，UI 侧永远找不到（死目录）。
    ///
    /// 会话里的 exe_name 在两条启动路径上都取自 `games.exe_name`：
    /// 本地游戏 `start_tracking` 直接传库内字段，平台游戏 `arm_session` 同源。
    /// Steam 条目该字段恒为空（acf 不记录 exe 名），调用方需对空串回退。
    pub fn session_exe_name(&self, game_id: &str) -> Option<String> {
        if let Some(session) = self.active_sessions.get(game_id) {
            return Some(session.exe_name.clone());
        }
        self.pending_sessions
            .get(game_id)
            .map(|session| session.exe_name.clone())
    }

    /// 是否存在需要本轮扫描的工作（活跃会话或待命会话）
    ///
    /// 后台循环据此跳过空转：两者皆空时无需刷新进程列表。
    /// 注意不能只看活跃会话——平台游戏刚发起启动时只有待命会话，
    /// 若据此跳过扫描，待命将永远无法转正。
    pub fn has_pending_work(&self) -> bool {
        !self.active_sessions.is_empty() || !self.pending_sessions.is_empty()
    }

    /// 根据进程名（exe 文件名，忽略大小写）查找活跃游戏，返回 game_id。
    /// 用于截图时把前台窗口进程反查回游戏库条目。
    ///
    /// 空串一律不匹配：Steam 条目经 acf 导入时 `exe_name` 恒为空，
    /// 若允许"空串 == 空串"命中，前台进程名一旦取成空就会误判成 Steam 游戏。
    pub fn find_active_game_by_exe(&self, exe_name: &str) -> Option<String> {
        let needle = exe_name.to_lowercase();
        if needle.is_empty() {
            return None;
        }
        self.active_sessions
            .iter()
            .find(|(_, s)| s.exe_name.to_lowercase() == needle)
            .map(|(id, _)| id.clone())
    }

    /// 按 PID 查询进程可执行文件完整路径
    ///
    /// 供截图把前台窗口反查回游戏：`windows_capture` 的 Window 只给进程名与 PID、
    /// 不给完整路径，故此处用 sysinfo 增量刷新后单点查询。
    /// 提权/受保护进程（反作弊服务等）读不到路径，返回 None —— 这类进程匹配不上，
    /// 也就不会误判，属安全侧行为。
    pub fn process_path(&mut self, pid: u32) -> Option<String> {
        self.sys.refresh_processes();
        self.sys
            .process(Pid::from(pid as usize))
            .and_then(|p| p.exe())
            .map(|p| p.to_string_lossy().to_string())
    }

    /// 把前台窗口进程反查回活跃游戏（返回 game_id）
    ///
    /// 两级匹配：
    /// 1. **进程名精确**（快路径）—— 覆盖本地游戏与 exe 名准确的平台游戏；
    /// 2. **进程完整路径落在 install_path 目录下** —— 覆盖两类"进程名对不上"的情况：
    ///    - Epic 的 `LaunchExecutable` 可能是启动器壳（实测 `Launcher.exe` / `PlayRDR2.exe`），
    ///      真正跑起来的游戏进程名与之不符；
    ///    - Steam 条目根本没有 exe 名（acf 不记录 exe 名）。
    pub fn find_active_game_by_process(
        &self,
        exe_name: &str,
        full_path: Option<&str>,
    ) -> Option<String> {
        if let Some(id) = self.find_active_game_by_exe(exe_name) {
            return Some(id);
        }

        let full_lower = full_path?.to_lowercase();
        self.active_sessions
            .iter()
            .find(|(_, s)| {
                s.install_path.as_deref().is_some_and(|dir| {
                    dir.len() >= 4 && Self::exe_under_dir(&full_lower, &dir.to_lowercase())
                })
            })
            .map(|(id, _)| id.clone())
    }

    /// 持久化已结束的会话到数据库（在 Tracker 锁释放后调用）
    /// 一次性获取锁并批量插入（单事务），避免逐条获取/释放锁与逐条事务的开销
    pub fn persist_finished_sessions(
        db: &Arc<Mutex<Database>>,
        sessions: &[FinishedSession],
    ) {
        if sessions.is_empty() {
            return;
        }
        let db = db.lock().unwrap_or_else(|e| e.into_inner());
        let batch: Vec<(String, String, u64)> = sessions
            .iter()
            .map(|s| (s.game_id.clone(), s.start_time.clone(), s.duration_seconds))
            .collect();
        match db.add_play_sessions_batch(&batch) {
            Ok(recorded) => {
                // recorded 只统计独立成条的会话；合并与不足时长的短会话不生成新行
                tracing::info!(
                    "结算 {} 条结束会话：{} 条独立记录，其余为合并或短会话",
                    sessions.len(),
                    recorded
                );
            }
            Err(e) => {
                // 批量失败时回退到逐条保存，尽量不丢数据
                tracing::error!("批量保存游戏会话失败: {}", e);
                for session in sessions {
                    if let Err(e2) = db.add_play_session(
                        &session.game_id,
                        &session.start_time,
                        session.duration_seconds,
                    ) {
                        tracing::error!("保存游戏会话失败: {}", e2);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 本地游戏：exe 名与安装目录都齐，进程名精确命中
    #[test]
    fn active_session_matches_by_exe_name() {
        let mut tracker = PlayTimeTracker::new();
        tracker.start_tracking(
            "g1",
            "Frostpunk.exe",
            Some("E:\\Frostpunk\\Frostpunk.exe"),
            Some(1234),
            Some("E:\\Frostpunk"),
        );

        assert_eq!(
            tracker.find_active_game_by_process("Frostpunk.EXE", None).as_deref(),
            Some("g1"),
            "进程名匹配应忽略大小写"
        );
        // 会话里的 exe 名原样可查（截图归档命名要用它）
        assert_eq!(tracker.session_exe_name("g1").as_deref(), Some("Frostpunk.exe"));
    }

    /// Epic 启动器壳：进程名对不上，靠"进程路径落在安装目录下"命中
    #[test]
    fn active_session_matches_by_install_path_prefix() {
        let mut tracker = PlayTimeTracker::new();
        tracker.start_tracking(
            "rdr2",
            "Launcher.exe",
            None,
            Some(4321),
            Some("K:\\SteamLibrary\\steamapps\\common\\RDR2"),
        );

        assert_eq!(
            tracker
                .find_active_game_by_process("RDR2.exe", Some("k:\\steamlibrary\\STEAMAPPS\\common\\RDR2\\RDR2.exe"))
                .as_deref(),
            Some("rdr2")
        );
        // 目录前缀必须按分隔符边界判定：SteamLibrary 不能命中 SteamLibrary2
        assert_eq!(
            tracker.find_active_game_by_process(
                "other.exe",
                Some("K:\\SteamLibrary\\steamapps\\common\\RDR22\\other.exe")
            ),
            None
        );
        // 前台进程名取空时不得误配（Steam 条目 exe_name 恒为空的历史坑）
        assert_eq!(tracker.find_active_game_by_process("", None), None);
    }

    /// 待命期放行：进程已现身但尚未转正时，截图仍应认得出来；
    /// 一旦转正，待命侧就不再命中（避免两处都认）
    #[test]
    fn pending_session_accepted_then_retired_after_promotion() {
        let mut tracker = PlayTimeTracker::new();
        tracker.arm_session(
            "epic1",
            "PlayRDR2.exe",
            None,
            Some("K:\\Games\\RDR2"),
        );

        assert_eq!(
            tracker.find_pending_game_by_process("PlayRDR2.exe", None).as_deref(),
            Some("epic1"),
            "待命期按 exe 名应命中"
        );
        assert_eq!(
            tracker
                .find_pending_game_by_process("RDR2.exe", Some("K:\\Games\\RDR2\\RDR2.exe"))
                .as_deref(),
            Some("epic1"),
            "待命期按安装目录前缀应命中"
        );
        assert_eq!(
            tracker.find_pending_game_by_process("calc.exe", Some("C:\\Windows\\calc.exe")),
            None
        );
        // 归档命名取的是会话 exe 名（不是前台进程名），待命期同样可用
        assert_eq!(tracker.session_exe_name("epic1").as_deref(), Some("PlayRDR2.exe"));

        // 转正后：活跃侧认得，待命侧不再命中
        tracker.start_tracking("epic1", "PlayRDR2.exe", None, Some(999), Some("K:\\Games\\RDR2"));
        assert_eq!(
            tracker
                .find_active_game_by_process("RDR2.exe", Some("K:\\Games\\RDR2\\RDR2.exe"))
                .as_deref(),
            Some("epic1")
        );
        assert_eq!(
            tracker.find_pending_game_by_process("RDR2.exe", Some("K:\\Games\\RDR2\\RDR2.exe")),
            None
        );
        assert_eq!(tracker.session_exe_name("epic1").as_deref(), Some("PlayRDR2.exe"));
    }

    /// 会话计数：截图日志要能分辨「活跃 0 / 待命 1」与「活跃 1 / 待命 0」
    #[test]
    fn session_counts_reports_both_kinds() {
        let mut tracker = PlayTimeTracker::new();
        assert_eq!(tracker.session_counts(), (0, 0));

        tracker.arm_session("p1", "A.exe", None, Some("D:\\A"));
        assert_eq!(tracker.session_counts(), (0, 1));
        assert!(tracker.has_pending_work(), "待命也算待处理工作，否则永远不转正");

        tracker.start_tracking("a1", "B.exe", None, Some(1), Some("D:\\B"));
        assert_eq!(tracker.session_counts(), (1, 1));

        // 无 exe 名的会话（Steam 条目）：只能靠路径匹配，不得按空名乱认
        tracker.arm_session("steam1", "", None, Some("G:\\SteamLibrary\\steamapps\\common\\Game"));
        assert_eq!(tracker.find_pending_game_by_process("", None), None);
        assert_eq!(
            tracker
                .find_pending_game_by_process("Game.exe", Some("G:\\SteamLibrary\\steamapps\\common\\Game\\bin\\Game.exe"))
                .as_deref(),
            Some("steam1"),
            "Steam 条目无 exe 名，必须靠安装目录前缀命中"
        );
        assert_eq!(tracker.session_exe_name("steam1").as_deref(), Some(""));
    }
}
