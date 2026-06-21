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

// ==================== 游戏状态 ====================

/** 游戏状态枚举值 */
export const GAME_STATUS = {
  UNPLAYED: "unplayed",
  PLAYING: "playing",
  COMPLETED: "completed",
  ABANDONED: "abandoned",
  FAVORITES: "favorites",
} as const;
