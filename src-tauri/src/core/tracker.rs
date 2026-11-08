use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use sysinfo::{Pid, ProcessRefreshKind, System, UpdateKind};
use crate::models::*;
use crate::core::Database;  // 用于 persist_finished_sessions 的参数类型
use crate::core::{platform, steam_state};

/// 已结束的游戏会话数据，用于外部持久化
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishedSession {
    pub game_id: String,
    pub start_time: String,  // RFC 3339
    pub duration_seconds: u64,
}

/// 主动停止追踪的结果（前端「停止游戏」按钮用）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopOutcome {
    /// 有正式会话：已结算，携带时长供持久化
    Stopped(FinishedSession),
    /// 只有待命会话：直接丢弃，本就没有计时
    Cancelled,
    /// 该游戏没有任何会话
    NotFound,
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
    /// 委托的平台与 appid：Steam 用它查客户端自报的「更新中」状态
    platform: Option<String>,
    platform_id: Option<String>,
    /// 点击启动的时刻（仅用于诊断「等了多久」，**不再作为计时起点**）
    armed_at: chrono::DateTime<chrono::Utc>,
    /// 待命截止时刻；Steam 更新期间每拍续期，超时视为未启动
    deadline: chrono::DateTime<chrono::Utc>,
    /// 「进入更新态」事件是否已通知前端，避免每秒刷一次
    notified_updating: bool,
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
    /// 客户端明确报告「正在更新」，前端应显示更新态而非游玩中
    pub updating: Vec<String>,
    /// 更新态已结束、进程尚未现身：前端应从「更新中」回落到「启动中」
    pub waiting: Vec<String>,
}

/// 单个待命会话本拍的裁决（纯函数，便于单测）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingDecision {
    /// 客户端正在更新：续期且**不转正**，哪怕安装目录里冒出了进程
    Renew,
    /// 游戏进程现身，可转正式会话
    Activate(u32),
    /// 超窗且无进程：放弃
    Timeout,
    /// 继续等
    Wait,
}

/// 待检查活跃会话的快照（避免遍历时与 `&mut self` 借用冲突）
type SessionCheckInfo = (
    String,
    String,
    Option<String>,
    Option<u32>,
    Option<String>,
);

/// 判定「有 CPU 活动」的下限（sysinfo 的 `cpu_usage` 口径：占全系统 CPU 的份额）
///
/// 存在意义是把「真的零活动」与「浮点噪声」分开：挂死进程恒为 0.0，
/// 而任何还在跑的进程（哪怕只剩一个消息泵线程）都会远高于 0.01。
const CPU_ACTIVE_EPSILON: f32 = 0.01;

/// 单个进程相对「本游戏」的身份判定（2026-10-20）
///
/// 病灶回顾：旧实现只问「进程是否存在」。于是游戏退出后，任何残留在进程树上的
/// 子孙（UE 的崩溃上报器、被系统复用的 PID、ppid 恰好落到本 PID 上的无关进程）
/// 都能把会话一直顶成「游玩中」——《猛鬼追魂》实测多记 52 分钟。
/// 现在每个候选进程都必须先过这道身份闸门。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcIdentity {
    /// 可证是游戏本体：exe 路径命中 `exe_path` / 落在 `install_path` 下 / 进程名命中 `exe_name`
    Game,
    /// 可证**不是**游戏（读到了 exe 路径且三条全不命中）——最硬的排除证据
    Foreign,
    /// 读不到 exe 路径（受保护进程、32-bit 老游戏）或会话没有任何身份签名 → 无法证伪
    Unknown,
}

/// 一轮全表裁决的结论（替代原来的裸 `bool`，把「接棒」与「诊断」一并带出）
#[derive(Debug, Default)]
struct SessionVerdict {
    /// 会话是否还应继续计时
    running: bool,
    /// 命中哪一级策略（日志用）
    strategy: &'static str,
    /// 原锚点已不可用、需要把会话改跟这个进程（PID, exe 路径）
    adopt: Option<(u32, String)>,
    /// 被身份闸门排除掉的存活子孙（PID, 进程名）——结算时落日志，定位"是谁顶住了会话"
    excluded: Vec<(u32, String)>,
}

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
                poll_cooldown_until: None,
                hang: HangWatch::default(),
                last_note: None,
            },
        );

        tracing::info!(
            "开始追踪游戏: {} (exe: {}, path: {:?}, pid: {:?}, install: {:?})",
            game_id, exe_name, exe_path, spawned_pid, install_path
        );
        finished
    }

    /// 待命启动（无平台信号的兼容入口，仅单测使用；生产路径走 `arm_platform_session`）
    #[cfg(test)]
    pub fn arm_session(
        &mut self,
        game_id: &str,
        exe_name: &str,
        exe_path: Option<&str>,
        install_path: Option<&str>,
    ) -> Option<FinishedSession> {
        self.arm_platform_session(game_id, exe_name, exe_path, install_path, None, None)
    }

    /// 待命启动：平台游戏经 URI 委托客户端启动后调用
    ///
    /// 与 `start_tracking` 的关键区别是**不立即计时**：平台游戏的进程由 Steam/Epic
    /// 客户端异步拉起（还要过 DRM 校验、云存档同步、甚至先下载更新），在进程现身
    /// 之前无法确认游戏真的跑起来了。先登记待命，由 `check_active_sessions` 每轮
    /// 尝试转正；Steam 报告 `Updating=1` 时续期，否则超过窗口仍未现身则放弃。
    ///
    /// 返回被顶掉的旧会话（语义与 `start_tracking` 一致）。
    pub fn arm_platform_session(
        &mut self,
        game_id: &str,
        exe_name: &str,
        exe_path: Option<&str>,
        install_path: Option<&str>,
        platform_name: Option<&str>,
        platform_id: Option<&str>,
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
                platform: platform_name.map(|s| s.to_string()),
                platform_id: platform_id.map(|s| s.to_string()),
                armed_at: now,
                deadline: now + chrono::Duration::seconds(window),
                notified_updating: false,
            },
        );

        tracing::info!(
            "待命启动游戏: {} (exe: {}, path: {:?}, install: {:?}, platform: {:?}/{:?})，{} 秒内等待进程现身",
            game_id, exe_name, exe_path, install_path, platform_name, platform_id, window
        );
        finished
    }

    /// 该进程是否「看起来就绝不是游戏本体」（伴随 / 安装 / 更新 / 反作弊进程）
    ///
    /// 这些进程同样躺在游戏安装目录下，只靠路径前缀识别会把它们当成游戏转正。
    /// 名单按前缀匹配，且只收绝无可能是主程序的名字（见 constants 注释）。
    fn is_non_game_process(name: &str) -> bool {
        if Self::is_helper_process(name) {
            return true;
        }
        let lower = name.to_lowercase();
        let stem = lower.strip_suffix(".exe").unwrap_or(&lower);
        // 安装/更新器常带游戏名前缀（GameUpdater.exe / PBZ_setup.exe），
        // 故这里按子串匹配；名单全是「绝不可能是主程序」的语义词，误伤风险可控
        crate::utils::constants::GAME_SETUP_HELPERS
            .iter()
            .any(|helper| stem.contains(helper))
    }

    /// 在系统进程列表中查找目标游戏的进程，返回其 PID
    ///
    /// 三级匹配，优先级由高到低：
    /// 1. **exe 完整路径精确匹配** —— 最可靠，但依赖库里有准确的 exe_path；
    /// 2. **安装目录前缀匹配** —— 主力策略：Steam 的 acf 不记录 exe 名、
    ///    Epic 的 `LaunchExecutable` 可能是启动器壳（实测 `Launcher.exe` / `PlayRDR2.exe`），
    ///    这两类都只能靠"进程落在安装目录下"来识别；
    /// 3. **进程名匹配**（兜底，exe_name 非空时）。
    ///
    /// 【2026-11-04 身份闸门】每一级都先排除伴随/安装/更新进程，并用 `classify`
    /// 复核；第 2 级不再「撞到谁算谁」，而是收集全部候选后交给 `pick_anchor`
    /// 挑最像本体的那个。现场教训：11-03 待命转正锚点 PID 4828，
    /// 真正的前台游戏却是 PID 20144——旧实现把同目录的另一个进程当成了本体。
    fn find_game_process(
        sys: &System,
        install_path: Option<&str>,
        exe_path: Option<&str>,
        exe_name: Option<&str>,
    ) -> Option<u32> {
        let wanted_name = exe_name.unwrap_or("");

        // 1. exe 完整路径精确匹配（取最小 PID，保证结果确定）
        if let Some(target) = exe_path {
            let target = target.to_lowercase();
            let mut best: Option<u32> = None;
            for (pid, p) in sys.processes() {
                if Self::is_non_game_process(p.name()) {
                    continue;
                }
                if let Some(exe) = p.exe() {
                    if exe.to_string_lossy().to_lowercase() == target {
                        let id = pid.as_u32();
                        best = Some(best.map_or(id, |b| b.min(id)));
                    }
                }
            }
            if best.is_some() {
                return best;
            }
        }

        // 2. 安装目录前缀匹配（复用 exe_under_dir，避免 "Steam"/"SteamLibrary" 这类前缀误匹配）
        if let Some(dir) = install_path {
            if dir.len() >= 4 {
                let dir_lower = dir.to_lowercase();
                let candidates: Vec<(u32, String, String)> = sys
                    .processes()
                    .iter()
                    .filter_map(|(pid, p)| {
                        let name = p.name().to_string();
                        if Self::is_non_game_process(&name) {
                            return None;
                        }
                        let exe = p.exe()?.to_string_lossy().to_lowercase();
                        if !Self::exe_under_dir(&exe, &dir_lower) {
                            return None;
                        }
                        // 身份闸门：读得到路径且三条签名全不命中才算外人
                        if Self::classify(
                            Some(&exe),
                            &name,
                            wanted_name,
                            exe_path,
                            install_path,
                        ) == ProcIdentity::Foreign
                        {
                            return None;
                        }
                        Some((pid.as_u32(), exe, name))
                    })
                    .collect();
                if let Some(idx) = Self::pick_anchor(&candidates, wanted_name) {
                    return Some(candidates[idx].0);
                }
            }
        }

        // 3. 进程名兜底
        if !wanted_name.is_empty() {
            let name = wanted_name.to_lowercase();
            let mut best: Option<u32> = None;
            for (pid, p) in sys.processes() {
                if Self::is_non_game_process(p.name()) {
                    continue;
                }
                if p.name().to_lowercase() == name {
                    let id = pid.as_u32();
                    best = Some(best.map_or(id, |b| b.min(id)));
                }
            }
            if best.is_some() {
                return best;
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

    /// 本轮全表刷新使用的口径：**仅 exe**。
    ///
    /// 从 `refresh_processes()`（内存 + CPU + 磁盘 I/O + exe）降为只刷 exe 后，
    /// 单轮实测 2.7~4.4ms（本机 300 上下进程数），比旧口径省约三分之一。
    /// tracker 只用 exe / name / parent / 存在性这四项，而：
    /// - `name` 来自 `NtQuerySystemInformation` 的内核快照，与刷新口径无关；
    /// - `parent` 每轮**无条件**更新（sysinfo `windows/system.rs`）；
    /// - 死进程清理靠 `updated` 标记，同样与口径无关。
    ///
    /// 故降级对判定语义**零影响**（已用探针逐项核对：两口径下 exe/name/parent
    /// 的字段覆盖率完全一致，新进程发现与退出清理行为也一致）。
    fn light_kind() -> ProcessRefreshKind {
        ProcessRefreshKind::new().with_exe(UpdateKind::OnlyIfNotSet)
    }

    /// 按需做一次全表刷新 —— 分层扫描里只在「待命期」与「点查报死」两处调用
    fn refresh_all(&mut self) {
        self.sys.refresh_processes_specifics(Self::light_kind());
    }

    /// 检查活跃会话（定期调用）
    ///
    /// 返回本轮扫描产出（结束的会话 + 待命转正 + 待命超时），由调用方负责持久化与通知。
    ///
    /// 【2026-09-19 分层扫描】旧实现每拍无条件全表刷新 + 建进程树，成本约 7ms，
    /// 因此轮询只能停留在 10 秒。现拆成三层，使 1 秒间隔成为可能：
    /// - 阶段 0（待命期）才全表刷新：要发现的是**未知的新进程**，没有 PID 可以先验地查；
    /// - 阶段 1（计时期）只做单 PID 点查：实测约 0.4 微秒/会话，不刷表、不建树；
    /// - 阶段 2（疑似退出）才全表 + 三级判定：点查只能证明「根 PID 没了」，
    ///   证明不了「游戏没了」，必须靠进程树往下看子孙（父退子活）。
    pub fn check_active_sessions(&mut self) -> TrackerTick {
        let mut tick = TrackerTick::default();

        // ============================================
        // 阶段 0: 待命会话转正 / 超时判定
        //   待命期必须全表刷新——要发现的是「未知的新进程」。
        // ============================================
        let mut full_refreshed = false;
        if !self.pending_sessions.is_empty() {
            self.refresh_all();
            full_refreshed = true;
            self.process_pending_sessions(&mut tick);
        }

        // ============================================
        // 阶段 0.5: 挂死观察（P1，2026-10-20）
        //   进程树还在、但 CPU 时间长时间零增长 → 判「退出挂起」，兜底结算。
        //   放在点查之前：冷却期内的会话同样要接受观察，否则受保护进程的会话
        //   一旦进了冷却就再也不看 CPU 了。
        // ============================================
        self.poll_hang_watch(&mut tick);

        // ============================================
        // 阶段 1: 计时期单点存活检查（约 0.4 微秒/会话）
        //   绝大多数轮次到这里就结束——活着的会话被放行，**完全不碰全表、不建进程树**。
        //   只有「点查报死」或「本来就没有 PID」的会话才进入 suspects，交阶段 2 裁决。
        // ============================================
        let now = std::time::Instant::now();
        let mut suspects: Vec<SessionCheckInfo> = Vec::new();

        for (game_id, session) in self.active_sessions.iter() {
            // 冷却期内：上一轮「点查说死、全表说活」→ 该会话点查不可信，本拍直接放过
            if session
                .poll_cooldown_until
                .is_some_and(|deadline| now < deadline)
            {
                continue;
            }

            match session.spawned_pid {
                Some(pid) => {
                    // 单点查询：false = 进程不存在或已退出。
                    // sysinfo 内部用**缓存句柄**判活并比对启动时刻（自带 PID 复用防护），
                    // 既不刷新全表、也不依赖进程表内容，故这里可以放心高频调用。
                    if !self
                        .sys
                        .refresh_process_specifics(Pid::from(pid as usize), Self::light_kind())
                    {
                        suspects.push((
                            game_id.clone(),
                            session.exe_name.clone(),
                            session.exe_path.clone(),
                            Some(pid),
                            session.install_path.clone(),
                        ));
                    }
                }
                // 无 PID 的会话（正常启动路径不会出现）：只能退回全表判定
                None => suspects.push((
                    game_id.clone(),
                    session.exe_name.clone(),
                    session.exe_path.clone(),
                    None,
                    session.install_path.clone(),
                )),
            }
        }

        // ============================================
        // 阶段 2: 疑似退出裁决 —— 只有到这一步才值得付出全表刷新的代价
        // ============================================
        if !suspects.is_empty() {
            if !full_refreshed {
                self.refresh_all();
            }
            let (parent_to_children, pid_to_exe) = Self::build_process_tree(&self.sys);

            // 先出裁决，再动 active_sessions（避免与 self.sys 的借用冲突）
            let verdicts: Vec<SessionVerdict> = {
                let sys = &self.sys;
                suspects
                    .iter()
                    .map(|info| Self::evaluate_session(sys, &parent_to_children, &pid_to_exe, info))
                    .collect()
            };

            let cooldown = std::time::Duration::from_secs(
                crate::utils::constants::POLL_FALLBACK_COOLDOWN_SECS,
            );

            for (info, verdict) in suspects.iter().zip(verdicts) {
                let game_id = info.0.as_str();
                if verdict.running {
                    match verdict.adopt {
                        // 原锚点已退出/被复用，但树里还有身份可信的子孙 → 接棒，
                        // 之后点查只认新锚点（旧实现是每拍全表重判，既慢又给了孤儿顶住的机会）
                        Some((new_pid, ref exe)) => {
                            if let Some(session) = self.active_sessions.get_mut(game_id) {
                                let old_pid = session.spawned_pid;
                                session.spawned_pid = Some(new_pid);
                                session.poll_cooldown_until = None;
                                // 换了观察对象，CPU 观察与日志去重都必须重来
                                session.hang = HangWatch::default();
                                session.last_note = None;
                                tracing::info!(
                                    "游戏 {} 接棒：原锚点 PID {:?} 已退出，改跟 PID {}（{}，策略 {}）",
                                    game_id,
                                    old_pid,
                                    new_pid,
                                    exe,
                                    verdict.strategy
                                );
                            }
                        }
                        // 锚点仍在（点查不可信）：打冷却标记，免得每拍都在它身上做一次全表刷新。
                        // 日志按状态去重——同一会话同一状态只打一次，幽灵会话挂几十分钟也只留一行。
                        None => {
                            if let Some(session) = self.active_sessions.get_mut(game_id) {
                                session.poll_cooldown_until =
                                    Some(std::time::Instant::now() + cooldown);
                                let note = format!("fallback:{}", verdict.strategy);
                                if session.last_note.as_deref() != Some(note.as_str()) {
                                    session.last_note = Some(note);
                                    tracing::warn!(
                                        "游戏 {} 单点查询报退出，但全表判定确认仍在运行（策略 {}），\
                                         该会话 {} 秒内只做全表确认",
                                        game_id,
                                        verdict.strategy,
                                        cooldown.as_secs()
                                    );
                                }
                            }
                        }
                    }
                } else {
                    let pid = self
                        .active_sessions
                        .get(game_id)
                        .and_then(|s| s.spawned_pid);
                    tracing::info!(
                        "游戏 {} 已退出 (anchor_pid: {:?}, 策略: {}, 已排除的非本体存活子孙: {})",
                        game_id,
                        pid,
                        verdict.strategy,
                        Self::describe_excluded(&verdict.excluded)
                    );
                    if let Some(session) = self.stop_tracking_internal(game_id) {
                        tick.finished.push(session);
                    }
                }
            }
        }

        tick
    }

    /// 对一个会话做完整的三级存活判定（进程树 → 安装目录 → exe 名）
    ///
    /// 这是**权威口径**，只在「点查报死」时才调用，因此可以承受全表遍历。
    /// 三级全落空才判定已退出。
    ///
    /// 之所以不能只靠单 PID 点查结算：点查只能证明「根 PID 没了」，证明不了「游戏没了」。
    /// 游戏常见「启动器校验完自己退出、主进程独立跑」的形态（父退子活），
    /// 此时根 PID 早已消失而游戏仍在，必须靠进程树往下看子孙才不误判。
    ///
    /// 【2026-10-20 两道新闸门】旧实现「树里有活进程 = 游戏在跑」，
    /// 《猛鬼追魂》退出后被 4 个身份不明的残留进程顶了 52 分钟。现在：
    /// 1. **身份闸门** —— 每个候选都要能对上 exe 路径 / 安装目录 / 进程名，
    ///    可证为 `Foreign`（读到路径却处处对不上，含 PID 被复用）的一律不算；
    /// 2. **接棒锚定** —— 原锚点没了就改跟树里那个"最像游戏本体"的进程，
    ///    此后只认它。孤儿进程再多也顶不住，因为不再有"任意子孙算数"这条口子。
    fn evaluate_session(
        sys: &System,
        parent_to_children: &HashMap<Pid, Vec<Pid>>,
        pid_to_exe: &HashMap<Pid, String>,
        info: &SessionCheckInfo,
    ) -> SessionVerdict {
        let (game_id, exe_name, exe_path, spawned_pid, install_path) = info;
        let exe_path = exe_path.as_deref();
        let install_path = install_path.as_deref();
        let mut verdict = SessionVerdict::default();

        // ============================================
        // 策略 1: 进程树检测 (最可靠，但要过身份闸门)
        // ============================================
        if let Some(pid) = *spawned_pid {
            let root_pid = Pid::from(pid as usize);
            let mut root_ok = false;

            if sys.processes().contains_key(&root_pid) {
                if Self::counts_as_game(sys, root_pid, exe_name, exe_path, install_path) {
                    root_ok = true;
                    verdict.running = true;
                    verdict.strategy = "tree:root";
                } else {
                    // 进程表里还有这个 PID，但读到的 exe 与三条签名都对不上：
                    // 要么 PID 已被系统复用，要么锚点本身只是个启动壳（如 gv-launch）。
                    // 两种情况都不能再据它判活，改看子孙与安装目录。
                    tracing::info!(
                        "游戏 {} 的会话锚点 PID {} 与游戏身份不符（当前进程 {}），\
                         可能已被系统复用，改按子孙与安装目录判定",
                        game_id,
                        pid,
                        sys.process(root_pid)
                            .map(|p| p.name().to_string())
                            .unwrap_or_default()
                    );
                }
            }

            // 锚点 + 全部子孙，逐个过身份闸门
            let mut tree_pids: Vec<u32> = vec![pid];
            tree_pids.extend(
                Self::collect_descendants(pid, parent_to_children)
                    .into_iter()
                    .map(|p| p.as_u32()),
            );
            tree_pids.sort_unstable();
            tree_pids.dedup();

            let mut trusted: Vec<(u32, String, String)> = Vec::new(); // (pid, exe, name)
            for tree_pid in tree_pids {
                if tree_pid == pid && root_ok {
                    continue; // 锚点已计入
                }
                let target = Pid::from(tree_pid as usize);
                let Some(proc_info) = sys.process(target) else {
                    continue; // 已退出
                };
                let name = proc_info.name().to_string();
                let exe = proc_info
                    .exe()
                    .map(|e| e.to_string_lossy().to_string())
                    .unwrap_or_default();

                // 【2026-11-08】与 find_game_process 共用同一份身份闸门：
                // 安装/更新器也躺在安装目录下，只挡崩溃上报器会让游戏退出后
                // 仍被判「在跑」（classify 对目录内进程恒 Game，永远兜不住）。
                if Self::is_non_game_process(&name)
                    || Self::classify(Some(exe.as_str()), &name, exe_name, exe_path, install_path)
                        == ProcIdentity::Foreign
                {
                    verdict.excluded.push((tree_pid, name));
                } else {
                    trusted.push((tree_pid, exe, name));
                }
            }

            if !trusted.is_empty() {
                verdict.running = true;
                if !root_ok {
                    verdict.strategy = "tree:handoff";
                    // 接棒：优先选与库内 exe 同干的进程（Hellraiser.exe → Hellraiser-Win64-Shipping.exe）
                    if let Some(idx) = Self::pick_anchor(&trusted, exe_name) {
                        verdict.adopt = Some((trusted[idx].0, trusted[idx].1.clone()));
                    }
                }
            }
        }

        // ============================================
        // 策略 2: 安装目录检测 (回退)
        // ============================================
        if !verdict.running {
            if let Some(install) = install_path.filter(|d| d.len() >= 4) {
                let install_lower = install.to_lowercase();
                let mut hits: Vec<(u32, String, String)> = Vec::new();

                // 两层扫描：先吃 pid_to_exe 缓存，再遍历进程表（覆盖 exe() 缓存缺失的）
                for (pid, exe) in pid_to_exe.iter() {
                    if Self::exe_under_dir(exe, &install_lower) {
                        if let Some(p) = sys.process(*pid) {
                            let name = p.name().to_string();
                            if !Self::is_non_game_process(&name) {
                                hits.push((pid.as_u32(), exe.clone(), name));
                            }
                        }
                    }
                }
                for (pid, p) in sys.processes().iter() {
                    if hits.iter().any(|(known, _, _)| *known == pid.as_u32()) {
                        continue;
                    }
                    let Some(exe) = p.exe() else { continue };
                    let exe_lower = exe.to_string_lossy().to_lowercase();
                    if !Self::exe_under_dir(&exe_lower, &install_lower) {
                        continue;
                    }
                    let name = p.name().to_string();
                    if Self::is_non_game_process(&name) {
                        continue; // 崩溃上报器 / 安装器 / 更新器躺在安装目录下，但不是游戏
                    }
                    hits.push((pid.as_u32(), exe_lower, name));
                }

                if !hits.is_empty() {
                    hits.sort_by_key(|(pid, _, _)| *pid);
                    verdict.running = true;
                    verdict.strategy = "install-prefix";
                    if let Some(idx) = Self::pick_anchor(&hits, exe_name) {
                        verdict.adopt = Some((hits[idx].0, hits[idx].1.clone()));
                    }
                }
            }
        }

        // ============================================
        // 策略 3: exe 文件名/路径匹配 (兼容旧数据)
        // ============================================
        if !verdict.running {
            if let Some(expected_path) = exe_path.filter(|p| !p.is_empty()) {
                let expected_lower = expected_path.to_lowercase();
                let hit = sys.processes().iter().find(|(_, p)| {
                    p.exe()
                        .is_some_and(|exe| exe.to_string_lossy().to_lowercase() == expected_lower)
                });
                if let Some((pid, _)) = hit {
                    verdict.running = true;
                    verdict.strategy = "exe-path";
                    verdict.adopt = Some((pid.as_u32(), expected_lower));
                }
            } else if !exe_name.is_empty() {
                let needle = exe_name.to_lowercase();
                let hit = sys
                    .processes()
                    .iter()
                    .find(|(_, p)| p.name().to_lowercase() == needle);
                if let Some((pid, _)) = hit {
                    verdict.running = true;
                    verdict.strategy = "exe-name";
                    verdict.adopt = Some((pid.as_u32(), String::new()));
                }
            }
        }

        if !verdict.running {
            verdict.strategy = "none";
        }
        verdict
    }

    /// 进程身份判定（纯函数，便于单测）
    ///
    /// 三条命中任一条即 `Game`：exe 路径等于 `exe_path`、exe 落在 `install_path` 下、
    /// 进程名等于 `exe_name`（后者专门救"启动器壳"：库里记 `Launcher.exe` 时，
    /// 真正跑起来的却是另一个名字，这条按名字放行）。
    ///
    /// 判 `Foreign` 的唯一硬证据是「读到了 exe 路径，且三条都对不上」。
    /// 读不到路径（受保护进程 / 32-bit 老游戏）只能算 `Unknown`——不可证伪即不可定罪。
    /// 会话没有任何签名（三条全空）时同样返回 `Unknown`，等价于沿用旧口径。
    fn classify(
        exe: Option<&str>,
        name: &str,
        exe_name: &str,
        exe_path: Option<&str>,
        install_path: Option<&str>,
    ) -> ProcIdentity {
        let exe_lower = exe.map(|e| e.to_lowercase());
        let has_signature = exe_path.is_some_and(|p| !p.is_empty())
            || install_path.is_some_and(|d| d.len() >= 4)
            || !exe_name.is_empty();
        if !has_signature {
            return ProcIdentity::Unknown;
        }

        if let Some(target) = exe_path.filter(|p| !p.is_empty()) {
            if exe_lower.as_deref() == Some(target.to_lowercase().as_str()) {
                return ProcIdentity::Game;
            }
        }
        if let Some(dir) = install_path.filter(|d| d.len() >= 4) {
            if let Some(exe) = exe_lower.as_deref() {
                if Self::exe_under_dir(exe, &dir.to_lowercase()) {
                    return ProcIdentity::Game;
                }
            }
        }
        if !exe_name.is_empty() && name.to_lowercase() == exe_name.to_lowercase() {
            return ProcIdentity::Game;
        }

        match exe_lower {
            Some(_) => ProcIdentity::Foreign,
            None => ProcIdentity::Unknown,
        }
    }

    /// 该进程是否属于「同树但不是游戏本体」的伴随进程（崩溃上报器等）
    fn is_helper_process(name: &str) -> bool {
        let lower = name.to_lowercase();
        let stem = lower.strip_suffix(".exe").unwrap_or(&lower);
        crate::utils::constants::GAME_SIBLING_HELPERS
            .iter()
            .any(|helper| stem.starts_with(helper))
    }

    /// 进程是否可计入「游戏仍在运行」（= 身份不是 Foreign，且不是伴随进程）
    fn counts_as_game(
        sys: &System,
        pid: Pid,
        exe_name: &str,
        exe_path: Option<&str>,
        install_path: Option<&str>,
    ) -> bool {
        let Some(proc_info) = sys.process(pid) else {
            return false;
        };
        let name = proc_info.name().to_string();
        if Self::is_non_game_process(&name) {
            return false;
        }
        let exe = proc_info
            .exe()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default();
        Self::classify(Some(exe.as_str()), &name, exe_name, exe_path, install_path)
            != ProcIdentity::Foreign
    }

    /// exe 名去扩展名的小写主干（`Hellraiser.exe` → `hellraiser`）
    fn exe_stem(name: &str) -> String {
        let lower = name.to_lowercase();
        lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
    }

    /// 从候选里挑接棒锚点：与库内 exe 同干的优先（`Hellraiser.exe` → `Hellraiser-Win64-Shipping.exe`），
    /// 其次取 PID 最小者（确定性，且启动早的通常是主进程）
    fn pick_anchor(candidates: &[(u32, String, String)], exe_name: &str) -> Option<usize> {
        if candidates.is_empty() {
            return None;
        }
        let wanted = Self::exe_stem(exe_name);
        if !wanted.is_empty() {
            if let Some(idx) = candidates
                .iter()
                .position(|(_, _, name)| Self::exe_stem(name) == wanted)
            {
                return Some(idx);
            }
            if let Some(idx) = candidates
                .iter()
                .position(|(_, _, name)| Self::exe_stem(name).starts_with(&wanted))
            {
                return Some(idx);
            }
        }
        Some(
            candidates
                .iter()
                .enumerate()
                .min_by_key(|(_, (pid, _, _))| *pid)
                .map(|(idx, _)| idx)
                .unwrap_or(0),
        )
    }

    /// 结算日志用：把被身份闸门排除的存活子孙渲染成一行（最多 6 个）
    fn describe_excluded(excluded: &[(u32, String)]) -> String {
        if excluded.is_empty() {
            return "无".to_string();
        }
        let mut parts: Vec<String> = excluded
            .iter()
            .take(6)
            .map(|(pid, name)| format!("{}({})", name, pid))
            .collect();
        if excluded.len() > 6 {
            parts.push(format!("…共 {} 个", excluded.len()));
        }
        parts.join(", ")
    }

    /// 挂死观察（P1）：进程树还在、CPU 却连续 `HANG_CPU_IDLE_SECS` 秒毫无活动 → 强制结算
    ///
    /// 为什么需要它：UE5 游戏退出时会出现"存档写完、窗口消失、进程挂在关机流程里"，
    /// 身份闸门拦不住（那确实是游戏本体的进程），只有行为证据能证伪。
    ///
    /// 三重保守边界，缺一不可：
    /// 1. **读不到 CPU 就闭嘴**（受保护进程）——`None` 不推进、不判死；
    /// 2. **有可见窗口就闭嘴** —— 最小化 / 后台挂机同样可能近乎零 CPU，
    ///    只有连窗口都没了（真退出后的挂起）才敢动刀；
    /// 3. **整棵可信进程树的合计**才算数，任一进程有活动即重置计时。
    fn poll_hang_watch(&mut self, tick: &mut TrackerTick) {
        let now = std::time::Instant::now();
        let sample_interval =
            std::time::Duration::from_secs(crate::utils::constants::HANG_CPU_SAMPLE_SECS);

        let due: Vec<(String, u32)> = self
            .active_sessions
            .iter()
            .filter_map(|(id, session)| {
                let pid = session.spawned_pid?;
                let ready = session.hang.next_sample_at.is_none_or(|at| now >= at);
                ready.then(|| (id.clone(), pid))
            })
            .collect();
        if due.is_empty() {
            return;
        }

        // 采样要连子孙一起算，故必须整表刷新（每个会话最多每分钟一次）
        self.refresh_all();
        let (parent_to_children, _) = Self::build_process_tree(&self.sys);

        for (game_id, anchor) in due {
            let Some((exe_name, exe_path, install_path)) = self
                .active_sessions
                .get(&game_id)
                .map(|s| (s.exe_name.clone(), s.exe_path.clone(), s.install_path.clone()))
            else {
                continue;
            };

            let usage = self.sample_tree_cpu_usage(
                anchor,
                &parent_to_children,
                &exe_name,
                exe_path.as_deref(),
                install_path.as_deref(),
            );

            let threshold =
                std::time::Duration::from_secs(crate::utils::constants::HANG_CPU_IDLE_SECS);
            let mut reached_threshold = false;
            if let Some(session) = self.active_sessions.get_mut(&game_id) {
                session.hang.next_sample_at = Some(now + sample_interval);
                match usage {
                    // 读不到 CPU：受保护进程/进程已消失，两种都不该据此判死
                    None => {}
                    Some(usage) => {
                        session.hang.last_usage = usage;
                        if usage > CPU_ACTIVE_EPSILON {
                            session.hang.idle_since = None;
                        } else {
                            let since = *session.hang.idle_since.get_or_insert(now);
                            if now.duration_since(since) >= threshold {
                                reached_threshold = true;
                            }
                        }
                    }
                }
            }
            if !reached_threshold {
                continue;
            }

            // 零 CPU 也可能是「最小化 / 后台挂机」：窗口还在就放过，并重新起算下一个周期
            let mut tree_pids: Vec<u32> = vec![anchor];
            tree_pids.extend(
                Self::collect_descendants(anchor, &parent_to_children)
                    .into_iter()
                    .map(|p| p.as_u32()),
            );
            if Self::any_visible_window(&tree_pids) {
                if let Some(session) = self.active_sessions.get_mut(&game_id) {
                    session.hang.idle_since = Some(now);
                }
                tracing::info!(
                    "游戏 {} 进程树 CPU 近 {} 秒无活动，但仍有可见窗口（最小化/后台挂机），不结算",
                    game_id,
                    threshold.as_secs()
                );
                continue;
            }

            let usage_note = self
                .active_sessions
                .get(&game_id)
                .map(|s| s.hang.last_usage)
                .unwrap_or(0.0);
            tracing::warn!(
                "游戏 {} 的进程树（锚点 PID {}）已无可视窗口且 CPU 连续 {} 秒无活动\
                 （最近一次采样 {:.3}），判定退出挂起，强制结算",
                game_id,
                anchor,
                threshold.as_secs(),
                usage_note
            );
            if let Some(session) = self.stop_tracking_internal(&game_id) {
                tick.finished.push(session);
            }
        }
    }

    /// 采样「锚点 + 身份可信子孙」的 CPU 占用合计（sysinfo 口径，零 = 该采样窗口内毫无活动）
    ///
    /// 返回 `None` 表示**一个可读进程都没有**（受保护进程 / 已全死），调用方不得据此判死。
    fn sample_tree_cpu_usage(
        &mut self,
        anchor: u32,
        parent_to_children: &HashMap<Pid, Vec<Pid>>,
        exe_name: &str,
        exe_path: Option<&str>,
        install_path: Option<&str>,
    ) -> Option<f32> {
        let mut pids: Vec<u32> = vec![anchor];
        pids.extend(
            Self::collect_descendants(anchor, parent_to_children)
                .into_iter()
                .map(|p| p.as_u32()),
        );
        pids.sort_unstable();
        pids.dedup();

        // cpu_usage 是「距上一次刷新该进程这段时间内」的占用，故采样间隔即观察窗口
        let kind = Self::light_kind().with_cpu();
        let mut total: f32 = 0.0;
        let mut readable = 0usize;
        for pid in pids {
            let target = Pid::from(pid as usize);
            if !self.sys.refresh_process_specifics(target, kind) {
                continue; // 已退出，或句柄打不开（受保护进程）
            }
            if !Self::counts_as_game(&self.sys, target, exe_name, exe_path, install_path) {
                continue; // 与游戏无关的残留子进程不参与合计
            }
            if let Some(proc_info) = self.sys.process(target) {
                total += proc_info.cpu_usage();
                readable += 1;
            }
        }
        (readable > 0).then_some(total)
    }

    /// 进程集合里是否有「可见的顶层窗口」（最小化也算可见：Win32 语义下 minimized ≠ hidden）
    ///
    /// 这是挂死兜底的最后一道保险：真退出后的挂起进程不会留下可见窗口，
    /// 而后台挂机/最小化的游戏一定还有。
    #[cfg(windows)]
    fn any_visible_window(pids: &[u32]) -> bool {
        use windows::core::BOOL;
        use windows::Win32::Foundation::{HWND, LPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
        };

        struct Ctx {
            pids: *const u32,
            len: usize,
            found: bool,
        }

        unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let ctx = &mut *(lparam.0 as *mut Ctx);
            let mut pid: u32 = 0;
            let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
            if pid != 0 && !ctx.found {
                let targets = std::slice::from_raw_parts(ctx.pids, ctx.len);
                if targets.contains(&pid) && IsWindowVisible(hwnd).as_bool() {
                    ctx.found = true;
                    return BOOL(0); // 命中即停止枚举
                }
            }
            BOOL(1)
        }

        let mut ctx = Ctx {
            pids: pids.as_ptr(),
            len: pids.len(),
            found: false,
        };
        unsafe {
            let _ = EnumWindows(Some(enum_proc), LPARAM(&mut ctx as *mut Ctx as isize));
        }
        ctx.found
    }

    /// 非 Windows 平台（本应用只出 Windows 便携版，仅为保持可编译）：
    /// 宁可放过，也不误杀——恒返回 true 等于挂死兜底永不触发。
    #[cfg(not(windows))]
    fn any_visible_window(_pids: &[u32]) -> bool {
        true
    }

    /// 待命会话本拍的裁决（纯函数，单测直接覆盖四种分支）
    ///
    /// `updating` 优先于 `hit`：Steam 明确报告正在更新时，安装目录里冒出的
    /// 任何进程（安装脚本、更新器、VC 运行库分发器）都不许把会话转正。
    fn decide_pending(
        ps: &PendingSession,
        now: chrono::DateTime<chrono::Utc>,
        hit: Option<u32>,
        updating: bool,
    ) -> PendingDecision {
        if updating {
            // 【2026-11-08】更新态残留（Steam 崩溃后 Updating DWORD 没清）会让
            // deadline 每拍续 60s，待命永不超时、进程表也永远不查。
            // 绝对上限的作用是「不再信任这个信号」，**不是**直接判死：
            // 超上限后回落到普通判定 —— 有进程就转正，没进程等 deadline
            // 自然超时。旧版在这里直接 Timeout，会把「更新完才启动」的
            // 整段游玩时长确定性丢掉（复审 🟠4）。
            let waited = now.signed_duration_since(ps.armed_at).num_seconds();
            if waited < crate::utils::constants::STEAM_ARM_MAX_RENEW_SECS {
                return PendingDecision::Renew;
            }
        }
        if let Some(pid) = hit {
            return PendingDecision::Activate(pid);
        }
        if now >= ps.deadline {
            return PendingDecision::Timeout;
        }
        PendingDecision::Wait
    }

    /// Steam 是否明确报告「正在更新且没在跑」（Epic 无此信号，恒 false）
    fn steam_is_updating(ps: &PendingSession) -> bool {
        ps.platform.as_deref() == Some(platform::PLATFORM_STEAM)
            && ps
                .platform_id
                .as_deref()
                .is_some_and(|id| steam_state::app_state(id).blocking_update())
    }

    /// 待命转正：起始时刻取**进程现身那一刻**
    ///
    /// 【2026-11-04 改口径】旧实现回溯到点击启动那一刻，于是 Steam 更新 / 排队 /
    /// 客户端登录 / 云同步的整段等待全被算成游玩时长；超过固定窗口又会干脆丢掉
    /// 会话，游戏更新完真跑起来反而不再计时。现在等待期一律不计入，
    /// 时长精度只受「结束检测」影响（与本地游戏同口径）。
    fn promote_pending(
        &mut self,
        game_id: &str,
        ps: &PendingSession,
        pid: u32,
        now: chrono::DateTime<chrono::Utc>,
    ) {
        self.pending_sessions.remove(game_id);
        self.active_sessions.insert(
            game_id.to_string(),
            ActiveSession {
                exe_name: ps.exe_name.clone(),
                exe_path: ps.exe_path.clone(),
                spawned_pid: Some(pid),
                install_path: ps.install_path.clone(),
                start_time: now,
                poll_cooldown_until: None,
                hang: HangWatch::default(),
                last_note: None,
            },
        );
        tracing::info!(
            "待命转正：游戏 {} 命中进程 PID {}，起始时刻 {}（等待 {} 秒未计入时长）",
            game_id,
            pid,
            now.to_rfc3339(),
            now.signed_duration_since(ps.armed_at).num_seconds().max(0)
        );
    }

    /// 处理待命会话：更新中续期、进程现身转正、超窗放弃
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
            let updating = Self::steam_is_updating(&ps);
            // 【2026-11-08 复审】Updating 信号过了绝对上限就当它不可信，
            // 恢复进程探测：Steam 崩溃后残留 DWORD=1、但游戏其实已启动时，
            // 仍能在这里转正计时，而不是被一条陈旧状态直接判超时。
            let stale_updating = updating
                && now.signed_duration_since(ps.armed_at).num_seconds()
                    >= crate::utils::constants::STEAM_ARM_MAX_RENEW_SECS;
            // 正常的更新态下不必查进程表（查到了也不许转正）；
            // 超上限后与「没有更新信号」同等对待。
            let hit = if updating && !stale_updating {
                None
            } else {
                Self::find_game_process(
                    &self.sys,
                    ps.install_path.as_deref(),
                    ps.exe_path.as_deref(),
                    Some(ps.exe_name.as_str()),
                )
            };

            match Self::decide_pending(&ps, now, hit, updating) {
                PendingDecision::Renew => {
                    if let Some(p) = self.pending_sessions.get_mut(&game_id) {
                        p.deadline = now
                            + chrono::Duration::seconds(
                                crate::utils::constants::STEAM_ARM_RENEW_SECS,
                            );
                        if !p.notified_updating {
                            p.notified_updating = true;
                            tracing::info!(
                                "Steam 正在更新游戏 {}（appid {:?}）：暂停转正、续期待命，不计入时长",
                                game_id,
                                ps.platform_id
                            );
                            tick.updating.push(game_id.clone());
                        }
                    }
                }
                PendingDecision::Activate(pid) => {
                    if ps.notified_updating {
                        tracing::info!("Steam 更新结束，游戏 {} 进程已现身，开始计时", game_id);
                    }
                    self.promote_pending(&game_id, &ps, pid, now);
                    tick.activated.push(game_id);
                }
                PendingDecision::Timeout => {
                    self.pending_sessions.remove(&game_id);
                    tracing::warn!(
                        "待命超时：游戏 {} 在窗口期内未发现进程（已取消 / 客户端未登录 / 更新后未启动），放弃计时",
                        game_id
                    );
                    tick.arm_timeouts.push(game_id);
                }
                PendingDecision::Wait => {
                    // 更新态刚结束但进程尚未现身：撤掉通知标记，
                    // 这样若再次进入更新态仍能给前端发一次状态变化。
                    // 【2026-11-08】同时通知前端把「更新中」回落到「启动中」——
                    // 旧版后端在这里静默，前端会一直显示更新中直到转正/超时（最长 60s）。
                    if ps.notified_updating {
                        if let Some(p) = self.pending_sessions.get_mut(&game_id) {
                            p.notified_updating = false;
                        }
                        tick.waiting.push(game_id.clone());
                    }
                }
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

    /// 主动停止追踪（前端「停止游戏」按钮）
    ///
    /// 只结束 Game Vault 自己的计时会话，**绝不结束游戏进程**：
    /// 需求是「点一下就停止计时」，不是替老爷关掉正在玩的游戏。
    /// 会话移除后后台轮询不会再管它，也不会自动重新计时（只有显式启动才会）。
    pub fn stop_tracking(&mut self, game_id: &str) -> StopOutcome {
        if self.active_sessions.contains_key(game_id) {
            let finished = self.stop_tracking_internal(game_id);
            self.pending_sessions.remove(game_id);
            return match finished {
                Some(f) => {
                    tracing::info!(
                        "手动停止计时：游戏 {}，时长 {} 秒",
                        game_id,
                        f.duration_seconds
                    );
                    StopOutcome::Stopped(f)
                }
                // 会话刚建立不足 1 秒：已移除，但没有可写入的时长
                None => StopOutcome::Cancelled,
            };
        }
        if self.pending_sessions.remove(game_id).is_some() {
            tracing::info!("手动取消待命：游戏 {}（原本就没有计时）", game_id);
            return StopOutcome::Cancelled;
        }
        StopOutcome::NotFound
    }

    /// 会话状态快照：前端挂载 / 重连时对齐后端真相源
    ///
    /// 后端是唯一真相源——前端不再自己「点完就当在玩」，
    /// 重启或重新挂载后从这里把三态拉回来。
    pub fn snapshot(&self) -> Vec<SessionSnapshot> {
        let mut out = Vec::with_capacity(self.active_sessions.len() + self.pending_sessions.len());
        for id in self.active_sessions.keys() {
            out.push(SessionSnapshot {
                game_id: id.clone(),
                state: "playing".to_string(),
            });
        }
        for (id, ps) in &self.pending_sessions {
            out.push(SessionSnapshot {
                game_id: id.clone(),
                state: if ps.notified_updating {
                    "updating"
                } else {
                    "launching"
                }
                .to_string(),
            });
        }
        out.sort_by(|a, b| a.game_id.cmp(&b.game_id));
        out
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
    /// 【只刷新目标进程，不做整表刷新】旧实现走 `refresh_processes()`（等价于
    /// `ProcessRefreshKind::everything()`），会遍历全系统进程表，逐个更新内存/CPU/
    /// 命令行/环境变量/工作目录——而本函数**只读 exe 一个字段**，代价与收益完全不成比例。
    /// 本函数位于截图热键路径上、且调用方持有 tracker 锁，整表刷新的耗时直接变成锁持有时间，
    /// 会与后台 10s 一次的 tick 争锁、延迟游戏退出检测。
    ///
    /// 等价性依据（逐条对照 sysinfo 0.30.13 源码）：
    /// - `everything()` 的 exe 为 `UpdateKind::OnlyIfNotSet`，此处沿用**同一口径**；
    /// - 文档明示本方法在进程尚未列出时会**将其加入**，故「刚启动、还没被任何刷新收录」的进程照样能查到；
    /// - 返回 `false` 表示进程已不存在。整表刷新此时会把它移出进程表、随后同样查不到，
    ///   故此处显式 `return None`，与旧行为结果一致。
    ///   ⚠️ 不可改用 `refresh_pids_specifics`：它**不会移除**已消失的进程（sysinfo 文档明示），
    ///   会留下陈旧条目并返回过期路径，那就真的改变了行为。
    pub fn process_path(&mut self, pid: u32) -> Option<String> {
        let target = Pid::from(pid as usize);
        if !self
            .sys
            .refresh_process_specifics(target, ProcessRefreshKind::new().with_exe(UpdateKind::OnlyIfNotSet))
        {
            return None;
        }
        self.sys
            .process(target)
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

    // ============================================================
    // 分层扫描（2026-09-19）：单 PID 点查 + 疑似退出全表确认
    // ============================================================

    /// 空状态一轮扫描不做任何事：既不该结算、也不该误判转正
    #[test]
    fn empty_tick_does_nothing() {
        let mut tracker = PlayTimeTracker::new();
        let tick = tracker.check_active_sessions();
        assert!(tick.finished.is_empty());
        assert!(tick.activated.is_empty());
        assert!(tick.arm_timeouts.is_empty());
    }

    /// 点查报死的会话：全表三级判定也全部落空 → 正常结算，时长不能为 0
    ///
    /// 覆盖新路径「阶段 1 点查到 suspects → 阶段 2 全表裁决 → 结算」。
    /// 用几乎不可能存在的 PID 与目录，保证三级判定必然全部落空。
    #[test]
    fn dead_pid_session_settles_after_full_confirmation() {
        let mut tracker = PlayTimeTracker::new();
        tracker.start_tracking(
            "gone",
            "definitely-not-running-12345.exe",
            Some("Z:\\__no_such_dir__\\definitely-not-running-12345.exe"),
            Some(0xFFFF_FFF0),
            Some("Z:\\__no_such_dir__"),
        );

        // 结算要求时长 > 0（防系统时钟回退），故至少等满 1 秒
        std::thread::sleep(std::time::Duration::from_millis(1100));

        let tick = tracker.check_active_sessions();
        assert_eq!(
            tick.finished.len(),
            1,
            "点查报死且三级全落空，应结算该会话"
        );
        assert_eq!(tick.finished[0].game_id, "gone");
        assert!(
            tick.finished[0].duration_seconds >= 1,
            "时长应至少 1 秒，实得 {}",
            tick.finished[0].duration_seconds
        );
        assert!(tracker.active_sessions.is_empty(), "结算后不应残留活跃会话");
    }

    /// 待命会话不参与「计时期」的存活判定，绝不能被点查逻辑误结算
    ///
    /// 待命期尚未产生任何时长，它的出口只有「转正」与「超时」两个。
    #[test]
    fn pending_session_never_settles() {
        let mut tracker = PlayTimeTracker::new();
        tracker.arm_session(
            "p1",
            "no-such-game-12345.exe",
            None,
            Some("Z:\\__no_such_dir__"),
        );

        std::thread::sleep(std::time::Duration::from_millis(1100));
        let tick = tracker.check_active_sessions();

        assert!(tick.finished.is_empty(), "待命会话不产生时长，绝不能结算");
        assert!(
            tick.arm_timeouts.is_empty(),
            "300 秒待命窗口内不该判超时"
        );
        assert_eq!(tracker.session_counts(), (0, 1), "待命会话应原样保留");
    }

    // ============================================================
    // 平台启动三态 / 停止计时（2026-11-04）
    // ============================================================

    fn pending_at(now: chrono::DateTime<chrono::Utc>) -> PendingSession {
        PendingSession {
            exe_name: "Game.exe".to_string(),
            exe_path: None,
            install_path: Some("Z:\\__no_such_dir__".to_string()),
            platform: None,
            platform_id: None,
            armed_at: now - chrono::Duration::seconds(120),
            deadline: now + chrono::Duration::seconds(60),
            notified_updating: false,
        }
    }

    /// 更新态优先于「进程已现身」：Steam 在更新时，安装脚本绝不能把会话转正
    #[test]
    fn decide_pending_covers_all_four_branches() {
        let now = chrono::Utc::now();
        let ps = pending_at(now);
        assert_eq!(
            PlayTimeTracker::decide_pending(&ps, now, Some(42), true),
            PendingDecision::Renew,
            "客户端正在更新 → 续期，不转正"
        );
        assert_eq!(
            PlayTimeTracker::decide_pending(&ps, now, Some(42), false),
            PendingDecision::Activate(42)
        );
        assert_eq!(
            PlayTimeTracker::decide_pending(&ps, now, None, false),
            PendingDecision::Wait
        );
        let expired = PendingSession {
            deadline: now - chrono::Duration::seconds(1),
            ..ps.clone()
        };
        assert_eq!(
            PlayTimeTracker::decide_pending(&expired, now, None, false),
            PendingDecision::Timeout
        );
    }

    /// 【2026-11-08】Updating=1 残留（Steam 崩溃后没清注册表）不能无限续期：
    /// deadline 每拍被推后 60s 看着还很早，必须靠 armed_at 的绝对上限兜底。
    #[test]
    fn stale_updating_flag_does_not_renew_forever() {
        let now = chrono::Utc::now();
        let ps = pending_at(now);
        let max = crate::utils::constants::STEAM_ARM_MAX_RENEW_SECS;

        // 刚好没到上限：仍按更新态续期
        let young = PendingSession {
            armed_at: now - chrono::Duration::seconds(max - 1),
            deadline: now + chrono::Duration::seconds(60),
            ..ps.clone()
        };
        assert_eq!(
            PlayTimeTracker::decide_pending(&young, now, Some(42), true),
            PendingDecision::Renew
        );

        // 超过上限：不再 Renew，但**不能**直接判死——游戏真被拉起要能转正
        let stale = PendingSession {
            armed_at: now - chrono::Duration::seconds(max + 1),
            deadline: now + chrono::Duration::seconds(60),
            ..ps.clone()
        };
        assert_eq!(
            PlayTimeTracker::decide_pending(&stale, now, Some(42), true),
            PendingDecision::Activate(42),
            "Updating 残留 + 进程已现身：必须转正计时，不能丢掉整段时长"
        );
        assert_eq!(
            PlayTimeTracker::decide_pending(&stale, now, None, true),
            PendingDecision::Wait,
            "无进程时继续等 deadline，不再无限续期"
        );

        // deadline 也过了、仍无进程 → 正常结算
        let expired = PendingSession {
            deadline: now - chrono::Duration::seconds(1),
            ..stale
        };
        assert_eq!(
            PlayTimeTracker::decide_pending(&expired, now, None, true),
            PendingDecision::Timeout
        );
    }

    /// 转正起点必须是「进程现身那一刻」，等待 120 秒不能算进游玩时长
    #[test]
    fn promotion_starts_at_process_appearance_not_click() {
        let mut tracker = PlayTimeTracker::new();
        let now = chrono::Utc::now();
        let ps = pending_at(now);
        tracker.pending_sessions.insert("g1".to_string(), ps.clone());

        tracker.promote_pending("g1", &ps, 4242, now);

        assert_eq!(tracker.session_counts(), (1, 0));
        assert_eq!(tracker.active_sessions["g1"].start_time, now);
        assert_ne!(tracker.active_sessions["g1"].start_time, ps.armed_at);
        assert_eq!(tracker.active_sessions["g1"].spawned_pid, Some(4242));
    }

    /// 安装 / 更新 / 崩溃上报进程不得被当成游戏本体；真正的启动器壳要放行
    #[test]
    fn setup_processes_never_promote_a_pending_session() {
        for name in [
            "VC_redist.x64.exe",
            "DXSETUP.exe",
            "CrashReportClient.exe",
            "SteamWebHelper.exe",
            "GameUpdater.exe",
            "unins000.exe",
            "EasyAntiCheat_EOS.exe",
        ] {
            assert!(
                PlayTimeTracker::is_non_game_process(name),
                "{} 不该被当成游戏",
                name
            );
        }
        for name in ["PBZ.exe", "Launcher.exe", "PlayRDR2.exe", "Hellraiser-Win64-Shipping.exe"] {
            assert!(
                !PlayTimeTracker::is_non_game_process(name),
                "{} 是可能被真正追踪的游戏进程，不得误杀",
                name
            );
        }
    }

    /// 【2026-11-08 回归护栏】结算侧（evaluate_session 策略 1/2、counts_as_game、
    /// sample_tree_cpu_usage）必须与待命转正共用同一份闸门。
    ///
    /// 旧版结算侧只用 `is_helper_process`（仅 4 个崩溃上报器前缀），而 classify
    /// 对 install_path 内的进程恒判 Game —— 游戏退出后残留的更新器/安装器会被
    /// 算进可信进程树，会话永远不结算（2026-10-20 幽灵会话的同类病灶）。
    /// 本测试钉死「旧名单不够用」，防止有人把结算侧改回窄闸门。
    #[test]
    fn settlement_gate_must_include_setup_helpers() {
        // 旧名单仍要保留（不能把两者改成互斥）
        assert!(PlayTimeTracker::is_helper_process("CrashReportClient.exe"));
        for name in [
            "VC_redist.x64.exe",
            "DXSETUP.exe",
            "GameUpdater.exe",
            "unins000.exe",
            "EasyAntiCheat_EOS.exe",
        ] {
            assert!(
                !PlayTimeTracker::is_helper_process(name),
                "{name} 本就不在旧的 sibling 名单里"
            );
            assert!(
                PlayTimeTracker::is_non_game_process(name),
                "{name} 必须被结算侧排除，否则残留进程会顶住已结束的会话"
            );
        }
    }

    /// 手动停止：正式会话要结算、待命会话直接取消、无关 id 不影响其他会话
    #[test]
    fn stop_tracking_settles_active_and_cancels_pending() {
        let mut tracker = PlayTimeTracker::new();
        tracker.start_tracking("g1", "Game.exe", None, Some(std::process::id()), None);
        std::thread::sleep(std::time::Duration::from_millis(1100));

        let outcome = tracker.stop_tracking("g1");
        assert!(
            matches!(outcome, StopOutcome::Stopped(ref f) if f.duration_seconds >= 1),
            "停止计时应结算出一条 >=1 秒的会话，实得 {:?}",
            outcome
        );
        assert_eq!(tracker.session_counts(), (0, 0));

        tracker.arm_session("g2", "Game.exe", None, Some("Z:\\__no_such_dir__"));
        assert_eq!(tracker.stop_tracking("g2"), StopOutcome::Cancelled);
        assert_eq!(tracker.stop_tracking("not-there"), StopOutcome::NotFound);
        assert!(!tracker.has_pending_work());
    }

    /// 快照把后端三态交给前端，重启 / 重挂载后不会自己「猜」状态
    #[test]
    fn snapshot_reports_playing_launching_and_updating() {
        let mut tracker = PlayTimeTracker::new();
        tracker.start_tracking("a", "A.exe", None, Some(std::process::id()), None);
        tracker.arm_session("b", "B.exe", None, Some("Z:\\__no_such_dir__"));
        tracker.arm_session("c", "C.exe", None, Some("Z:\\__no_such_dir__"));
        if let Some(ps) = tracker.pending_sessions.get_mut("c") {
            ps.notified_updating = true;
        }

        assert_eq!(
            tracker.snapshot(),
            vec![
                SessionSnapshot { game_id: "a".into(), state: "playing".into() },
                SessionSnapshot { game_id: "b".into(), state: "launching".into() },
                SessionSnapshot { game_id: "c".into(), state: "updating".into() },
            ]
        );
    }

    /// 冷却期内的会话跳过点查（避免在「句柄打不开」的会话上每拍全表刷新）
    #[test]
    fn cooldown_skips_point_query() {
        let mut tracker = PlayTimeTracker::new();
        tracker.start_tracking(
            "cd1",
            "no-such-game-12345.exe",
            None,
            Some(0xFFFF_FFF0),
            Some("Z:\\__no_such_dir__"),
        );
        // 手工打上冷却标记（模拟「点查报死、全表说活」之后的状态）
        if let Some(session) = tracker.active_sessions.get_mut("cd1") {
            session.poll_cooldown_until =
                Some(std::time::Instant::now() + std::time::Duration::from_secs(60));
        }
        std::thread::sleep(std::time::Duration::from_millis(1100));

        let tick = tracker.check_active_sessions();
        assert!(
            tick.finished.is_empty(),
            "冷却期内应跳过点查，不得据此结算"
        );
        assert_eq!(tracker.session_counts(), (1, 0), "会话应仍在计时");
    }

    // ============================================================
    // 身份闸门与接棒锚定（2026-10-20 幽灵会话修复）
    //
    // 现场：《猛鬼追魂》退出后，引导壳 PID 已消失，树里还挂着 4 个身份不明的进程，
    // 旧口径「有活子孙 = 游戏在跑」把会话顶了 52 分钟，UI 一直显示「游玩中」。
    // ============================================================

    /// 身份判定的四条边界：安装目录内算本体、路径对不上算外人、读不到路径不下结论、无签名沿用旧口径
    #[test]
    fn classify_accepts_game_and_rejects_foreign_paths() {
        let exe_path = Some("I:\\Clive Barker's Hellraiser Revival\\Hellraiser.exe");
        let install = Some("I:\\Clive Barker's Hellraiser Revival");

        // 真身（shipping）落在安装目录下 → 游戏本体
        assert_eq!(
            PlayTimeTracker::classify(
                Some("i:\\clive barker's hellraiser revival\\hellraiser\\binaries\\win64\\hellraiser-win64-shipping.exe"),
                "Hellraiser-Win64-Shipping.exe",
                "Hellraiser.exe",
                exe_path,
                install,
            ),
            ProcIdentity::Game
        );
        // 系统进程：路径读得到，三条签名全不命中 → 确证不是游戏（PID 被复用的那道防线）
        assert_eq!(
            PlayTimeTracker::classify(
                Some("c:\\windows\\system32\\svchost.exe"),
                "svchost.exe",
                "Hellraiser.exe",
                exe_path,
                install,
            ),
            ProcIdentity::Foreign
        );
        // 读不到路径（受保护进程 / 32-bit 老游戏）→ 不可证伪，不得据以结算
        assert_eq!(
            PlayTimeTracker::classify(None, "protected.exe", "Hellraiser.exe", exe_path, install),
            ProcIdentity::Unknown
        );
        // 一个签名都没有的历史脏数据 → 沿用旧口径，不因"看起来不像"而误杀
        assert_eq!(
            PlayTimeTracker::classify(
                Some("c:\\windows\\system32\\svchost.exe"),
                "svchost.exe",
                "",
                None,
                None,
            ),
            ProcIdentity::Unknown
        );
        // 启动器壳：路径在别处，但进程名与库内 exe_name 一致 → 放行
        assert_eq!(
            PlayTimeTracker::classify(
                Some("c:\\program files\\rockstar games\\launcher.exe"),
                "Launcher.exe",
                "Launcher.exe",
                None,
                Some("K:\\Games\\RDR2"),
            ),
            ProcIdentity::Game
        );
    }

    /// 伴随进程名单：UE 崩溃上报器躺在安装目录里，路径前缀拦不住，只能靠名字拦
    #[test]
    fn helper_processes_never_count_as_game() {
        assert!(PlayTimeTracker::is_helper_process("CrashReportClient.exe"));
        assert!(PlayTimeTracker::is_helper_process("CRASHREPORTCLIENT-Win64-Shipping.exe"));
        assert!(PlayTimeTracker::is_helper_process("EOSOverlayRenderer-Win64-Shipping.exe"));
        // 真游戏不能被误伤（同前缀不算同名）
        assert!(!PlayTimeTracker::is_helper_process("Hellraiser-Win64-Shipping.exe"));
        assert!(!PlayTimeTracker::is_helper_process("CrashBandicoot.exe"));
    }

    /// 接棒候选：同干优先（Hellraiser.exe → Hellraiser-Win64-Shipping.exe），否则取最小 PID
    #[test]
    fn pick_anchor_prefers_same_stem_then_lowest_pid() {
        let candidates = vec![
            (
                900u32,
                "i:\\game\\engine\\binaries\\win64\\other.exe".to_string(),
                "other.exe".to_string(),
            ),
            (
                412u32,
                "i:\\game\\hellraiser\\binaries\\win64\\hellraiser-win64-shipping.exe".to_string(),
                "Hellraiser-Win64-Shipping.exe".to_string(),
            ),
        ];
        assert_eq!(
            PlayTimeTracker::pick_anchor(&candidates, "Hellraiser.exe"),
            Some(1),
            "同干进程优先于更早的无关进程"
        );

        let unrelated = vec![
            (900u32, "i:\\game\\a.exe".to_string(), "a.exe".to_string()),
            (412u32, "i:\\game\\b.exe".to_string(), "b.exe".to_string()),
        ];
        assert_eq!(
            PlayTimeTracker::pick_anchor(&unrelated, "Hellraiser.exe"),
            Some(1),
            "无同干候选时取 PID 最小者，结果必须确定"
        );
        assert_eq!(PlayTimeTracker::pick_anchor(&[], "Hellraiser.exe"), None);
    }

    /// 挂死观察不该在会话刚建立时就下结论（首个采样点没有历史可比）
    #[test]
    fn hang_watch_never_settles_within_one_tick() {
        let mut tracker = PlayTimeTracker::new();
        tracker.start_tracking(
            "fresh",
            "no-such-game-12345.exe",
            None,
            Some(std::process::id()),
            Some("Z:\\__no_such_dir__"),
        );

        let tick = tracker.check_active_sessions();
        assert!(
            tick.finished.is_empty(),
            "一分钟采样一次、十分钟才判死，单拍绝不可能结算"
        );
        assert_eq!(tracker.session_counts(), (1, 0));
        // 采样点已推进，不会每拍都做全表刷新
        let next = tracker
            .active_sessions
            .get("fresh")
            .and_then(|s| s.hang.next_sample_at);
        assert!(next.is_some(), "采样后必须排下一次采样时刻");
    }

    /// 【真机演练，默认忽略】复刻 2026-10-20 幽灵会话：根进程已死，子孙还在但**不是游戏**
    ///
    /// 现场形态：`Hellraiser.exe`（引导壳）退出后，树里残留 4 个身份不明的进程，
    /// 旧口径据此判"仍在游玩"，多记 52 分钟。这里用 `ping.exe` 冒充那批残留进程：
    /// 它的 exe 路径与库内安装目录/可执行文件全对不上 → 必须判 Foreign → 结算。
    ///
    /// 手动跑：`cargo test --lib -- --ignored foreign_orphan`
    #[test]
    #[ignore]
    fn foreign_orphan_does_not_keep_session_alive() {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let mut parent = std::process::Command::new("cmd")
            .args(["/c", "start", "/b", "ping", "-n", "20", "127.0.0.1"])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .expect("spawn 父进程失败");
        let parent_pid = parent.id();
        let _ = parent.wait();
        std::thread::sleep(std::time::Duration::from_millis(800));

        // 三项签名齐全（exe 名 / 完整路径 / 安装目录），全部指向一个不存在的"游戏"：
        // 存活的 ping.exe 三条全不命中 → 可证不是本体 → 会话必须结算。
        let mut tracker = PlayTimeTracker::new();
        tracker.start_tracking(
            "ghost",
            "GhostGame.exe",
            Some("Z:\\__no_such_dir__\\GhostGame.exe"),
            Some(parent_pid),
            Some("Z:\\__no_such_dir__"),
        );
        std::thread::sleep(std::time::Duration::from_millis(1100));

        let tick = tracker.check_active_sessions();
        assert_eq!(
            tick.finished.len(),
            1,
            "树里只剩与游戏身份无关的残留进程，不得继续计时（这正是幽灵会话的成因）"
        );
        assert_eq!(tick.finished[0].game_id, "ghost");
        assert!(tracker.active_sessions.is_empty());
    }

    /// 【真机演练，默认忽略】父退子活：父进程已退出但子孙仍在运行 → 不得结算，且应接棒
    ///
    /// 这是分层扫描里最要紧的正确性约束：单 PID 点查只能证明「根 PID 没了」，
    /// 证明不了「游戏没了」。单测无法合成进程树，只能真起一对父子进程。
    /// 标 `#[ignore]` 是因为它会拉起真实进程（无窗口），不该混进常规测试。
    ///
    /// 手动跑：`cargo test --lib -- --ignored parent_exit_child_alive`
    #[test]
    #[ignore]
    fn parent_exit_child_alive_keeps_session() {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        // cmd 起一个分离的 ping（约 20 秒）后自己立刻退出 → 父子关系成立
        let mut parent = std::process::Command::new("cmd")
            .args(["/c", "start", "/b", "ping", "-n", "20", "127.0.0.1"])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .expect("spawn 父进程失败");
        let parent_pid = parent.id();
        let _ = parent.wait();
        std::thread::sleep(std::time::Duration::from_millis(800));

        // 只给 PID：exe_name 为空、install_path 为 None → 策略 2/3 无从下手，
        // 命中的唯一可能是策略 1（进程树），验证才精确。
        let mut tracker = PlayTimeTracker::new();
        tracker.start_tracking("tree1", "", None, Some(parent_pid), None);
        std::thread::sleep(std::time::Duration::from_millis(1100));

        let tick = tracker.check_active_sessions();
        assert!(
            tick.finished.is_empty(),
            "父进程已退出但子孙仍在运行，不得结算（父退子活）"
        );
        assert_eq!(tracker.session_counts(), (1, 0), "会话应仍在计时");
        assert_ne!(
            tracker
                .active_sessions
                .get("tree1")
                .and_then(|s| s.spawned_pid),
            Some(parent_pid),
            "根 PID 已消失，会话应接棒到存活子孙上（否则每拍都要全表重判）"
        );
    }
}
