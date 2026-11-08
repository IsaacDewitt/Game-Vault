import { describe, expect, it } from "vitest";
import {
  effectiveStatus,
  filterAndSortGames,
  filterGames,
  sortGames,
  type FilterableGame,
} from "./gameFilter";

/** 构造最小可筛选游戏（未给字段走"从未玩过"的默认值） */
function g(over: Partial<FilterableGame> & { name: string }): FilterableGame {
  return {
    name_en: null,
    status: "unplayed",
    play_time_seconds: 0,
    genres: [],
    is_favorite: false,
    last_played: null,
    added_at: "2026-01-01T00:00:00+00:00",
    ...over,
  };
}

describe("effectiveStatus：三态派生口径", () => {
  it("completed 优先于游玩时长", () => {
    expect(effectiveStatus({ status: "completed", play_time_seconds: 0 })).toBe("completed");
    expect(effectiveStatus({ status: "completed", play_time_seconds: 9999 })).toBe("completed");
  });

  it("未通关时，有游玩时长即已游玩", () => {
    expect(effectiveStatus({ status: "unplayed", play_time_seconds: 1 })).toBe("played");
  });

  it("未通关且无时长即未游玩", () => {
    expect(effectiveStatus({ status: "unplayed", play_time_seconds: 0 })).toBe("unplayed");
  });

  it("历史的 playing / abandoned 标记一律按有无时长归类，不会掉出三态", () => {
    // 这是本次收敛的重点：这类值不再需要一条「按原始 status 精确匹配」的兜底分支
    expect(effectiveStatus({ status: "playing", play_time_seconds: 600 })).toBe("played");
    expect(effectiveStatus({ status: "abandoned", play_time_seconds: 0 })).toBe("unplayed");
    expect(effectiveStatus({ status: "abandoned", play_time_seconds: 600 })).toBe("played");
    expect(effectiveStatus({ status: "playing", play_time_seconds: 0 })).toBe("unplayed");
  });
});

describe("filterGames", () => {
  const library: FilterableGame[] = [
    g({ name: "赛博朋克 2077", name_en: "Cyberpunk 2077", genres: ["角色扮演", "开放世界"] }),
    g({
      name: "全面战争：三国",
      name_en: "Total War: THREE KINGDOMS",
      genres: ["策略"],
      play_time_seconds: 7200,
      last_played: "2026-05-01T00:00:00+00:00",
      status: "completed",
    }),
    g({
      name: "黑神话：悟空",
      name_en: "Black Myth: Wukong",
      genres: ["动作"],
      play_time_seconds: 3600,
      last_played: "2026-06-01T00:00:00+00:00",
      is_favorite: true,
    }),
  ];

  it("无条件时返回副本而非原数组（避免调用方误改 store 状态）", () => {
    const out = filterGames(library, {});
    expect(out).toEqual(library);
    expect(out).not.toBe(library);
  });

  it("搜索同时匹配中文名与英文名", () => {
    expect(filterGames(library, { search: "赛博" }).map((x) => x.name)).toEqual(["赛博朋克 2077"]);
    expect(filterGames(library, { search: "wukong" }).map((x) => x.name)).toEqual(["黑神话：悟空"]);
  });

  it("搜索忽略大小写与首尾空白", () => {
    expect(filterGames(library, { search: "  CYBERPUNK " })).toHaveLength(1);
  });

  it("三态筛选按派生口径，而不是原始 status 字符串", () => {
    const completed = filterGames(library, { status: "completed" });
    expect(completed.map((x) => x.name)).toEqual(["全面战争：三国"]);

    const played = filterGames(library, { status: "played" });
    expect(played.map((x) => x.name)).toEqual(["黑神话：悟空"]);

    const unplayed = filterGames(library, { status: "unplayed" });
    expect(unplayed.map((x) => x.name)).toEqual(["赛博朋克 2077"]);
  });

  it("收藏是独立维度，选中收藏时不再叠加状态过滤", () => {
    expect(filterGames(library, { status: "favorites" }).map((x) => x.name)).toEqual([
      "黑神话：悟空",
    ]);
  });

  it("未知状态值只会退化为「不过滤」，不会把列表清空", () => {
    expect(filterGames(library, { status: "whatever" })).toHaveLength(3);
  });

  it("类型按子串模糊匹配（保持既有语义：rpg 也会命中 jrpg）", () => {
    const lib = [
      g({ name: "A", genres: ["RPG"] }),
      g({ name: "B", genres: ["JRPG"] }),
      g({ name: "C", genres: ["策略"] }),
    ];
    expect(filterGames(lib, { genre: "rpg" }).map((x) => x.name)).toEqual(["A", "B"]);
  });
});

describe("sortGames", () => {
  it("最近互动：有游玩记录看游玩时间，从未玩过看入库时间（降序）", () => {
    const lib = [
      g({ name: "old", last_played: "2026-01-01T00:00:00+00:00", added_at: "2025-01-01T00:00:00+00:00" }),
      g({ name: "new", last_played: "2026-06-01T00:00:00+00:00", added_at: "2024-01-01T00:00:00+00:00" }),
      // 刚添加、还没玩过 → 排序键取 added_at，理应排在游玩记录之上
      g({ name: "刚添加", added_at: "2026-11-08T10:00:00+00:00" }),
      // 很久以前就入库且一直没玩 → 沉到最底下
      g({ name: "陈年未玩", added_at: "2023-01-01T00:00:00+00:00" }),
    ];
    expect(sortGames(lib, "last_played", "desc").map((x) => x.name)).toEqual([
      "刚添加",
      "new",
      "old",
      "陈年未玩",
    ]);
  });

  it("最近互动：升序就是反向（最久没互动的在前）", () => {
    const lib = [
      g({ name: "old", last_played: "2026-01-01T00:00:00+00:00", added_at: "2025-01-01T00:00:00+00:00" }),
      g({ name: "new", last_played: "2026-06-01T00:00:00+00:00", added_at: "2024-01-01T00:00:00+00:00" }),
      g({ name: "刚添加", added_at: "2026-11-08T10:00:00+00:00" }),
    ];
    expect(sortGames(lib, "last_played", "asc").map((x) => x.name)).toEqual([
      "old",
      "new",
      "刚添加",
    ]);
  });

  it("最近互动：两个时间字段都缺的脏数据沉底，不会冒到前面", () => {
    const lib = [
      g({ name: "脏数据", added_at: "" }),
      g({ name: "刚添加", added_at: "2026-11-08T10:00:00+00:00" }),
    ];
    expect(sortGames(lib, "last_played", "desc").map((x) => x.name)).toEqual([
      "刚添加",
      "脏数据",
    ]);
  });

  it("名称：按展示名排序（英文名优先）", () => {
    const lib = [
      g({ name: "黑神话：悟空", name_en: "Black Myth: Wukong" }),
      g({ name: "赛博朋克 2077", name_en: "Cyberpunk 2077" }),
    ];
    expect(sortGames(lib, "name", "asc").map((x) => x.name_en)).toEqual([
      "Black Myth: Wukong",
      "Cyberpunk 2077",
    ]);
  });

  it("时长：降序按秒数从多到少", () => {
    const lib = [
      g({ name: "mid", play_time_seconds: 600 }),
      g({ name: "max", play_time_seconds: 7200 }),
      g({ name: "zero" }),
    ];
    expect(sortGames(lib, "play_time", "desc").map((x) => x.name)).toEqual(["max", "mid", "zero"]);
  });

  it("添加时间：升序按入库先后", () => {
    const lib = [
      g({ name: "b", added_at: "2026-02-01T00:00:00+00:00" }),
      g({ name: "a", added_at: "2026-01-01T00:00:00+00:00" }),
    ];
    expect(sortGames(lib, "added_at", "asc").map((x) => x.name)).toEqual(["a", "b"]);
  });

  it("未知排序字段退回最近互动（与后端白名单兜底一致）", () => {
    const lib = [
      g({ name: "never", added_at: "2025-01-01T00:00:00+00:00" }),
      g({ name: "played", last_played: "2026-01-01T00:00:00+00:00" }),
    ];
    expect(sortGames(lib, "drop table", "desc").map((x) => x.name)).toEqual(["played", "never"]);
  });

  it("不改动原数组", () => {
    const lib = [g({ name: "a" }), g({ name: "b", play_time_seconds: 10 })];
    const snapshot = lib.map((x) => x.name);
    sortGames(lib, "play_time", "desc");
    expect(lib.map((x) => x.name)).toEqual(snapshot);
  });
});

describe("filterAndSortGames：组合后仍守同一口径", () => {
  it("先筛后排，且默认排序等价于旧行为（最近互动降序）", () => {
    const lib = [
      g({ name: "未玩", last_played: null, added_at: "2025-06-01T00:00:00+00:00" }),
      g({
        name: "已玩旧",
        play_time_seconds: 60,
        last_played: "2026-01-01T00:00:00+00:00",
        genres: ["动作"],
      }),
      g({
        name: "已玩新",
        play_time_seconds: 60,
        last_played: "2026-09-01T00:00:00+00:00",
        genres: ["动作"],
      }),
    ];
    expect(filterAndSortGames(lib, { genre: "动作" }).map((x) => x.name)).toEqual([
      "已玩新",
      "已玩旧",
    ]);
  });

  it("默认排序下，刚添加的游戏出现在第一位（免去往下翻）", () => {
    const lib = [
      g({ name: "老游戏", play_time_seconds: 7200, last_played: "2026-10-01T00:00:00+00:00" }),
      g({ name: "刚添加", added_at: "2026-11-08T12:00:00+00:00" }),
    ];
    expect(filterAndSortGames(lib).map((x) => x.name)).toEqual(["刚添加", "老游戏"]);
  });
});
