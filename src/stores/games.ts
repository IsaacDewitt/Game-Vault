import { defineStore } from "pinia";
import { ref, computed } from "vue";
import { listen } from "@tauri-apps/api/event";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { Game } from "../lib/tauri";
import * as api from "../lib/tauri";
import { filterAndSortGames } from "../lib/gameFilter";
import {
  reduceLaunchStates,
  stateOf,
  type LaunchEvent,
  type LaunchState,
} from "../lib/launchState";

/**
 * 卡片展示名：英文名优先（与手账 `reviews.ts` 同口径）。
 *
 * 实现已下沉到 `lib/format.ts`——`lib/gameFilter.ts` 的排序要用它，
 * 若留在这里会形成 store ↔ lib 的循环依赖。此处只做 re-export，
 * 保持 `GameCard.vue` 等既有导入路径不变。
 */
export { displayName } from "../lib/format";

export const useGamesStore = defineStore("games", () => {
  // 状态
  const games = ref<Game[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);
  const searchQuery = ref("");
  const selectedGame = ref<Game | null>(null);

  // 启动三态（2026-11-04）：后端是唯一真相源，前端不再「点完就当在玩」。
  // 所有变更只走 `lib/launchState.ts` 的纯 reducer，便于测试与复盘。
  const gameStates = ref<Record<string, LaunchState>>({});
  /** 兼容既有模板：仅「正式计时中」的游戏 */
  const activeGames = computed(() =>
    Object.keys(gameStates.value).filter((id) => gameStates.value[id] === "playing"),
  );
  /** 已委托客户端、尚未开始计时的游戏（启动中 / 更新中） */
  const launchingGames = computed(() =>
    Object.keys(gameStates.value).filter((id) => gameStates.value[id] !== "playing"),
  );
  function launchState(gameId: string): LaunchState | null {
    return stateOf(gameStates.value, gameId);
  }
  function dispatchLaunch(event: LaunchEvent) {
    gameStates.value = reduceLaunchStates(gameStates.value, event);
  }
  /** 启动超时提示：store 只记事实，toast 交给 HomeView（store 不碰 UI API） */
  const launchNotice = ref<{ gameId: string; kind: "timeout" } | null>(null);
  function clearLaunchNotice() {
    launchNotice.value = null;
  }
  // 封面路径映射 (game_id -> { main, thumb })，含已移除游戏的留档封面
  const coverPaths = ref<Record<string, api.CoverSet>>({});
  // 封面获取进度
  const coverFetchProgress = ref<{ current: number; total: number; game_name: string } | null>(null);
  // 游戏信息获取进度
  const gameInfoFetchProgress = ref<{ current: number; total: number; game_name: string } | null>(null);
  // 筛选状态
  const statusFilter = ref("");
  const genreFilter = ref("");
  const allGenres = ref<string[]>([]);
  // 排序状态（默认与旧行为一致：最近游玩优先）
  const sortBy = ref("last_played");
  const sortOrder = ref("desc");
  // 存档路径检查结果 (game_id -> 是否存在)
  const savePathStatus = ref<Record<string, boolean>>({});
  const checkingSavePaths = ref(false);

  // 会话状态事件统一挂在全局 store（此前 platform-* 挂在 HomeView，
  // 一旦视图生命周期变化就会丢事件 → 状态永远清不掉）
  let unlistenGameStopped: (() => void) | null = null;
  let unlistenSessionSettled: (() => void) | null = null;
  let unlistenPlatformStarted: (() => void) | null = null;
  let unlistenPlatformUpdating: (() => void) | null = null;
  let unlistenPlatformWaiting: (() => void) | null = null;
  let unlistenPlatformTimeout: (() => void) | null = null;
  let unlistenCoverProgress: (() => void) | null = null;
  let unlistenGameInfoProgress: (() => void) | null = null;
  let unlistenCoversUpgraded: (() => void) | null = null;
  let listenersReady = false;

  /** 挂载 / 重连时整体对齐后端会话快照（后端是唯一真相源） */
  async function syncLaunchStates() {
    try {
      const snapshots = await api.getActiveSessions();
      dispatchLaunch({ type: "synced", snapshots });
    } catch (e) {
      console.error("同步后端会话状态失败:", e);
    }
  }

  async function setupEventListeners() {
    if (listenersReady) {
      await syncLaunchStates();
      return;
    }
    unlistenGameStopped = await listen<string>("game-stopped", (event) => {
      dispatchLaunch({ type: "stopped", gameId: event.payload });
      // 刷新游戏数据以更新时长。
      // 轻量模式：游戏退出不会增删游戏/封面/类型，跳过全量封面与类型重扫
      loadGames({ light: true });
    });

    // 重复启动顶掉旧会话：只刷新时长，绝不改三态（新会话刚刚建好）
    unlistenSessionSettled = await listen<string>("game-session-settled", () => {
      loadGames({ light: true });
    });

    // 平台游戏待命转正：只有后端真正发现游戏进程，才升为「游玩中」
    unlistenPlatformStarted = await listen<string[]>("platform-game-started", (event) => {
      dispatchLaunch({ type: "activated", gameIds: event.payload });
    });
    // Steam 客户端明确报告 Updating=1：显示「更新中」，绝不显示「游玩中」
    unlistenPlatformUpdating = await listen<string>("platform-launch-updating", (event) => {
      dispatchLaunch({ type: "updating", gameId: event.payload });
    });
    // 更新结束但进程还没起：把角标从「更新中」回落到「启动中」
    unlistenPlatformWaiting = await listen<string>("platform-launch-waiting", (event) => {
      dispatchLaunch({ type: "waiting", gameId: event.payload });
    });
    // 启动超时 / 取消：清回 idle，并让视图弹一条说明
    unlistenPlatformTimeout = await listen<string>("platform-launch-timeout", (event) => {
      dispatchLaunch({ type: "stopped", gameId: event.payload });
      launchNotice.value = { gameId: event.payload, kind: "timeout" };
    });

    // 监听封面获取进度事件
    unlistenCoverProgress = await listen<{ current: number; total: number; game_name: string }>(
      "cover-fetch-progress",
      (event) => {
        coverFetchProgress.value = event.payload;
      }
    );

    // 监听游戏信息获取进度事件
    unlistenGameInfoProgress = await listen<{ current: number; total: number; game_name: string }>(
      "game-info-fetch-progress",
      (event) => {
        gameInfoFetchProgress.value = event.payload;
      }
    );

    // 后端缩略图规格升级（重建老缩略图）完成 → 封面行的破缓存时间戳已被推进，
    // 前端手里的映射还是旧的（URL 不变 = 显示缓存里的旧图），必须重拉一次。
    unlistenCoversUpgraded = await listen<number>("covers-upgraded", (event) => {
      console.info(`[Cover] 缩略图规格升级完成，重建 ${event.payload} 张，刷新封面映射`);
      loadAllCovers().catch(() => {});
    });

    listenersReady = true;
    await syncLaunchStates();
  }

  // 清理事件监听器（应用退出时调用）
  function cleanupEventListeners() {
    unlistenGameStopped?.();
    unlistenGameStopped = null;
    unlistenSessionSettled?.();
    unlistenSessionSettled = null;
    unlistenPlatformStarted?.();
    unlistenPlatformStarted = null;
    unlistenPlatformUpdating?.();
    unlistenPlatformUpdating = null;
    unlistenPlatformWaiting?.();
    unlistenPlatformWaiting = null;
    unlistenPlatformTimeout?.();
    unlistenPlatformTimeout = null;
    unlistenCoverProgress?.();
    unlistenCoverProgress = null;
    unlistenGameInfoProgress?.();
    unlistenGameInfoProgress = null;
    unlistenCoversUpgraded?.();
    unlistenCoversUpgraded = null;
    listenersReady = false;
  }

  // 计算属性：筛选 + 排序统一走 lib/gameFilter.ts（纯函数，有单测守护）
  // 三态口径：已通关 = status === "completed"（用户手动标记）；
  // 已游玩 / 未游玩 = 由 play_time_seconds 派生；后端第二套 CASE 已删除，别再提它。
  // 原实现在这里内联了一份同义逻辑，且末尾留着一个 UI 值域下永远走不到的
  // 「按原始 status 精确匹配」分支——2026-10-17 随抽离一并清理。
  const filteredGames = computed(() =>
    filterAndSortGames(games.value, {
      search: searchQuery.value,
      status: statusFilter.value,
      genre: genreFilter.value,
      sortBy: sortBy.value,
      sortOrder: sortOrder.value,
    })
  );

  // 方法
  let loadGamesLock = false;
  let pendingRefresh = false;
  /** 拉取游戏列表。light=true 时跳过封面/类型全量重扫（仅时长等字段变化时用） */
  async function loadGames(opts?: { light?: boolean }) {
    if (loadGamesLock) {
      pendingRefresh = true;
      return;
    }
    loadGamesLock = true;
    loading.value = true;
    error.value = null;
    try {
      games.value = await api.getGames({
        sort_by: "last_played",
        sort_order: "desc",
      });
      if (!opts?.light) {
        await loadAllCovers();
        await loadAllGenres();
      }
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      error.value = `加载游戏列表失败: ${msg}`;
      console.error("加载游戏列表失败:", e);
    } finally {
      loading.value = false;
      loadGamesLock = false;
      // 如果在加载期间有新的刷新请求，执行一次补刷新
      if (pendingRefresh) {
        pendingRefresh = false;
        loadGames();
      }
    }
    // 后台刷新 exe 版本号（节流：5 分钟内最多执行一次，不阻塞 UI）
    // 注意：不重新获取整个列表，避免覆盖用户在两次请求之间的操作
    throttledRefreshExeVersions();
  }

  // exe 版本刷新节流（5 分钟），避免每次 loadGames 都触发全量 stat
  let lastVersionRefreshAt = 0;
  const VERSION_REFRESH_INTERVAL_MS = 5 * 60 * 1000;
  function throttledRefreshExeVersions() {
    const now = Date.now();
    if (now - lastVersionRefreshAt < VERSION_REFRESH_INTERVAL_MS) return;
    lastVersionRefreshAt = now;
    api.refreshExeVersions().then(async (updated) => {
      if (updated > 0) {
        // 仅更新有变化的游戏的 exe_version 字段，而非替换整个列表
        try {
          const freshGames = await api.getGames({
            sort_by: "last_played",
            sort_order: "desc",
          });
          for (const fresh of freshGames) {
            const existing = games.value.find((g) => g.id === fresh.id);
            if (existing && existing.exe_version !== fresh.exe_version) {
              existing.exe_version = fresh.exe_version;
            }
          }
        } catch (_) {}
      }
    }).catch(() => {});
  }

  /** 静默触发成就检测（数据变更后调用，由后端事件负责弹通知） */
  function checkAchievementsQuietly() {
    api.checkAchievements().catch(() => {});
  }

  async function loadAllGenres() {
    try {
      allGenres.value = await api.getAllGenres();
    } catch (e) {
      console.error("加载游戏类型失败:", e);
    }
  }

  async function loadAllCovers() {
    try {
      coverPaths.value = await api.getAllCovers();
    } catch (e) {
      console.error("加载封面路径失败:", e);
    }
  }

  /**
   * 将封面路径转为 asset URL（Tauri asset 协议直接加载，避免全量 base64 传输）。
   * preferThumb=true 时优先用缩略图（卡片网格/统计排行等小尺寸场景，避免整张原图参与解码）；
   * 缩略图缺失时自动回退原图。
   *
   * **必须拼破缓存参数**（2026-09-24 修）：封面文件名恒为 `<game_id>.<ext>`，换封面是
   * 原地覆盖同名文件 → asset URL 逐字节不变 → WebView2 命中图片缓存继续显示旧图
   * （老爷实测：点开卡片换封面，关闭后卡片纹丝不动）。
   *
   * 参数优先取**封面行**的 `updated_at`（只有封面真换了才变）；取不到时（旧数据/
   * 索引兜底路径）回退条目自身的 `updated_at`——它偏"吵"（涨时长/改评分都会动，
   * 会白烧一次解码），但至少保证换完封面能立刻看到新图。
   */
  function coverSrc(gameId: string, preferThumb = true): string | null {
    const entry = coverPaths.value[gameId];
    if (!entry) return null;
    const p = (preferThumb ? entry.thumb : entry.main) || entry.main || entry.thumb;
    if (!p) return null;
    try {
      const url = convertFileSrc(p);
      const t = entry.updated_at || games.value.find((g) => g.id === gameId)?.updated_at || gameId;
      return `${url}${url.includes("?") ? "&" : "?"}t=${encodeURIComponent(t)}`;
    } catch (e) {
      console.error("转换封面路径失败:", gameId, e);
      return null;
    }
  }

  /**
   * 手动添加游戏：**纯本地入库，不发任何网络请求**（2026-11-08 按老爷要求改）。
   *
   * 两段自动化都是被老爷逐次砍掉的，别再好心补回来：
   * - 2026-10-30 砍 `fetchCovers()`：它是个「批量补全库内**所有**缺失封面」的动作，
   *   加一个游戏会把整库扫一遍，图和这条新条目还未必对得上。
   * - 2026-11-08 砍 `fetchGameInfoLlm()`：加游戏就该是"填进去即完事"，
   *   不该在后台偷偷发一次 LLM 请求，还顺手把用户刚敲的名字改写一遍。
   *
   * 补信息 / 补封面全部改为手动入口：详情页的「获取信息」「更换封面」，
   * 侧边栏的「刷新游戏信息 / 刷新封面」。本函数只保证入库 + 刷新内存列表 + 本地成就。
   * 返回入库后的条目，调用方据此立刻给出"已添加"反馈。
   */
  async function addGameManual(name: string, exePath: string) {
    try {
      const game = await api.addGameManual(name, exePath);
      await loadGames();
      checkAchievementsQuietly();
      return game;
    } catch (e) {
      console.error("添加游戏失败:", e);
      throw e;
    }
  }

  async function fetchGameInfoLlm(gameId: string) {
    const updated = await api.fetchGameInfoLlm(gameId);
    const idx = games.value.findIndex((g) => g.id === gameId);
    if (idx !== -1) {
      games.value[idx] = updated;
    }
    if (selectedGame.value?.id === gameId) {
      selectedGame.value = updated;
    }
    checkAchievementsQuietly();
    return updated;
  }

  async function updateGameMeta(gameId: string, meta: api.GameMetaInput) {
    const updated = await api.updateGameMeta(gameId, meta);
    const idx = games.value.findIndex((g) => g.id === gameId);
    if (idx !== -1) {
      games.value[idx] = updated;
    }
    if (selectedGame.value?.id === gameId) {
      selectedGame.value = updated;
    }
    checkAchievementsQuietly();
    return updated;
  }

  async function fetchCovers() {
    try {
      coverFetchProgress.value = null;
      const result = await api.fetchMissingCovers();
      coverFetchProgress.value = null;
      if (result.fetched > 0) {
        await loadGames();
        checkAchievementsQuietly();
      }
      if (result.errors.length > 0) {
        console.warn("封面获取:", result.errors);
      }
      return result;
    } catch (e) {
      coverFetchProgress.value = null;
      console.error("获取封面失败:", e);
      throw e;
    }
  }

  async function fetchGameInfo() {
    try {
      gameInfoFetchProgress.value = null;
      const result = await api.fetchMissingGameInfo();
      gameInfoFetchProgress.value = null;
      if (result.fetched > 0) {
        await loadGames();
        checkAchievementsQuietly();
      }
      if (result.errors.length > 0) {
        console.warn("游戏信息获取:", result.errors);
      }
      return result;
    } catch (e) {
      gameInfoFetchProgress.value = null;
      console.error("获取游戏信息失败:", e);
      throw e;
    }
  }

  async function refreshCover(gameId: string) {
    try {
      // 重新获取单个游戏的数据以更新封面
      const game = await api.getGameDetail(gameId);
      if (game) {
        const idx = games.value.findIndex((g) => g.id === gameId);
        if (idx !== -1) {
          games.value[idx] = game;
        }
      }
      // 同时刷新封面路径
      await loadAllCovers();
    } catch (e) {
      console.error("刷新封面失败:", e);
    }
  }

  async function setCover(gameId: string, coverPath: string) {
    try {
      await api.setGameCover(gameId, coverPath);
      await loadGames();
      checkAchievementsQuietly();
    } catch (e) {
      console.error("设置封面失败:", e);
      throw e;
    }
  }

  async function fetchCoverOptions(gameId: string): Promise<api.CoverOption[]> {
    return api.fetchCoverOptions(gameId);
  }

  async function setCoverFromUrl(gameId: string, url: string) {
    try {
      await api.setGameCoverFromUrl(gameId, url);
      await loadGames();
      checkAchievementsQuietly();
    } catch (e) {
      console.error("从 URL 设置封面失败:", e);
      throw e;
    }
  }

  /** 用 Steam 官方封面（appid 直取 Valve 图床；识别不到 appid 会抛错） */
  async function setCoverFromSteam(gameId: string) {
    try {
      await api.setGameCoverFromSteam(gameId);
      await loadGames();
      checkAchievementsQuietly();
    } catch (e) {
      console.error("设置 Steam 官方封面失败:", e);
      throw e;
    }
  }

  async function launch(gameId: string) {
    try {
      const outcome = await api.launchGame(gameId);
      // 本地直启：PID 在手，立即计时；平台委托：只标「启动中」，
      // 等后端 platform-game-started 才升「游玩中」（Steam 更新期尤其关键）。
      // untracked：后端连 exe_path 都补不出身份签名，绝不能显示游玩中。
      switch (outcome.kind) {
        case "delegated":
          dispatchLaunch({ type: "delegated", gameId });
          break;
        case "untracked":
          dispatchLaunch({ type: "untracked", gameId });
          break;
        default:
          dispatchLaunch({ type: "spawned", gameId });
      }
    } catch (e) {
      console.error("启动游戏失败:", e);
      throw e;
    }
  }

  /** 停止计时（不关闭游戏进程）：立即清状态，后端 game-stopped 事件再来一次也幂等 */
  async function stop(gameId: string) {
    try {
      const result = await api.stopGame(gameId);
      dispatchLaunch({ type: "stopped", gameId });
      return result;
    } catch (e) {
      console.error("停止游戏失败:", e);
      throw e;
    }
  }

  async function toggleFav(gameId: string) {
    try {
      const isFav = await api.toggleFavorite(gameId);
      const game = games.value.find((g) => g.id === gameId);
      if (game) {
        game.is_favorite = isFav;
      }
    } catch (e) {
      console.error("切换收藏失败:", e);
    }
  }

  async function removeGame(gameId: string, keepCover = true) {
    try {
      await api.deleteGame(gameId, keepCover);
      games.value = games.value.filter((g) => g.id !== gameId);
      if (selectedGame.value?.id === gameId) {
        selectedGame.value = null;
      }
      // 保留封面时它进了 archive，全量刷新一次封面映射即可（时长排行要用）；
      // 不保留时直接摘掉本地缓存
      if (keepCover) {
        loadAllCovers().catch(() => {});
      } else {
        delete coverPaths.value[gameId];
      }
    } catch (e) {
      console.error("删除游戏失败:", e);
      throw e;  // 向上传播错误，让调用方可以提示用户
    }
  }

  /**
   * 重命名游戏（中文名入口）。
   *
   * 后端改名时会顺带做语言归位（纯 ASCII 名补 `name_en`、中文名清 `name_en`），
   * 故这里必须拿返回的完整 `Game` 把 `name` 与 `name_en` **一并**写回——
   * 只同步 `name` 会让卡片（英文名优先）立刻退回中文名，看着像没生效
   * （2026-10-05 修复）。
   */
  async function renameGame(gameId: string, newName: string): Promise<Game> {
    try {
      const updated = await api.renameGame(gameId, newName);
      const idx = games.value.findIndex((g) => g.id === gameId);
      if (idx !== -1) {
        games.value[idx] = updated;
      }
      if (selectedGame.value?.id === gameId) {
        selectedGame.value = updated;
      }
      return updated;
    } catch (e) {
      console.error("重命名失败:", e);
      throw e;  // 向上传播错误，让调用方可以提示用户
    }
  }

  async function updateExePath(gameId: string, newExePath: string) {
    try {
      const updated = await api.updateExePath(gameId, newExePath);
      const idx = games.value.findIndex((g) => g.id === gameId);
      if (idx !== -1) {
        games.value[idx] = updated;
      }
      if (selectedGame.value?.id === gameId) {
        selectedGame.value = updated;
      }
      checkAchievementsQuietly();
      return updated;
    } catch (e) {
      console.error("更新可执行文件路径失败:", e);
      throw e;
    }
  }

  async function setGameStatus(gameId: string, status: string) {
    try {
      await api.setGameStatus(gameId, status);
      const game = games.value.find((g) => g.id === gameId);
      if (game) {
        game.status = status;
      }
      if (selectedGame.value?.id === gameId) {
        selectedGame.value.status = status;
      }
      checkAchievementsQuietly();
    } catch (e) {
      console.error("设置游戏状态失败:", e);
      throw e;
    }
  }

  async function checkSavePaths() {
    checkingSavePaths.value = true;
    try {
      savePathStatus.value = await api.checkSavePaths();
    } catch (e) {
      console.error("检查存档路径失败:", e);
    } finally {
      checkingSavePaths.value = false;
    }
  }

  /** 更新单个游戏的存档路径检查状态（仅检查该游戏，避免全量扫描） */
  async function updateSingleSavePathStatus(gameId: string) {
    try {
      savePathStatus.value[gameId] = await api.checkSavePathsForGame(gameId);
    } catch (e) {
      console.error("更新存档路径状态失败:", e);
    }
  }

  function selectGame(game: Game) {
    selectedGame.value = game;
  }

  function clearSelection() {
    selectedGame.value = null;
  }

  /** 更新 store 中指定游戏的数据（避免组件直接操作 store 内部状态） */
  function updateGameInStore(gameId: string, updatedGame: Game) {
    const idx = games.value.findIndex((g) => g.id === gameId);
    if (idx !== -1) {
      games.value[idx] = updatedGame;
    }
    if (selectedGame.value?.id === gameId) {
      selectedGame.value = updatedGame;
    }
  }

  return {
    games,
    loading,
    error,
    searchQuery,
    selectedGame,
    activeGames,
    launchingGames,
    gameStates,
    launchState,
    launchNotice,
    clearLaunchNotice,
    coverPaths,
    coverSrc,
    coverFetchProgress,
    gameInfoFetchProgress,
    statusFilter,
    genreFilter,
    sortBy,
    sortOrder,
    allGenres,
    savePathStatus,
    checkingSavePaths,
    filteredGames,
    loadGames,
    loadAllCovers,
    loadAllGenres,
    addGameManual,
    fetchGameInfoLlm,
    updateGameMeta,
    fetchCovers,
    fetchGameInfo,
    refreshCover,
    setCover,
    fetchCoverOptions,
    setCoverFromUrl,
    setCoverFromSteam,
    launch,
    stop,
    toggleFav,
    removeGame,
    renameGame,
    updateExePath,
    setGameStatus,
    checkSavePaths,
    updateSingleSavePathStatus,
    selectGame,
    clearSelection,
    updateGameInStore,
    setupEventListeners,
    cleanupEventListeners,
  };
});
