import type { Game } from "./tauri";
import { displayName } from "./format";

/**
 * 游戏库的筛选与排序单一实现。
 *
 * 为什么放在这里：这套口径原先写在 `stores/games.ts` 的 `filteredGames` computed 里，
 * 后端 `get_status_stats` 还有一份 SQL 推导，两份实现互不相干、只能靠注释维持一致。
 * 抽成纯函数后可以被 vitest 直接覆盖（增量口径出错会红），store 只负责接线。
 *
 * **本文件是三态口径的唯一真相源**（2026-11-08 收敛完成）：状态分布饼图下线后，
 * 后端的第二套 SQL、命令与前端封装已一并删除，"两份实现各自漂移"的隐患消除。
 * 将来若要做状态统计，请在此文件补纯函数并加单测，别在 Rust 侧再写一份 CASE。
 *
 * **口径约定（改动前先看这里）**
 * - 「已通关」= `status === "completed"`（用户手动标记，唯一真值来源）。
 * - 「已游玩 / 未游玩」= 由 `play_time_seconds` 派生，与 `status` 无关
 *   （`playing` / `abandoned` 这类历史标记一律按有无时长归入这两桶）。
 * - 「收藏」是独立维度，与三态互斥：选中收藏时不再按状态过滤
 *   （与 `HomeView` 的按钮组语义一致，选收藏就只看收藏）。
 */
export type GameStatusFilter = "" | "favorites" | "unplayed" | "played" | "completed";

export type GameSortBy = "last_played" | "play_time" | "added_at" | "name";

export type SortOrder = "asc" | "desc";

/** 参与筛选/排序所需的最小字段集（便于测试构造，不要求完整 `Game`） */
export type FilterableGame = Pick<
  Game,
  | "name"
  | "name_en"
  | "status"
  | "play_time_seconds"
  | "genres"
  | "is_favorite"
  | "last_played"
  | "added_at"
>;

export interface GameFilterInput {
  search?: string;
  status?: string;
  genre?: string;
  sortBy?: string;
  sortOrder?: string;
}

/** 三态归属（收藏不参与） */
export function effectiveStatus(
  game: Pick<FilterableGame, "status" | "play_time_seconds">
): "unplayed" | "played" | "completed" {
  if (game.status === "completed") return "completed";
  return game.play_time_seconds > 0 ? "played" : "unplayed";
}

/**
 * 按状态 / 关键字 / 类型过滤。三者都为空时原样返回（不复制数组）。
 *
 * 搜索匹配**中英文名两者**：卡片展示的是 `displayName`（英文优先），
 * 若只匹配 `name`，用户照着卡片上的英文名搜就是 0 命中。
 */
export function filterGames<T extends FilterableGame>(
  games: readonly T[],
  input: Pick<GameFilterInput, "search" | "status" | "genre">
): T[] {
  let result: readonly T[] = games;

  const search = input.search?.trim().toLowerCase();
  if (search) {
    result = result.filter(
      (g) =>
        g.name.toLowerCase().includes(search) ||
        (g.name_en ?? "").toLowerCase().includes(search)
    );
  }

  if (input.status) {
    const status = input.status;
    if (status === "favorites") {
      result = result.filter((g) => g.is_favorite);
    } else if (status === "unplayed" || status === "played" || status === "completed") {
      // 三态一律走派生口径：不再有「按原始 status 字符串精确匹配」的分支
      // （那条分支在 UI 值域下永远走不到，2026-10-17 清理）。
      result = result.filter((g) => effectiveStatus(g) === status);
    }
    // 未知值 = 不过滤（防御：调用方传了脏值时不至于把列表清空）
  }

  if (input.genre) {
    const genre = input.genre.toLowerCase();
    result = result.filter((g) => g.genres.some((gr) => gr.toLowerCase().includes(genre)));
  }

  // filter 产生的已是新数组；未过滤时复制一份，保证调用方拿到可安全修改的副本
  return result === games ? games.slice() : (result as T[]);
}

/**
 * 排序（不改原数组）。
 *
 * - `last_played`（界面名「最近互动」）：排序键 = 有游玩记录取 `last_played`，
 *   从未玩过则退回 `added_at`——即「最近一次互动（玩过或入库）」。
 *   2026-11-08 由「NULL 恒排最后」改成这个样子：旧口径下新加的游戏必然沉到列表末尾，
 *   加完还得一路往下翻才看得见（老爷原话）。改成入库时间兜底后，新游戏自然落在最前面。
 * - `name`：按展示名排序（英文优先），中文用 `zh-Hans-CN` 排序规则。
 * - 时间字段是 RFC3339 字符串，同为 UTC（`chrono::Utc::now().to_rfc3339()`），
 *   字典序等价于时间序，故 `last_played` 与 `added_at` 可以放在同一根轴上比较。
 */
export function sortGames<T extends FilterableGame>(
  games: readonly T[],
  sortBy: string = "last_played",
  sortOrder: string = "desc"
): T[] {
  const order = sortOrder === "asc" ? 1 : -1;
  const result = [...games];

  result.sort((a, b) => {
    switch (sortBy) {
      case "name":
        return displayName(a).localeCompare(displayName(b), "zh-Hans-CN") * order;

      case "play_time":
        return (a.play_time_seconds - b.play_time_seconds) * order;

      case "added_at":
        return a.added_at.localeCompare(b.added_at) * order;

      case "last_played":
      default: {
        // 排序键：游过的看游玩时间，没游过的看入库时间（新加的游戏才不会沉底）
        const av = a.last_played ?? a.added_at ?? "";
        const bv = b.last_played ?? b.added_at ?? "";
        if (!av && !bv) return 0;
        if (!av) return 1;
        if (!bv) return -1;
        return av.localeCompare(bv) * order;
      }
    }
  });

  return result;
}

/** 一步到位：过滤 → 排序 */
export function filterAndSortGames<T extends FilterableGame>(
  games: readonly T[],
  input: GameFilterInput = {}
): T[] {
  return sortGames(
    filterGames(games, input),
    input.sortBy ?? "last_played",
    input.sortOrder ?? "desc"
  );
}
