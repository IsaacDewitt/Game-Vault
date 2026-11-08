import { describe, it, expect } from "vitest";
import { reduceLaunchStates, stateOf, type LaunchState } from "./launchState";

describe("reduceLaunchStates", () => {
  it("本地直启立刻进入游玩中，平台委托只进入启动中", () => {
    const spawned = reduceLaunchStates({}, { type: "spawned", gameId: "local" });
    expect(stateOf(spawned, "local")).toBe("playing");

    const delegated = reduceLaunchStates({}, { type: "delegated", gameId: "steam" });
    expect(stateOf(delegated, "steam")).toBe("launching");
    expect(stateOf(delegated, "steam")).not.toBe("playing");
  });

  it("untracked 清掉旧状态，不能被误显示成游玩中", () => {
    const states = reduceLaunchStates({}, { type: "spawned", gameId: "orphan" });
    const next = reduceLaunchStates(states, { type: "untracked", gameId: "orphan" });
    expect(stateOf(next, "orphan")).toBeNull();
    expect(reduceLaunchStates({}, { type: "untracked", gameId: "none" })).toEqual({});
  });

  it("Steam 更新中显示更新态，转正后才变成游玩中", () => {
    let states: Record<string, LaunchState> = reduceLaunchStates(
      {},
      { type: "delegated", gameId: "pbz" },
    );
    states = reduceLaunchStates(states, { type: "updating", gameId: "pbz" });
    expect(stateOf(states, "pbz")).toBe("updating");

    states = reduceLaunchStates(states, { type: "activated", gameIds: ["pbz"] });
    expect(stateOf(states, "pbz")).toBe("playing");
  });

  it("更新结束后回落到启动中，迟到的 waiting 不能把已转正的游戏打回去", () => {
    let states: Record<string, LaunchState> = reduceLaunchStates(
      {},
      { type: "updating", gameId: "pbz" },
    );
    states = reduceLaunchStates(states, { type: "waiting", gameId: "pbz" });
    expect(stateOf(states, "pbz")).toBe("launching");

    // 同一轮的 playing / 未知 id 都不受影响
    const playing = reduceLaunchStates(
      { a: "playing" } as Record<string, LaunchState>,
      { type: "waiting", gameId: "a" },
    );
    expect(stateOf(playing, "a")).toBe("playing");
    expect(reduceLaunchStates(states, { type: "waiting", gameId: "nope" })).toBe(states);
  });

  it("超时 / 停止 / 退出都清回 idle，且互不干扰其他游戏", () => {
    let states = reduceLaunchStates({}, { type: "spawned", gameId: "a" });
    states = reduceLaunchStates(states, { type: "delegated", gameId: "b" });
    states = reduceLaunchStates(states, { type: "stopped", gameId: "a" });

    expect(stateOf(states, "a")).toBeNull();
    expect(stateOf(states, "b")).toBe("launching");
    // 清理不存在的 id 不应产生新对象引用（避免无意义重渲染）
    expect(reduceLaunchStates(states, { type: "stopped", gameId: "nope" })).toBe(states);
  });

  it("synced 整体对齐后端快照，丢弃前端残留的乐观状态", () => {
    const stale = reduceLaunchStates({}, { type: "spawned", gameId: "ghost" });
    const synced = reduceLaunchStates(stale, {
      type: "synced",
      snapshots: [
        { gameId: "a", state: "launching" },
        { gameId: "b", state: "updating" },
        { gameId: "c", state: "playing" },
      ],
    });
    expect(synced).toEqual({
      a: "launching",
      b: "updating",
      c: "playing",
    });
    expect(stateOf(synced, "ghost")).toBeNull();
  });

  it("纯函数：不修改入参", () => {
    const before: Record<string, LaunchState> = { x: "launching" };
    const after = reduceLaunchStates(before, { type: "activated", gameIds: ["x"] });
    expect(before).toEqual({ x: "launching" });
    expect(after).not.toBe(before);
    expect(stateOf(after, "x")).toBe("playing");
  });
});
