use anyhow::Result;
use std::collections::HashMap;
use crate::models::*;
use crate::core::Database;

/// 成就引擎：定义 + 检测 + 汇总
pub struct AchievementEngine;

/// 目标值单位为「秒」的成就基础 ID。
///
/// 其余成就的 target 一律是计数（款 / 次 / 天 / 级），前端按纯数字展示。
/// 这份白名单是**唯一**的量纲真相源：新增时间型成就时必须把 base_id 加进来，
/// 否则前端会把「3 小时」显示成「10800 / 10800」。
/// 宁可显式列举也不按数值猜：P-08/P-09 的 HLTB 参考时长可能只有 600 秒
/// （比 G-18 的 500 次、G-39 的 365 天还小），数值区间根本不具判别力。
const SECONDS_TARGET_BASES: &[&str] = &[
    // 全局：累计时长 / 单次会话 / 周末 / 单日
    "G-04", "G-05", "G-15", "G-19", "G-30", "G-32",
    // 单游戏：累计时长 / HLTB 参考时长 / 单次或单日时长
    "P-02", "P-03", "P-04", "P-05", "P-08", "P-09", "P-10", "P-12", "P-16",
];

impl AchievementEngine {
    /// 生成一条多级成就的展平定义列表
    fn tiered(
        base: &str,
        name: &str,
        scope: &str,
        category: &str,
        desc: &str,
        icon: &str,
        targets: &[u64],
    ) -> Vec<AchievementDef> {
        let total = targets.len() as u32;
        let id_prefix = base.to_lowercase().replace('-', "");
        // 量纲由定义白名单决定，不随 target 数值浮动（同一成就的多级也必然同量纲）
        let unit = if SECONDS_TARGET_BASES.contains(&base) {
            "seconds"
        } else {
            "count"
        }
        .to_string();
        targets
            .iter()
            .enumerate()
            .map(|(i, target)| AchievementDef {
                id: format!("{}_{}", id_prefix, i + 1),
                base_id: base.to_string(),
                name: name.to_string(),
                scope: scope.to_string(),
                category: category.to_string(),
                desc: desc.to_string(),
                icon: icon.to_string(),
                tier: (i + 1) as u32,
                tier_total: total,
                target: *target,
                unit: unit.clone(),
            })
            .collect()
    }

    fn single(
        base: &str,
        name: &str,
        scope: &str,
        category: &str,
        desc: &str,
        icon: &str,
        target: u64,
    ) -> Vec<AchievementDef> {
        Self::tiered(base, name, scope, category, desc, icon, &[target])
    }

    /// 全部成就定义（展平，多级成就拆分为多条）
    pub fn definitions() -> Vec<AchievementDef> {
        let mut defs = Vec::new();

        // ==================== 全局成就 ====================
        defs.extend(Self::single("G-01", "初来乍到", "global", "collect", "向游戏库添加第一款游戏", "🎯", 1));
        defs.extend(Self::tiered("G-02", "藏书家", "global", "collect", "库中拥有 10 / 50 / 100 / 250 款游戏", "📚", &[10, 50, 100, 250]));
        defs.extend(Self::single("G-03", "首战告捷", "global", "progress", "首次启动任意一款游戏", "🚀", 1));
        defs.extend(Self::tiered("G-04", "废寝忘食", "global", "progress", "累计游玩达 1 / 3 / 10 天", "⏳", &[86400, 259200, 864000]));
        defs.extend(Self::single("G-05", "游戏人生", "global", "challenge", "累计游玩突破 1000 小时", "👑", 3_600_000));
        defs.extend(Self::single("G-06", "夜行者", "global", "fun", "在 0:00–5:00 之间游玩过任意游戏", "🌙", 1));
        defs.extend(Self::single("G-07", "破晓党", "global", "fun", "在 5:00–8:00 之间游玩过任意游戏（不是没睡，是起得早）", "🌅", 1));
        defs.extend(Self::tiered("G-08", "铁杆玩家", "global", "challenge", "连续 3 / 7 / 14 / 30 天，每天都有游玩记录", "🔥", &[3, 7, 14, 30]));
        defs.extend(Self::single("G-09", "一日五游", "global", "challenge", "同一天内启动 5 款不同的游戏", "🎪", 5));
        defs.extend(Self::tiered("G-10", "通关之路", "global", "progress", "标记通关 1 / 5 / 15 款游戏", "🏁", &[1, 5, 15]));
        defs.extend(Self::tiered("G-11", "收藏瘾", "global", "collect", "收藏 1 / 10 / 30 款游戏", "⭐", &[1, 10, 30]));
        defs.extend(Self::tiered("G-12", "美术总监", "global", "collect", "为 10 / 50 款游戏获取封面", "🖼️", &[10, 50]));
        defs.extend(Self::single("G-13", "资料管理员", "global", "collect", "用 LLM 补全 10 款游戏的元数据", "🧾", 10));
        defs.extend(Self::tiered("G-14", "存档狂魔", "global", "collect", "为 10 / 30 款游戏配置存档路径", "💾", &[10, 30]));
        defs.extend(Self::single("G-15", "马拉松选手", "global", "challenge", "单次游玩会话 ≥ 6 小时", "🏃", 21600));
        defs.extend(Self::single("G-16", "浪子回头", "global", "fun", "重新游玩一款已标记「弃坑」的游戏", "🔁", 1));
        defs.extend(Self::tiered("G-17", "完美主义者", "global", "challenge", "3 / 10 款游戏时长达到 HLTB 完美通关参考时长", "💎", &[3, 10]));
        defs.extend(Self::single("G-18", "老江湖", "global", "progress", "累计启动游戏 500 次", "🎩", 500));
        defs.extend(Self::single("G-19", "周末战士", "global", "fun", "周末（六/日）累计游玩 ≥ 10 小时", "🛋️", 36000));

        // ==================== 第二批全局成就（反向/收集/肝/作息/长情） ====================
        defs.extend(Self::single("G-20", "出去走走", "global", "fun", "连续 30 天没打开任何游戏（世界很精彩）", "🚪", 30));
        defs.extend(Self::single("G-21", "积灰如山", "global", "fun", "库中 20 款游戏从未启动（Backlog of Shame）", "🕸️", 20));
        defs.extend(Self::single("G-22", "三分钟热度", "global", "fun", "5 款游戏都玩过、但每款不足 1 小时", "🔥", 5));
        defs.extend(Self::single("G-24", "摸鱼大师", "global", "fun", "某款游戏时长已超 HLTB 主线参考，却仍标「未通关」", "🐟", 1));
        defs.extend(Self::single("G-25", "博古通今", "global", "collect", "库中 5 款发行于 20 年前的老游戏", "🏺", 5));
        defs.extend(Self::single("G-26", "类型图鉴", "global", "collect", "覆盖 10 种不同游戏类型", "🧩", 10));
        defs.extend(Self::single("G-27", "厂商死忠", "global", "collect", "同一开发商拥有 5 款游戏", "🏭", 5));
        defs.extend(Self::single("G-29", "全员启动", "global", "challenge", "库中每款游戏都启动过至少一次", "🎯", 1));
        defs.extend(Self::single("G-30", "肝帝一日", "global", "challenge", "单日游玩达 12 小时", "🌋", 43200));
        defs.extend(Self::single("G-31", "百小时俱乐部", "global", "challenge", "3 款游戏各累计 100 小时以上", "💯", 3));
        defs.extend(Self::single("G-32", "周末王者", "global", "challenge", "单个周末（六日合计）游玩达 20 小时", "🛋️", 72000));
        defs.extend(Self::single("G-33", "雨露均沾", "global", "challenge", "一周内玩过 10 款不同游戏", "🌧️", 10));
        defs.extend(Self::single("G-34", "百炼成钢", "global", "challenge", "连续 100 天每天都有游玩记录", "🥇", 100));
        defs.extend(Self::single("G-35", "连续夜猫", "global", "fun", "连续 7 天在 0:00–5:00 有游玩记录", "🦉", 7));
        defs.extend(Self::single("G-36", "昼夜颠倒", "global", "fun", "同一天既在凌晨又在白天玩过游戏", "🔄", 1));
        defs.extend(Self::single("G-37", "全勤奖", "global", "fun", "一周 7 天，天天都有游玩记录", "📅", 1));
        defs.extend(Self::single("G-38", "久别重逢", "global", "fun", "重玩一款超过 180 天没启动的游戏", "🤝", 1));
        defs.extend(Self::single("G-39", "养老玩家", "global", "progress", "游戏库建立满 1 年", "🧓", 365));
        defs.extend(Self::tiered("G-40", "手账作家", "global", "collect", "为 5 / 15 / 30 款游戏写下评价（游戏手账）", "✍️", &[5, 15, 30]));

        // ==================== 单游戏成就（每款游戏独立结算） ====================
        defs.extend(Self::single("P-01", "破冰", "pergame", "progress", "首次启动这款游戏", "🧊", 1));
        defs.extend(Self::single("P-02", "浅尝辄止", "pergame", "progress", "该游戏累计游玩 1 小时", "☕", 3600));
        defs.extend(Self::single("P-03", "渐入佳境", "pergame", "progress", "该游戏累计游玩 10 小时", "🎮", 36000));
        defs.extend(Self::single("P-04", "资深玩家", "pergame", "progress", "该游戏累计游玩 50 小时", "🕹️", 180000));
        defs.extend(Self::single("P-05", "骨灰级", "pergame", "progress", "该游戏累计游玩 100 小时", "💀", 360000));
        defs.extend(Self::tiered("P-06", "常客", "pergame", "progress", "该游戏累计启动 10 / 50 次", "🪑", &[10, 50]));
        defs.extend(Self::single("P-07", "通关者", "pergame", "progress", "将该游戏标记为「已通关」", "🏆", 1));
        defs.extend(Self::single("P-08", "主线之旅", "pergame", "challenge", "游玩时长达到 HLTB 主线参考时长", "🗺️", 0));
        defs.extend(Self::single("P-09", "完美通关", "pergame", "challenge", "游玩时长达到 HLTB 完美通关参考时长", "💯", 0));
        defs.extend(Self::single("P-10", "马拉松", "pergame", "challenge", "该游戏单次会话 ≥ 4 小时", "🏅", 14400));
        defs.extend(Self::single("P-11", "夜猫子", "pergame", "fun", "在 0:00–5:00 游玩过该游戏", "🦉", 1));
        defs.extend(Self::single("P-12", "沉迷一日", "pergame", "challenge", "同一天内该游戏游玩 ≥ 5 小时", "🌀", 18000));
        defs.extend(Self::single("P-13", "老友记", "pergame", "fun", "在 ≥ 10 个不同的日期游玩过该游戏", "🤝", 10));
        defs.extend(Self::single("P-14", "回头客", "pergame", "fun", "该游戏被标记「弃坑」后重新游玩", "↩️", 1));
        defs.extend(Self::single("P-15", "三日之约", "pergame", "challenge", "连续 3 天游玩该游戏", "📆", 3));
        defs.extend(Self::single("P-16", "从一而终", "pergame", "progress", "该游戏累计游玩 500 小时", "💘", 1_800_000));

        defs
    }

    /// 评估全局成就
    fn eval_global(def: &AchievementDef, s: &AchievementGlobalStats) -> (u64, bool) {
        let (progress, satisfied) = match def.base_id.as_str() {
            "G-01" | "G-02" => (s.game_count, s.game_count >= def.target),
            "G-03" | "G-18" => (s.total_sessions, s.total_sessions >= def.target),
            "G-04" | "G-05" => (s.total_play_time, s.total_play_time >= def.target),
            "G-06" => (s.night_session as u64, s.night_session),
            "G-07" => (s.dawn_session as u64, s.dawn_session),
            "G-08" => (s.longest_streak, s.longest_streak >= def.target),
            "G-09" => (s.max_distinct_games_per_day, s.max_distinct_games_per_day >= def.target),
            "G-10" => (s.completed_count, s.completed_count >= def.target),
            "G-11" => (s.favorite_count, s.favorite_count >= def.target),
            "G-12" => (s.cover_count, s.cover_count >= def.target),
            "G-13" => (s.llm_filled_count, s.llm_filled_count >= def.target),
            "G-14" => (s.save_paths_count, s.save_paths_count >= def.target),
            "G-15" => (s.max_session_duration, s.max_session_duration >= def.target),
            "G-16" => (s.abandoned_replayed, s.abandoned_replayed >= def.target),
            "G-17" => (s.hltb_completed_count, s.hltb_completed_count >= def.target),
            "G-19" => (s.weekend_total, s.weekend_total >= def.target),
            "G-20" => (s.days_since_last_play, s.total_sessions > 0 && s.days_since_last_play >= def.target),
            "G-21" => (s.unplayed_count, s.unplayed_count >= def.target),
            "G-22" => (s.played_under_1h_count, s.played_under_1h_count >= def.target),
            "G-24" => (s.over_main_not_completed_count, s.over_main_not_completed_count >= def.target),
            "G-25" => (s.old_game_count, s.old_game_count >= def.target),
            "G-26" => (s.distinct_genre_count, s.distinct_genre_count >= def.target),
            "G-27" => (s.max_dev_count, s.max_dev_count >= def.target),
            "G-29" => {
                let done = s.game_count > 0 && s.unplayed_count == 0;
                (done as u64, done)
            }
            "G-30" => (s.max_day_seconds, s.max_day_seconds >= def.target),
            "G-31" => (s.games_over_100h_count, s.games_over_100h_count >= def.target),
            "G-32" => (s.max_weekend_total, s.max_weekend_total >= def.target),
            "G-33" => (s.max_distinct_games_per_week, s.max_distinct_games_per_week >= def.target),
            "G-34" => (s.longest_streak, s.longest_streak >= def.target),
            "G-35" => (s.night_streak, s.night_streak >= def.target),
            "G-36" => (s.day_night_same_day as u64, s.day_night_same_day),
            "G-37" => (s.has_full_week as u64, s.has_full_week),
            "G-38" => (s.long_gap_replay as u64, s.long_gap_replay),
            "G-39" => (s.days_since_first_add, s.days_since_first_add >= def.target),
            "G-40" => (s.review_written_count, s.review_written_count >= def.target),
            _ => (0, false),
        };
        (progress, satisfied)
    }

    /// 评估单游戏成就
    ///
    /// 已删除留档条目（`removed`）不参与结算：它已不入库，`play_time_seconds` 等是从
    /// 每日汇总回填的展示值，不能反过来当作「时长达标」去补发新成就（否则重装认领时
    /// 会凭空多出一批解锁）。它**已经解锁**的记录仍由 `get_summary` 照旧读出展示。
    fn eval_per_game(def: &AchievementDef, gs: &PerGameStats) -> (u64, bool) {
        if gs.removed {
            return (0, false);
        }
        let (progress, satisfied) = match def.base_id.as_str() {
            "P-01" => (gs.sessions_count, gs.sessions_count >= 1),
            "P-02" | "P-03" | "P-04" | "P-05" | "P-16" => (gs.play_time_seconds, gs.play_time_seconds >= def.target),
            "P-06" => (gs.play_count, gs.play_count >= def.target),
            "P-07" => {
                let completed = gs.status == "completed";
                (completed as u64, completed)
            }
            "P-08" => {
                let target = gs.hltb_main_story.map(|m| m * 60).unwrap_or(u64::MAX);
                (gs.play_time_seconds, gs.hltb_main_story.is_some() && gs.play_time_seconds >= target)
            }
            "P-09" => {
                let target = gs.hltb_completionist.map(|m| m * 60).unwrap_or(u64::MAX);
                (gs.play_time_seconds, gs.hltb_completionist.is_some() && gs.play_time_seconds >= target)
            }
            "P-10" => (gs.max_session_duration, gs.max_session_duration >= def.target),
            "P-11" => (gs.night_session as u64, gs.night_session),
            "P-12" => (gs.max_day_seconds, gs.max_day_seconds >= def.target),
            "P-13" => (gs.distinct_days, gs.distinct_days >= def.target),
            "P-14" => (gs.replayed_after_abandon as u64, gs.replayed_after_abandon),
            "P-15" => (gs.longest_streak, gs.longest_streak >= def.target),
            _ => (0, false),
        };
        (progress, satisfied)
    }

    /// 检测并解锁所有满足条件的成就，返回本次新解锁的事件列表
    /// 存量数据在此立即结算（首次调用即回溯解锁历史数据）
    pub fn evaluate(db: &Database) -> Result<Vec<UnlockEvent>> {
        let global_stats = db.get_achievement_global_stats()?;
        let per_game_stats = db.get_per_game_achievement_stats()?;
        let defs = Self::definitions();
        let now = chrono::Utc::now().to_rfc3339();
        let mut events = Vec::new();

        // 先取已解锁集合做内存过滤：绝大多数成就早已解锁，
        // 避免每次评估对 73 + 16×N 条目逐条发 INSERT OR IGNORE（每条都是一个写事务）
        let unlocked_set: std::collections::HashSet<(String, String)> = db
            .get_achievement_unlocks()?
            .into_iter()
            .map(|u| (u.achievement_id, u.game_id))
            .collect();

        // 全局成就
        for def in defs.iter().filter(|d| d.scope == "global") {
            let (_, satisfied) = Self::eval_global(def, &global_stats);
            if satisfied
                && !unlocked_set.contains(&(def.id.clone(), String::new()))
                && db.try_unlock_achievement(&def.id, "", &now)?
            {
                events.push(UnlockEvent {
                    def: def.clone(),
                    game_id: None,
                });
            }
        }

        // 单游戏成就（每个游戏独立评估）
        for gs in &per_game_stats {
            for def in defs.iter().filter(|d| d.scope == "pergame") {
                let (_, satisfied) = Self::eval_per_game(def, gs);
                if satisfied
                    && !unlocked_set.contains(&(def.id.clone(), gs.game_id.clone()))
                    && db.try_unlock_achievement(&def.id, &gs.game_id, &now)?
                {
                    events.push(UnlockEvent {
                        def: def.clone(),
                        game_id: Some(gs.game_id.clone()),
                    });
                }
            }
        }

        if !events.is_empty() {
            tracing::info!("成就系统：本次解锁 {} 条新成就", events.len());
        }
        Ok(events)
    }

    /// 组装成就汇总（成就页一次性拉取）
    pub fn get_summary(db: &Database) -> Result<AchievementSummary> {
        let global_stats = db.get_achievement_global_stats()?;
        let per_game_stats = db.get_per_game_achievement_stats()?;
        let unlocks = db.get_achievement_unlocks()?;
        let defs = Self::definitions();

        // 解锁记录索引
        let mut global_unlocked: HashMap<String, String> = HashMap::new();
        let mut per_game_unlocked: HashMap<String, HashMap<String, String>> = HashMap::new();
        for u in &unlocks {
            if u.game_id.is_empty() {
                global_unlocked.insert(u.achievement_id.clone(), u.unlocked_at.clone());
            } else {
                per_game_unlocked
                    .entry(u.game_id.clone())
                    .or_default()
                    .insert(u.achievement_id.clone(), u.unlocked_at.clone());
            }
        }

        // 全局成就状态
        let mut global = Vec::new();
        for def in defs.iter().filter(|d| d.scope == "global") {
            let (progress, _) = Self::eval_global(def, &global_stats);
            global.push(GlobalAchievementStatus {
                def: def.clone(),
                unlocked: global_unlocked.contains_key(&def.id),
                unlocked_at: global_unlocked.get(&def.id).cloned(),
                progress,
                target: def.target,
            });
        }

        // 单游戏成就状态（按游戏分组）
        let mut per_game = Vec::new();
        for gs in &per_game_stats {
            let game_unlocks = per_game_unlocked.get(&gs.game_id).cloned().unwrap_or_default();
            let mut achievements = Vec::new();
            for def in defs.iter().filter(|d| d.scope == "pergame") {
                let (progress, _) = Self::eval_per_game(def, gs);
                // 留档条目（removed）不参与结算（见 `eval_per_game` 早退），它那两条依赖
                // HLTB 的成就也必须报 target = 0，否则前端按「target > 0 即可达成」计入分母，
                // 而它们永远解不开 —— 留档卡片的完成度就会永远停在天花板（旧口径 88%）。
                //
                // 【只清 P-08/P-09，其余 14 条 def.target 必须原样】：
                // 若一并清零，前端分母会塌成「已解锁条目数」，留档卡片直接跳 100%。
                let target = if gs.removed {
                    match def.base_id.as_str() {
                        "P-08" | "P-09" => 0,
                        _ => def.target,
                    }
                } else {
                    match def.base_id.as_str() {
                        "P-08" => gs.hltb_main_story.map(|m| m * 60).unwrap_or(0),
                        "P-09" => gs.hltb_completionist.map(|m| m * 60).unwrap_or(0),
                        _ => def.target,
                    }
                };
                achievements.push(GameAchievementStatus {
                    def: def.clone(),
                    unlocked: game_unlocks.contains_key(&def.id),
                    unlocked_at: game_unlocks.get(&def.id).cloned(),
                    progress,
                    target,
                });
            }
            per_game.push(GameAchievements {
                game_id: gs.game_id.clone(),
                game_name: gs.game_name.clone(),
                removed: gs.removed,
                achievements,
            });
        }

        // 已解锁数（去重口径，2026-10-12 拍板）：数的是**至少解锁过一次的成就定义条目**——
        // 每款游戏的同名成就（如各游戏各自的「破冰」）只计一次，与 total_count（展平定义
        // 条目数）同单位，百分比恒 ≤ 100%。
        // 为什么改：旧口径把「解锁记录条数」当分子（每款游戏各一份记录，随游戏数膨胀），
        // 单游戏解锁攒过 72 − 全局解锁数后就会显示破百（实机 84/72 = 117%）。
        // 只采信**仍在册**的定义：已删定义（如 G-23/G-28）留下的历史解锁行不计入。
        let mut unlocked_def_ids: std::collections::HashSet<&str> =
            std::collections::HashSet::new();
        for id in global_unlocked.keys() {
            unlocked_def_ids.insert(id.as_str());
        }
        for unlocks_of_game in per_game_unlocked.values() {
            for id in unlocks_of_game.keys() {
                unlocked_def_ids.insert(id.as_str());
            }
        }
        let unlocked_count = defs
            .iter()
            .filter(|d| unlocked_def_ids.contains(d.id.as_str()))
            .count() as u32;

        Ok(AchievementSummary {
            global,
            per_game,
            total_count: defs.len() as u32,
            unlocked_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_are_valid() {
        let defs = AchievementEngine::definitions();
        assert!(!defs.is_empty());

        // ID 必须唯一（数据库 UNIQUE 约束依赖此点）
        let mut ids = std::collections::HashSet::new();
        for def in &defs {
            assert!(ids.insert(def.id.clone()), "重复成就 ID: {}", def.id);
            assert!(def.target > 0 || def.base_id == "P-08" || def.base_id == "P-09",
                "成就 {} 的目标值无效", def.base_id);
        }

        // 数量与设计一致：全局 38 条基础 + 单游戏 16 条基础
        let global_bases: std::collections::HashSet<&str> =
            defs.iter().filter(|d| d.scope == "global").map(|d| d.base_id.as_str()).collect();
        let pergame_bases: std::collections::HashSet<&str> =
            defs.iter().filter(|d| d.scope == "pergame").map(|d| d.base_id.as_str()).collect();
        assert_eq!(global_bases.len(), 38, "全局成就基础数应为 38");
        assert_eq!(pergame_bases.len(), 16, "单游戏成就基础数应为 16");
        assert_eq!(defs.len(), 72, "展平后成就记录总数应为 72");
    }

    /// 【2026-11-08】量纲必须由白名单显式给出，前端才不会把「250 款」显示成「4 分钟」。
    /// 这里同时钉死两件事：白名单里的每个 base_id 都真有定义；每条定义的 unit 与白名单一致。
    #[test]
    fn every_definition_has_explicit_unit() {
        let defs = AchievementEngine::definitions();
        for base in SECONDS_TARGET_BASES {
            assert!(
                defs.iter().any(|d| d.base_id == *base),
                "白名单中的 {base} 没有对应定义（改名或漏定义）"
            );
        }
        for d in &defs {
            let expected = if SECONDS_TARGET_BASES.contains(&d.base_id.as_str()) {
                "seconds"
            } else {
                "count"
            };
            assert_eq!(d.unit, expected, "{} 的 unit 标错", d.id);
        }
        // 反面护栏：真实定义里计数型最大 target（G-18 的 500 次）已大于
        // 某些秒型成就的 target（P-08/P-09 的 HLTB 占位 0），
        // 证明「按数值大小猜单位」根本不可行，禁止回退。
        let max_count_target = defs
            .iter()
            .filter(|d| d.unit == "count")
            .map(|d| d.target)
            .max()
            .unwrap_or(0);
        assert!(max_count_target >= 500, "计数型成就应存在大数值目标");
        assert!(
            defs.iter()
                .any(|d| d.unit == "seconds" && d.target <= max_count_target),
            "应存在 target 不大于计数型的秒型成就（HLTB 动态目标）"
        );
    }

    /// 汇总计数去重口径（2026-10-12 拍板）：已解锁数 = 至少解锁过一次的**定义条目**数，
    /// 每款游戏的同名成就（如各游戏各自的「破冰」）只计一次；已删定义的幽灵行不计入。
    ///
    /// 回归背景：旧口径按「解锁记录条数」累加（本场景旧算法 = g01_1 + g23_1 + p01_1×3 = 5），
    /// 分子随游戏数膨胀，实机出现 84/72 = 117%；本断言把口径钉死在 2。
    #[test]
    fn summary_counts_unlocked_definitions_once() {
        let db = Database::new(std::path::Path::new(":memory:")).expect("内存库初始化失败");
        let now = "2026-10-12T00:00:00+00:00";

        // 三款游戏各自解锁「破冰」(p01_1)；再加一条全局「初来乍到」(g01_1)；
        // 以及一条**已删定义** G-23 的历史解锁行（幽灵行）
        for gid in ["g-a", "g-b", "g-c"] {
            let mut game = Game::new(format!("测试游戏 {gid}"));
            game.id = gid.to_string();
            db.upsert_game(&game).unwrap();
            db.try_unlock_achievement("p01_1", gid, now).unwrap();
        }
        db.try_unlock_achievement("g01_1", "", now).unwrap();
        db.try_unlock_achievement("g23_1", "", now).unwrap();

        let summary = AchievementEngine::get_summary(&db).unwrap();

        // 分母：展平定义条目数（55 全局 + 17 单游戏），不随游戏数变化
        assert_eq!(summary.total_count, 72, "分母应为展平定义条目数");
        // 分子：p01_1 三份记录只计一次 + g01_1 一次 = 2；g23_1 已删定义不计
        assert_eq!(
            summary.unlocked_count, 2,
            "去重口径：同名成就跨游戏只计一次，且只采信仍在册的定义"
        );
        // 不变量：百分比恒 ≤ 100%
        assert!(summary.unlocked_count <= summary.total_count);
    }

    /// 【2026-10-28】留档条目的 P-08/P-09 必须报 target = 0，其余 14 条原样。
    ///
    /// 背景：留档条目不参与结算（`eval_per_game` 早退返回 (0,false)），但 HLTB 会从同名
    /// 手账回填，于是这两条成了「可达但永不可完成」——前端按 target>0 计入分母，
    /// 留档卡片的完成度就永远停在天花板（旧口径 16 条里卡 14/16 = 88%）。
    ///
    /// 反向护栏同样重要：**不能**把 16 条统一清零，否则分母塌成「已解锁条目数」，
    /// 留档卡片会直接跳 100%。故这里同时断言 P-01 的 target 仍是 1、条目数仍是 16。
    #[test]
    fn removed_game_reports_zero_target_for_hltb_achievements() {
        let db = Database::new(std::path::Path::new(":memory:")).expect("内存库初始化失败");
        let now = "2026-10-28T00:00:00+00:00";

        // 留档条目：入库 → 删（只删 games 行）→ 写墓碑 → 留一条历史解锁让它进 with_traces
        let mut game = Game::new("战神".to_string());
        game.id = "g-removed".to_string();
        db.upsert_game(&game).unwrap();
        db.delete_game("g-removed").unwrap();
        db.insert_tombstone("g-removed", "战神", Some("God of War"), Some("GoW.exe"), now)
            .unwrap();
        db.try_unlock_achievement("p01_1", "g-removed", now).unwrap();

        // 手账里填了 HLTB，名字与墓碑一致 → database.rs 那一段会把它回填给留档条目
        let mut review = Review::new("战神".to_string());
        review.hltb_main_story = Some(10);
        review.hltb_completionist = Some(20);
        db.insert_review(&review).unwrap();

        let summary = AchievementEngine::get_summary(&db).unwrap();
        let entry = summary
            .per_game
            .iter()
            .find(|g| g.game_id == "g-removed")
            .expect("留档条目应出现在成就列表里");
        assert!(entry.removed, "该条目应标记为已删除留档");

        let target_of = |base: &str| {
            entry
                .achievements
                .iter()
                .find(|a| a.def.base_id == base)
                .unwrap_or_else(|| panic!("缺少成就定义 {base}"))
                .target
        };

        assert_eq!(target_of("P-08"), 0, "留档条目的主线成就不得再算可达成");
        assert_eq!(target_of("P-09"), 0, "留档条目的完美通关不得再算可达成");
        assert_eq!(target_of("P-01"), 1, "其余成就的目标值必须原样保留");
        // 单游戏 16 个基础定义里有一个是二级成就，展平后 17 条（与 defs.len()==72 的口径一致）
        assert_eq!(entry.achievements.len(), 17, "单游戏成就条目数不得缩水");
    }

    /// 【反向护栏】在库且填了 HLTB 的条目，P-08/P-09 仍取 HLTB 分钟数 × 60。
    /// 防「清零写成无条件」，把正常条目的参考时长也抹掉。
    #[test]
    fn live_game_keeps_hltb_targets() {
        let db = Database::new(std::path::Path::new(":memory:")).expect("内存库初始化失败");

        let mut game = Game::new("控制".to_string());
        game.id = "g-live".to_string();
        game.hltb_main_story = Some(10);
        game.hltb_completionist = Some(20);
        db.upsert_game(&game).unwrap();

        let summary = AchievementEngine::get_summary(&db).unwrap();
        let entry = summary
            .per_game
            .iter()
            .find(|g| g.game_id == "g-live")
            .expect("在库条目应出现在成就列表里");
        assert!(!entry.removed);

        let target_of = |base: &str| {
            entry
                .achievements
                .iter()
                .find(|a| a.def.base_id == base)
                .unwrap_or_else(|| panic!("缺少成就定义 {base}"))
                .target
        };
        assert_eq!(target_of("P-08"), 600, "主线 10 分钟 × 60 应保持 600");
        assert_eq!(target_of("P-09"), 1200, "完美通关 20 分钟 × 60 应保持 1200");
    }
}
