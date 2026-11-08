import { describe, expect, it } from "vitest";
import type { AchievementDef } from "./tauri";
import {
  categoryColor,
  computeGameAchievementProgress,
  formatAchievementProgress,
  groupAchievements,
  groupProgress,
  groupStatusText,
  type AchievementStatusLike,
} from "./achievements";

function status(
  baseId: string,
  tier: number,
  over: Partial<AchievementStatusLike> = {},
  unit: "seconds" | "count" = "count"
): AchievementStatusLike {
  const def: AchievementDef = {
    id: `${baseId}-${tier}`,
    base_id: baseId,
    name: "藏书家",
    scope: "pergame",
    category: "collect",
    desc: "收藏 N 个",
    icon: "book",
    tier,
    tier_total: 3,
    target: tier * 10,
    unit,
  };
  return { def, unlocked: false, unlocked_at: null, progress: 0, target: def.target, ...over };
}

describe("groupAchievements：多级成就合并", () => {
  it("按 base_id 归组，组内保持后端定义顺序", () => {
    const groups = groupAchievements([
      status("P-01", 1),
      status("P-01", 2),
      status("P-01", 3),
      status("G-01", 1),
    ]);
    expect(groups).toHaveLength(2);
    expect(groups[0].items.map((i) => i.def.tier)).toEqual([1, 2, 3]);
    expect(groups[1].name).toBe("藏书家");
  });

  it("统计已解锁数量，并取最后一个已解锁的时间戳", () => {
    const groups = groupAchievements([
      status("P-01", 1, { unlocked: true, unlocked_at: "2026-01-01T00:00:00Z" }),
      status("P-01", 2, { unlocked: true, unlocked_at: "2026-02-01T00:00:00Z" }),
      status("P-01", 3),
    ]);
    expect(groups[0].unlockedCount).toBe(2);
    expect(groups[0].lastUnlockedAt).toBe("2026-02-01T00:00:00Z");
  });

  it("一个都没解锁时 lastUnlockedAt 为 null", () => {
    expect(groupAchievements([status("P-01", 1)])[0].lastUnlockedAt).toBeNull();
  });
});

describe("groupProgress：组进度取「下一个未达成」的目标", () => {
  it("未解锁任何等级时取第一级", () => {
    const [group] = groupAchievements([status("P-01", 1, { progress: 3 }), status("P-01", 2)]);
    expect(groupProgress(group)).toEqual({ progress: 3, target: 10 });
  });

  it("已解锁一级时，分母换成下一级目标", () => {
    const [group] = groupAchievements([
      status("P-01", 1, { unlocked: true, progress: 10 }),
      status("P-01", 2, { progress: 12 }),
    ]);
    expect(groupProgress(group)).toEqual({ progress: 10, target: 20 });
  });

  it("全级解锁后停在最高级", () => {
    const [group] = groupAchievements([
      status("P-01", 1, { unlocked: true, progress: 10 }),
      status("P-01", 2, { unlocked: true, progress: 30 }),
    ]);
    expect(groupProgress(group)).toEqual({ progress: 30, target: 20 });
  });
});

describe("groupStatusText：全解锁不再报进度", () => {
  it("全部等级解锁后直接说「已全部解锁」，不出现 30 / 20 这种倒挂", () => {
    const [group] = groupAchievements([
      status("P-01", 1, { unlocked: true, progress: 10 }),
      status("P-01", 2, { unlocked: true, progress: 30 }),
    ]);
    expect(groupStatusText(group)).toBe("已全部解锁");
  });

  it("未全解锁时仍按下一级目标报进度", () => {
    const [group] = groupAchievements([
      status("P-01", 1, { unlocked: true, progress: 10 }),
      status("P-01", 2, { unlocked: false, progress: 12 }),
    ]);
    expect(groupStatusText(group)).toBe("10 / 20");
  });
});

describe("formatAchievementProgress：按显式量纲切换，不靠数值猜", () => {
  it("target = 0 是「未收录参考时长」，不能说成未完成", () => {
    expect(formatAchievementProgress({ progress: 0, target: 0 })).toBe("未收录参考时长");
  });

  it("target = 1 视为布尔成就", () => {
    expect(formatAchievementProgress({ progress: 1, target: 1 })).toBe("已完成");
    expect(formatAchievementProgress({ progress: 0, target: 1 })).toBe("未完成");
  });

  it("unit = seconds 走时长格式，与 target 大小无关", () => {
    expect(formatAchievementProgress({ progress: 3600, target: 7200, unit: "seconds" })).toBe(
      "1h 0m / 2h 0m"
    );
    // P-08/P-09 的 HLTB 参考时长可能只有 10 分钟（600 秒）
    expect(formatAchievementProgress({ progress: 120, target: 600, unit: "seconds" })).toBe(
      "2m / 10m"
    );
  });

  it("计数型成就即便 target 很大也直出数字（旧版误显示成「X 分钟」）", () => {
    // G-02 藏书家 250 款、G-18 老江湖 500 次、G-39 养老玩家 365 天
    expect(formatAchievementProgress({ progress: 120, target: 250 })).toBe("120 / 250");
    expect(formatAchievementProgress({ progress: 0, target: 500 })).toBe("0 / 500");
    expect(formatAchievementProgress({ progress: 100, target: 365 })).toBe("100 / 365");
    expect(formatAchievementProgress({ progress: 3, target: 5 })).toBe("3 / 5");
  });

  it("缺省 unit 按计数处理，兼容旧数据", () => {
    expect(formatAchievementProgress({ progress: 3, target: 5 })).toBe("3 / 5");
  });
});

describe("groupStatusText：卡片墙也按量纲出文案", () => {
  it("计数型组显示纯数字，不出现「4 分钟 / 4 分钟」", () => {
    const [group] = groupAchievements([
      status("G-02", 1, { progress: 120, target: 250 }),
      status("G-02", 2, { progress: 120, target: 250 }),
    ]);
    expect(groupStatusText(group)).toBe("120 / 250");
  });

  it("秒型组显示时长", () => {
    const [group] = groupAchievements([
      status("P-02", 1, { progress: 120, target: 600 }, "seconds"),
      status("P-02", 2, { progress: 120, target: 600 }, "seconds"),
    ]);
    expect(groupStatusText(group)).toBe("2m / 10m");
  });
});

describe("computeGameAchievementProgress：分母只算可达成条目", () => {
  it("剔除 target=0 的条目（P-08/P-09 缺 HLTB 的场景）", () => {
    const items = [
      ...Array.from({ length: 15 }, () => status("P-01", 1, { target: 10 })),
      status("P-08", 1, { target: 0 }),
      status("P-09", 1, { target: 0 }),
    ];
    const out = computeGameAchievementProgress(items);
    expect(out.attainable).toBe(15);
    expect(out.skipped).toBe(2);
    expect(out.unlocked).toBe(0);
    expect(out.completed).toBe(false);
  });

  it("15/15 全部解锁即 100%，不再被 target=0 的条目压到 88%", () => {
    const items = [
      ...Array.from({ length: 15 }, () => status("P-01", 1, { unlocked: true })),
      status("P-08", 1, { target: 0 }),
      status("P-09", 1, { target: 0 }),
    ];
    const out = computeGameAchievementProgress(items);
    expect(out.percent).toBe(100);
    expect(out.completed).toBe(true);
  });

  it("已解锁但 target=0 的脏数据仍计入分子与分母", () => {
    const out = computeGameAchievementProgress([
      status("P-08", 1, { target: 0, unlocked: true }),
      status("P-01", 1, { target: 10 }),
    ]);
    expect(out.attainable).toBe(2);
    expect(out.unlocked).toBe(1);
    expect(out.percent).toBe(50);
  });

  it("没有任何可达成条目时不判定完成，避免 0/0 变成 100%", () => {
    const out = computeGameAchievementProgress([status("P-08", 1, { target: 0 })]);
    expect(out.attainable).toBe(0);
    expect(out.percent).toBe(0);
    expect(out.completed).toBe(false);
  });

  it("百分比取整后不超过 100", () => {
    const out = computeGameAchievementProgress([
      status("P-01", 1, { unlocked: true }),
      status("P-02", 1, { unlocked: true }),
      status("P-03", 1, { target: 10 }),
    ]);
    expect(out.percent).toBeLessThanOrEqual(100);
  });
});

describe("categoryColor", () => {
  it("已知分类取对应色，未知分类回退灰色", () => {
    expect(categoryColor("progress")).toBe("#4da3ff");
    expect(categoryColor("unknown")).toBe("#888");
  });
});
