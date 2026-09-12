/**
 * 全局常量 — 集中管理所有魔法值
 * 修改默认主题色、超时时间等只需改这里
 */

// ==================== 主题色 ====================

/** 默认强调色（靛蓝） */
export const DEFAULT_ACCENT_COLOR = "#6366f1";

/** 深色背景主色 */
export const COLOR_DARK_BG = "#1a1a2e";

/** 深色背景副色 */
export const COLOR_DARK_BG_ALT = "#16213e";

/** 深色卡片/表面色 */
export const COLOR_SURFACE = "#2a2a3e";

// ==================== 交互 ====================

/** 搜索/输入防抖延迟（毫秒） */
export const DEBOUNCE_MS = 300;

// ==================== 封面主色调提取 ====================

/** 采样画布尺寸（像素） */
export const COVER_SAMPLE_SIZE = 32;

/** 亮度下限 — 低于此值的像素跳过 */
export const COVER_BRIGHTNESS_MIN = 30;

/** 亮度上限 — 高于此值的像素跳过 */
export const COVER_BRIGHTNESS_MAX = 230;

/** 默认强调色的 RGB 分量（用于 fallback） */
export const DEFAULT_ACCENT_RGB = { r: 99, g: 102, b: 241 };

// ==================== 时间常量（毫秒） ====================

export const TIME_MS = {
  MINUTE: 60_000,
  HOUR: 3_600_000,
  DAY: 86_400_000,
  WEEK: 604_800_000,
} as const;

// ==================== 游戏类型 ====================

/**
 * 规范游戏类型表（分组展示）。
 *
 * 唯一事实来源在后端 `src-tauri/src/core/genres.rs`（CANONICAL_GENRES），
 * 这里只是给前端录入/筛选用的副本——改类型表时**两边都要改**，
 * 否则前端候选与后端归一化口径会漂移。词表依据 Steam 官方中文标签。
 */
export const GENRE_GROUPS: { group: string; genres: string[] }[] = [
  {
    group: "动作",
    genres: ["动作", "动作冒险", "动作角色扮演", "砍杀", "格斗", "类魂系列", "跑动射击"],
  },
  {
    group: "射击",
    genres: ["射击", "第一人称射击", "第三人称射击", "弹幕射击", "刷宝射击游戏", "潜行"],
  },
  {
    group: "冒险与角色扮演",
    genres: [
      "冒险", "开放世界", "角色扮演", "日系角色扮演", "策略角色扮演", "战术角色扮演",
      "大型多人在线角色扮演", "迷宫探索", "类银河战士恶魔城", "视觉小说", "互动戏剧", "剧情丰富",
    ],
  },
  { group: "Roguelike", genres: ["类 Rogue", "轻度 Rogue"] },
  {
    group: "策略",
    genres: [
      "策略", "即时战略", "回合战略", "即时战术", "回合制战术",
      "4X", "塔防", "自走棋", "多人在线战术竞技", "战争游戏",
    ],
  },
  { group: "卡牌与解谜", genres: ["卡牌游戏", "集换式卡牌", "解谜", "三消", "隐藏物体", "益智问答"] },
  {
    group: "模拟与经营",
    genres: [
      "模拟", "沉浸式模拟", "生活模拟", "农场模拟", "汽车模拟", "飞行", "太空模拟", "上帝模拟",
      "城市营造", "基地建设", "沙盒", "开放世界生存制作", "生存", "管理", "恋爱模拟",
    ],
  },
  { group: "竞速与体育", genres: ["竞速", "体育", "足球", "篮球", "摔角", "滑板"] },
  { group: "恐怖", genres: ["恐怖", "生存恐怖"] },
  { group: "平台", genres: ["平台游戏", "精确平台游戏"] },
  { group: "其他", genres: ["休闲", "独立", "大逃杀", "节奏", "街机", "多人", "文字游戏"] },
];

/** 扁平化的规范类型列表 */
export const CANONICAL_GENRES: string[] = GENRE_GROUPS.flatMap((g) => g.genres);

// ==================== 游戏状态 ====================

/** 游戏状态枚举值 */
export const GAME_STATUS = {
  UNPLAYED: "unplayed",
  PLAYING: "playing",
  COMPLETED: "completed",
  ABANDONED: "abandoned",
  FAVORITES: "favorites",
} as const;
