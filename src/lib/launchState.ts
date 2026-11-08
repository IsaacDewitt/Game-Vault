/**
 * 启动三态状态机（纯函数，有单测守护）
 *
 * 病根（2026-11-04）：旧版 `store.launch()` 在 `launchGame()` 一返回就把游戏
 * 塞进 `activeGames`，UI 立刻显示「游玩中」。可对 Steam/Epic 来说，那一刻只是
 * 把 URI 交给了客户端，游戏可能还在下载更新、排队、甚至被取消。
 *
 * 现在以后端为唯一真相源：
 *   idle ──delegated──> launching ──updating──> updating
 *     │                    │                      │
 *     │                    │            waiting ─┘ (回落到 launching)
 *     └──spawned──> playing <──────activated──────┘
 *                    │
 *                    └──stopped──> idle
 *
 * 「启动中 / 更新中」都不计时，也不会让详情按钮变成「停止游戏」。
 * 后端补不出身份签名时返回 `untracked`：只清状态，不进入任何一态。
 *
 * 注意：后端没有单独的 `timeout` 事件类型——`platform-launch-timeout` 由
 * store 转成 `stopped` 派发（见 stores/games.ts），故 `LaunchEvent` 里
 * 也没有 timeout 变体，别照着旧注释去找。
 */

export type LaunchState = "launching" | "updating" | "playing";

/** 后端 `SessionSnapshot` 的前端镜像（字段名 camelCase） */
export interface SessionSnapshot {
  gameId: string;
  state: LaunchState;
}

/** 后端 `launch_game` 的返回：本地直启拿到 PID，平台启动只是委托，无签名则不追踪 */
export type LaunchOutcome =
  | { kind: "spawned" }
  | { kind: "delegated" }
  | { kind: "untracked" };

export type LaunchEvent =
  | { type: "spawned"; gameId: string }
  | { type: "delegated"; gameId: string }
  | { type: "untracked"; gameId: string }
  | { type: "activated"; gameIds: string[] }
  | { type: "updating"; gameId: string }
  /** 更新态结束、进程尚未现身：从「更新中」回落到「启动中」 */
  | { type: "waiting"; gameId: string }
  | { type: "stopped"; gameId: string }
  | { type: "synced"; snapshots: SessionSnapshot[] };

/** 所有状态变更的唯一入口；不修改入参，便于测试与排查 */
export function reduceLaunchStates(
  states: Record<string, LaunchState>,
  event: LaunchEvent,
): Record<string, LaunchState> {
  switch (event.type) {
    case "spawned":
      // 本地 exe 由我们亲自拉起、PID 在手 → 立即计时
      return { ...states, [event.gameId]: "playing" };
    case "delegated":
      // 平台游戏只发出启动请求，等后端确认进程现身
      return { ...states, [event.gameId]: "launching" };
    case "untracked": {
      // 进程拉起了但后端无法追踪：清掉旧状态，绝不显示「游玩中」
      if (!(event.gameId in states)) return states;
      const next = { ...states };
      delete next[event.gameId];
      return next;
    }
    case "activated": {
      const next = { ...states };
      for (const id of event.gameIds) next[id] = "playing";
      return next;
    }
    case "updating":
      return { ...states, [event.gameId]: "updating" };
    case "waiting":
      // 只回退「更新中」：已转正 / 已清掉的条目不能被一条迟到的事件打回去
      if (states[event.gameId] !== "updating") return states;
      return { ...states, [event.gameId]: "launching" };
    case "stopped": {
      if (!(event.gameId in states)) return states;
      const next = { ...states };
      delete next[event.gameId];
      return next;
    }
    case "synced": {
      // 挂载 / 重连时整体对齐后端快照，清掉前端残留的乐观状态
      const next: Record<string, LaunchState> = {};
      for (const snapshot of event.snapshots) next[snapshot.gameId] = snapshot.state;
      return next;
    }
    default:
      return states;
  }
}

/** 单取某游戏状态；供模板调用，避免每处自己查数组 */
export function stateOf(
  states: Record<string, LaunchState>,
  gameId: string,
): LaunchState | null {
  return states[gameId] ?? null;
}
