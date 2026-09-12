//! 游戏类型规范表 —— 统一 LLM 输出与历史数据的类型口径
//!
//! ## 背景
//! LLM 输出不稳定，同一类型会同时落出「动作」与「Action」、「开放世界」与「Open World」等
//! 中英双份写法。后果是筛选下拉出现重复项、类型统计被摊薄失真。
//!
//! ## 设计
//! - `CANONICAL_GENRES`：规范类型表，中文，词表参照 Steam 官方中文标签
//!   （<https://partner.steamgames.com/doc/store/tags>），按语义分组排列。
//! - `GENRE_ALIASES`：别名表，把常见英文写法与近义中文映射到规范名。
//! - **归一化策略：命中别名→替换为规范名；未命中→原样保留。**
//!   宁可留一条陌生类型，也不静默丢弃用户已录入的信息（未知类型会记 warn 日志便于后续补表）。
//!
//! ## 新增类型的正确姿势
//! 1. 往 `CANONICAL_GENRES` 对应分组加规范名；
//! 2. 若存在常见异写，往 `GENRE_ALIASES` 补映射；
//! 3. 跑 `cargo test -p game-vault genres`（单测里有规范表自洽性断言）。

/// 规范类型表（中文，Steam 官方标签词汇）。顺序 = 提示词与前端选项的展示顺序。
pub const CANONICAL_GENRES: &[&str] = &[
    // ---- 动作 ----
    "动作",
    "动作冒险",
    "动作角色扮演",
    "砍杀",
    "格斗",
    "类魂系列",
    "跑动射击",
    // ---- 射击 ----
    "射击",
    "第一人称射击",
    "第三人称射击",
    "弹幕射击",
    "刷宝射击游戏",
    "潜行",
    // ---- 冒险与角色扮演 ----
    "冒险",
    "开放世界",
    "角色扮演",
    "日系角色扮演",
    "策略角色扮演",
    "战术角色扮演",
    "大型多人在线角色扮演",
    "迷宫探索",
    "类银河战士恶魔城",
    "视觉小说",
    "互动戏剧",
    "剧情丰富",
    // ---- Roguelike ----
    "类 Rogue",
    "轻度 Rogue",
    // ---- 策略 ----
    "策略",
    "即时战略",
    "回合战略",
    "即时战术",
    "回合制战术",
    "4X",
    "塔防",
    "自走棋",
    "多人在线战术竞技",
    "战争游戏",
    // ---- 卡牌与解谜 ----
    "卡牌游戏",
    "集换式卡牌",
    "解谜",
    "三消",
    "隐藏物体",
    "益智问答",
    // ---- 模拟与经营 ----
    "模拟",
    "沉浸式模拟",
    "生活模拟",
    "农场模拟",
    "汽车模拟",
    "飞行",
    "太空模拟",
    "上帝模拟",
    "城市营造",
    "基地建设",
    "沙盒",
    "开放世界生存制作",
    "生存",
    "管理",
    "恋爱模拟",
    // ---- 竞速与体育 ----
    "竞速",
    "体育",
    "足球",
    "篮球",
    "摔角",
    "滑板",
    // ---- 恐怖 ----
    "恐怖",
    "生存恐怖",
    // ---- 平台 ----
    "平台游戏",
    "精确平台游戏",
    // ---- 其他 ----
    "休闲",
    "独立",
    "大逃杀",
    "节奏",
    "街机",
    "多人",
    "文字游戏",
];

/// 别名表：`(别名, 规范名)`。匹配忽略大小写与首尾空白。
/// 键一律小写，值必须存在于 `CANONICAL_GENRES`（单测校验）。
pub const GENRE_ALIASES: &[(&str, &str)] = &[
    // ---- 动作 ----
    ("action", "动作"),
    ("action game", "动作"),
    ("动作游戏", "动作"),
    ("动作类", "动作"),
    ("act", "动作"),
    ("action-adventure", "动作冒险"),
    ("action adventure", "动作冒险"),
    ("动作冒险游戏", "动作冒险"),
    ("action rpg", "动作角色扮演"),
    ("action-rpg", "动作角色扮演"),
    ("arpg", "动作角色扮演"),
    ("动作角色扮演游戏", "动作角色扮演"),
    ("hack and slash", "砍杀"),
    ("hack & slash", "砍杀"),
    ("hack and slash game", "砍杀"),
    ("砍杀游戏", "砍杀"),
    ("割草", "砍杀"),
    ("fighting", "格斗"),
    ("格斗游戏", "格斗"),
    ("对战格斗", "格斗"),
    ("souls-like", "类魂系列"),
    ("soulslike", "类魂系列"),
    ("souls like", "类魂系列"),
    ("魂系", "类魂系列"),
    ("类魂", "类魂系列"),
    ("run and gun", "跑动射击"),
    ("run-and-gun", "跑动射击"),
    ("跑打", "跑动射击"),
    // ---- 射击 ----
    ("shooter", "射击"),
    ("射击游戏", "射击"),
    ("fps", "第一人称射击"),
    ("first-person shooter", "第一人称射击"),
    ("first person shooter", "第一人称射击"),
    ("第一人称射击游戏", "第一人称射击"),
    ("tps", "第三人称射击"),
    ("third-person shooter", "第三人称射击"),
    ("third person shooter", "第三人称射击"),
    ("第三人称射击游戏", "第三人称射击"),
    ("bullet hell", "弹幕射击"),
    ("shoot 'em up", "弹幕射击"),
    ("shoot em up", "弹幕射击"),
    ("清版射击", "弹幕射击"),
    ("弹幕", "弹幕射击"),
    ("looter shooter", "刷宝射击游戏"),
    ("刷宝射击", "刷宝射击游戏"),
    ("stealth", "潜行"),
    ("潜行游戏", "潜行"),
    ("隐匿", "潜行"),
    // ---- 冒险与角色扮演 ----
    ("adventure", "冒险"),
    ("冒险游戏", "冒险"),
    ("avg", "冒险"),
    ("open world", "开放世界"),
    ("open-world", "开放世界"),
    ("开放世界游戏", "开放世界"),
    ("rpg", "角色扮演"),
    ("role-playing", "角色扮演"),
    ("role playing game", "角色扮演"),
    ("角色扮演游戏", "角色扮演"),
    ("jrpg", "日系角色扮演"),
    ("日式角色扮演", "日系角色扮演"),
    ("srpg", "策略角色扮演"),
    ("strategy rpg", "策略角色扮演"),
    ("strategy-rpg", "策略角色扮演"),
    ("tactical rpg", "战术角色扮演"),
    ("战术角色扮演游戏", "战术角色扮演"),
    ("mmorpg", "大型多人在线角色扮演"),
    ("mmo", "大型多人在线角色扮演"),
    ("大型多人在线", "大型多人在线角色扮演"),
    ("dungeon crawler", "迷宫探索"),
    ("地牢探索", "迷宫探索"),
    ("metroidvania", "类银河战士恶魔城"),
    ("银河恶魔城", "类银河战士恶魔城"),
    ("类银河恶魔城", "类银河战士恶魔城"),
    ("visual novel", "视觉小说"),
    ("vn", "视觉小说"),
    ("interactive drama", "互动戏剧"),
    ("互动电影", "互动戏剧"),
    ("交互式电影", "互动戏剧"),
    ("story rich", "剧情丰富"),
    ("剧情", "剧情丰富"),
    ("叙事", "剧情丰富"),
    // ---- Roguelike ----
    ("roguelike", "类 Rogue"),
    ("rogue-like", "类 Rogue"),
    ("roguelike游戏", "类 Rogue"),
    ("roguelite", "轻度 Rogue"),
    ("rogue-lite", "轻度 Rogue"),
    // ---- 策略 ----
    ("strategy", "策略"),
    ("策略游戏", "策略"),
    ("rts", "即时战略"),
    ("real-time strategy", "即时战略"),
    ("real time strategy", "即时战略"),
    ("即时战略游戏", "即时战略"),
    ("tbs", "回合战略"),
    ("turn-based strategy", "回合战略"),
    ("turn based strategy", "回合战略"),
    ("回合制战略", "回合战略"),
    ("rtt", "即时战术"),
    ("real-time tactics", "即时战术"),
    ("real time tactics", "即时战术"),
    ("turn-based tactics", "回合制战术"),
    ("turn based tactics", "回合制战术"),
    ("turn-based tactical", "回合制战术"),
    ("战术回合制", "回合制战术"),
    ("tower defense", "塔防"),
    ("塔防游戏", "塔防"),
    ("auto battler", "自走棋"),
    ("auto chess", "自走棋"),
    ("moba", "多人在线战术竞技"),
    ("war game", "战争游戏"),
    ("wargame", "战争游戏"),
    // ---- 卡牌与解谜 ----
    ("card game", "卡牌游戏"),
    ("卡牌", "卡牌游戏"),
    ("trading card game", "集换式卡牌"),
    ("tcg", "集换式卡牌"),
    ("puzzle", "解谜"),
    ("解谜游戏", "解谜"),
    ("益智", "解谜"),
    ("match 3", "三消"),
    ("三消游戏", "三消"),
    ("hidden object", "隐藏物体"),
    ("trivia", "益智问答"),
    // ---- 模拟与经营 ----
    ("simulation", "模拟"),
    ("sim", "模拟"),
    ("模拟游戏", "模拟"),
    ("immersive sim", "沉浸式模拟"),
    ("life simulation", "生活模拟"),
    ("farming sim", "农场模拟"),
    ("农场经营", "农场模拟"),
    ("car simulation", "汽车模拟"),
    ("driving", "汽车模拟"),
    ("驾驶模拟", "汽车模拟"),
    ("flight", "飞行"),
    ("飞行模拟", "飞行"),
    ("space sim", "太空模拟"),
    ("god game", "上帝模拟"),
    ("city builder", "城市营造"),
    ("城市建造", "城市营造"),
    ("base building", "基地建设"),
    ("基地建造", "基地建设"),
    ("sandbox", "沙盒"),
    ("沙盒游戏", "沙盒"),
    ("survival", "生存"),
    ("生存游戏", "生存"),
    ("management", "管理"),
    ("经营", "管理"),
    ("模拟经营", "管理"),
    ("dating sim", "恋爱模拟"),
    // ---- 竞速与体育 ----
    ("racing", "竞速"),
    ("赛车", "竞速"),
    ("竞速游戏", "竞速"),
    ("sports", "体育"),
    ("体育游戏", "体育"),
    ("football", "足球"),
    ("soccer", "足球"),
    ("basketball", "篮球"),
    ("wrestling", "摔角"),
    ("skateboarding", "滑板"),
    // ---- 恐怖 ----
    ("horror", "恐怖"),
    ("恐怖游戏", "恐怖"),
    ("survival horror", "生存恐怖"),
    ("生存恐怖游戏", "生存恐怖"),
    // ---- 平台 ----
    ("platformer", "平台游戏"),
    ("platform", "平台游戏"),
    ("平台跳跃", "平台游戏"),
    ("平台", "平台游戏"),
    ("precision platformer", "精确平台游戏"),
    // ---- 其他 ----
    ("casual", "休闲"),
    ("indie", "独立"),
    ("独立游戏", "独立"),
    ("battle royale", "大逃杀"),
    ("吃鸡", "大逃杀"),
    ("rhythm", "节奏"),
    ("音乐游戏", "节奏"),
    ("音乐", "节奏"),
    ("arcade", "街机"),
    ("multiplayer", "多人"),
    ("多人游戏", "多人"),
    ("word game", "文字游戏"),
];

/// 归一化单个类型：命中别名→规范名，否则原样返回（trim 后）。
///
/// 空串原样返回，由调用方决定是否过滤。
pub fn normalize_genre(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // 1. 已是规范名（忽略大小写）→ 直接采用规范写法
    let lower = trimmed.to_lowercase();
    if let Some(hit) = CANONICAL_GENRES.iter().find(|g| g.to_lowercase() == lower) {
        return hit.to_string();
    }

    // 2. 别名表命中
    if let Some((_, canonical)) = GENRE_ALIASES.iter().find(|(alias, _)| *alias == lower) {
        return canonical.to_string();
    }

    // 3. 未命中：原样保留，避免静默丢失用户数据
    tracing::debug!("未知游戏类型（未收录别名，原样保留）: {}", trimmed);
    trimmed.to_string()
}

/// 归一化类型列表：trim → 归一化 → 去重（保持原有顺序）→ 去空。
///
/// 注意：顺序敏感的调用方（如写入 DB）依赖此处保持输入顺序，不要改成 HashSet。
pub fn normalize_genres(genres: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(genres.len());
    for g in genres {
        let n = normalize_genre(g);
        if n.is_empty() {
            continue;
        }
        // 归一化后可能与前面已收录项重复（如 ["动作", "Action"]），去重
        if !out.iter().any(|x| x == &n) {
            out.push(n);
        }
    }
    out
}

/// 供 LLM 提示词内联的类型清单（中文顿号分隔）
pub fn genre_prompt_list() -> String {
    CANONICAL_GENRES.join("、")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_list_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for g in CANONICAL_GENRES {
            assert!(seen.insert(*g), "规范类型重复: {}", g);
        }
    }

    #[test]
    fn every_alias_target_is_canonical() {
        for (alias, target) in GENRE_ALIASES {
            assert!(
                CANONICAL_GENRES.contains(target),
                "别名 [{}] 指向了非规范类型: {}",
                alias,
                target
            );
        }
    }

    #[test]
    fn aliases_are_lowercase_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for (alias, _) in GENRE_ALIASES {
            assert_eq!(*alias, alias.to_lowercase(), "别名必须小写: {}", alias);
            assert!(seen.insert(*alias), "别名重复: {}", alias);
        }
    }

    #[test]
    fn canonical_names_round_trip() {
        for g in CANONICAL_GENRES {
            assert_eq!(normalize_genre(g), *g, "规范类型应原样返回: {}", g);
        }
        // 大小写扰动也要落回规范写法
        assert_eq!(normalize_genre("  ACTION  "), "动作");
    }

    #[test]
    fn english_aliases_map_to_chinese() {
        assert_eq!(normalize_genre("Action"), "动作");
        assert_eq!(normalize_genre("Adventure"), "冒险");
        assert_eq!(normalize_genre("Open World"), "开放世界");
        assert_eq!(normalize_genre("Stealth"), "潜行");
        assert_eq!(normalize_genre("FPS"), "第一人称射击");
        assert_eq!(normalize_genre("Third-person shooter"), "第三人称射击");
        assert_eq!(normalize_genre("Action RPG"), "动作角色扮演");
        assert_eq!(normalize_genre("RTS"), "即时战略");
        assert_eq!(normalize_genre("Metroidvania"), "类银河战士恶魔城");
        assert_eq!(normalize_genre("Souls-like"), "类魂系列");
    }

    #[test]
    fn chinese_aliases_map_to_canonical() {
        assert_eq!(normalize_genre("剧情"), "剧情丰富");
        assert_eq!(normalize_genre("平台跳跃"), "平台游戏");
        assert_eq!(normalize_genre("战术回合制"), "回合制战术");
        assert_eq!(normalize_genre("魂系"), "类魂系列");
    }

    #[test]
    fn unknown_genre_is_preserved() {
        // 未收录类型原样保留，不静默丢弃
        assert_eq!(normalize_genre("某种新潮玩法"), "某种新潮玩法");
        assert_eq!(normalize_genre(""), "");
    }

    #[test]
    fn normalize_list_dedupes_and_keeps_order() {
        let out = normalize_genres(&[
            "动作".to_string(),
            "Action".to_string(),
            "冒险".to_string(),
            "Adventure".to_string(),
            "  ".to_string(),
            "RPG".to_string(),
        ]);
        assert_eq!(out, vec!["动作", "冒险", "角色扮演"]);
    }

    /// 覆盖现有库中真实出现过的中英双份写法，确保迁移后全部收敛
    #[test]
    fn all_legacy_values_in_db_converge() {
        let legacy = [
            "动作", "冒险", "动作冒险", "开放世界", "Action", "Adventure", "潜行", "生存恐怖",
            "第三人称射击", "角色扮演", "Open World", "即时战略", "砍杀", "第一人称射击",
            "Action RPG", "First-person shooter", "Stealth", "Third-person shooter", "互动戏剧",
            "剧情", "动作角色扮演", "多人", "平台跳跃", "恐怖", "战术回合制", "跑动射击",
        ];
        let out = normalize_genres(&legacy.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        // 全部落回中文规范名：不应残留任何纯 ASCII 类型
        assert!(
            !out.iter().any(|g| g.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ')),
            "仍有英文类型未收敛: {:?}",
            out
        );
        // 26 条原始值去重归一后应显著收敛
        assert!(out.len() <= 20, "归一后类型数未收敛: {} -> {:?}", out.len(), out);
    }

    #[test]
    fn prompt_list_is_non_empty() {
        let list = genre_prompt_list();
        assert!(list.contains("动作"));
        assert_eq!(list.matches('、').count() + 1, CANONICAL_GENRES.len());
    }
}
