import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { displayName, formatDate, formatPlayTime } from "./format";

describe("formatPlayTime", () => {
  it("0 秒显示「未游玩」而不是「0m」", () => {
    expect(formatPlayTime(0)).toBe("未游玩");
    expect(formatPlayTime(-1)).toBe("未游玩");
  });

  it("不足一分钟显示秒数", () => {
    expect(formatPlayTime(30)).toBe("30s");
    expect(formatPlayTime(59)).toBe("59s");
  });

  it("不足一小时显示分钟", () => {
    expect(formatPlayTime(60)).toBe("1m");
    expect(formatPlayTime(3599)).toBe("59m");
  });

  it("一小时以上显示「时 + 分」，分钟为 0 也保留", () => {
    expect(formatPlayTime(3600)).toBe("1h 0m");
    expect(formatPlayTime(9000)).toBe("2h 30m");
    expect(formatPlayTime(360000)).toBe("100h 0m");
  });
});

describe("formatDate", () => {
  beforeEach(() => {
    // 固定"现在"，否则相对时间断言会随真实时钟漂移
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-10-17T12:00:00+08:00"));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("空值与非法值统一显示「未知」", () => {
    expect(formatDate(null)).toBe("未知");
    expect(formatDate("")).toBe("未知");
    expect(formatDate("not-a-date")).toBe("未知");
  });

  it("一分钟内显示「刚刚」", () => {
    expect(formatDate("2026-10-17T11:59:30+08:00")).toBe("刚刚");
  });

  it("按跨度选单位：分钟 / 小时 / 天", () => {
    expect(formatDate("2026-10-17T11:55:00+08:00")).toBe("5分钟前");
    expect(formatDate("2026-10-17T09:00:00+08:00")).toBe("3小时前");
    expect(formatDate("2026-10-15T12:00:00+08:00")).toBe("2天前");
  });

  it("超过一周回落到日期字符串", () => {
    const out = formatDate("2026-08-01T12:00:00+08:00");
    expect(out).not.toContain("前");
    expect(out).toMatch(/2026/);
  });

  it("未来时间不回推成「刚刚」，直接显示日期", () => {
    const out = formatDate("2026-12-01T12:00:00+08:00");
    expect(out).not.toContain("前");
  });
});

describe("displayName", () => {
  it("纯 ASCII 英文名优先", () => {
    expect(displayName({ name: "赛博朋克 2077", name_en: "Cyberpunk 2077" })).toBe(
      "Cyberpunk 2077"
    );
  });

  it("name_en 是中文（历史脏数据）时退回中文主名", () => {
    expect(displayName({ name: "赛博朋克 2077", name_en: "赛博朋克" })).toBe("赛博朋克 2077");
  });

  it("中英混排的 name_en 不算纯 ASCII，同样退回主名", () => {
    expect(displayName({ name: "黑神话：悟空", name_en: "Black Myth 悟空" })).toBe(
      "黑神话：悟空"
    );
  });

  it("name_en 为空或缺省时用主名", () => {
    expect(displayName({ name: "只狼", name_en: null })).toBe("只狼");
    expect(displayName({ name: "只狼", name_en: "   " })).toBe("只狼");
    expect(displayName({ name: "只狼" })).toBe("只狼");
  });
});
