import type { AchievementDef } from "./tauri";
import { formatPlayTime } from "./format";

/**
 * 成就状态（全局成就与单游戏成就字段同构，后端是两个独立 struct）
 * 放在这里统一处理，避免卡片、抽屉、页面各写一套分组/进度算法。
 */
export interface AchievementStatusLike {
  def: AchievementDef;
  unlocked: boolean;
  unlocked_at: string | null;
  progress: number;
  target: number;
}

export const CATEGORY_META: Record<string, { label: string; color: string }> = {
  progress: { label: "进度", color: "#4da3ff" },
  collect: { label: "收集", color: "#a78bfa" },
  fun: { label: "趣味", color: "#fbbf24" },
  challenge: { label: "挑战", color: "#f87171" },
};

/** 按基础 ID 合并后的成就组（多级成就合为一行展示） */
export interface AchievementGroup {
  name: string;
  icon: string;
  desc: string;
  category: string;
  items: AchievementStatusLike[];
  unlockedCount: number;
  lastUnlockedAt: string | null;
}

/** 分类徽章颜色（未知分类回退灰色） */
export function categoryColor(cat: string): string {
  return CATEGORY_META[cat]?.color ?? "#888";
}

/**
 * 按 `def.base_id` 分组：多级成就（如藏书家 1/4 级）合并成一组，
 * 组内顺序沿用后端定义顺序（tier 1 → N）。
 */
export function groupAchievements(items: AchievementStatusLike[]): AchievementGroup[] {
  const map = new Map<string, AchievementStatusLike[]>();
  for (const item of items) {
    const arr = map.get(item.def.base_id) ?? [];
    arr.push(item);
    map.set(item.def.base_id, arr);
  }
  return [...map.values()].map((groupItems) => {
    const unlockedItems = groupItems.filter((i) => i.unlocked);
    return {
      name: groupItems[0].def.name,
      icon: groupItems[0].def.icon,
      desc: groupItems[0].def.desc,
      category: groupItems[0].def.category,
      items: groupItems,
      unlockedCount: unlockedItems.length,
      lastUnlockedAt: unlockedItems.length
        ? unlockedItems[unlockedItems.length - 1].unlocked_at
        : null,
    };
  });
}

/**
 * 组进度：取当前最高已解锁等级的下一个目标；未解锁任何等级则取第一级目标。
 */
export function groupProgress(group: AchievementGroup): { progress: number; target: number } {
  const unlockedTiers = group.items.filter((i) => i.unlocked).length;
  if (unlockedTiers === 0) {
    return { progress: group.items[0]?.progress ?? 0, target: group.items[0]?.target ?? 0 };
  }
  const lastUnlocked = group.items[unlockedTiers - 1];
  const next = group.items[unlockedTiers];
  if (next) return { progress: lastUnlocked.progress, target: next.target };
  return { progress: lastUnlocked.progress, target: lastUnlocked.target };
}

/**
 * 进度文案。`target === 0` 特指 P-08/P-09 这类缺少 HLTB 参考时长的条目——
 * 它们永远无法达成，不计入完成度，文案上要说清楚，不能糊成「未完成」。
 */
export function formatAchievementProgress(item: { progress: number; target: number }): string {
  const { progress, target } = item;
  if (target === 0) return "未收录参考时长";
  if (target === 1) return progress >= 1 ? "已完成" : "未完成";
  if (target >= 3600) return `${formatPlayTime(progress)} / ${formatPlayTime(target)}`;
  if (target >= 60) {
    const pm = Math.floor(progress / 60);
    const tm = Math.floor(target / 60);
    return `${pm} 分钟 / ${tm} 分钟`;
  }
  return `${progress} / ${target}`;
}

export interface GameAchievementProgress {
  /** 已解锁条目数 */
  unlocked: number;
  /** 可达成条目数（分母；`target === 0` 的条目已剔除） */
  attainable: number;
  /** 因缺少参考时长而不计入完成度的条目数 */
  skipped: number;
  /** 完成度百分比（0-100） */
  percent: number;
  /** 可达成条目全部解锁 */
  completed: boolean;
}

/**
 * 单游戏完成度（2026-10-17 拍板）：分母只算**可达成**条目。
 * P-08/P-09 依赖 HLTB 参考时长，库里没收录时 target = 0，永远解锁不了；
 * 若仍按 17 条做分母，这类游戏的天花板永远是 15/17（88%）。
 * 已解锁的条目无条件计入分子（防御 target 异常为 0 的脏数据）。
 */
export function computeGameAchievementProgress(
  items: AchievementStatusLike[]
): GameAchievementProgress {
  const attainableItems = items.filter((i) => i.target > 0 || i.unlocked);
  const unlocked = items.filter((i) => i.unlocked).length;
  const attainable = attainableItems.length;
  const percent =
    attainable > 0 ? Math.min(100, Math.round((unlocked / attainable) * 100)) : 0;
  return {
    unlocked,
    attainable,
    skipped: items.length - attainable,
    percent,
    completed: attainable > 0 && unlocked >= attainable,
  };
}
