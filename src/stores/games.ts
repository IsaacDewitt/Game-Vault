import { defineStore } from "pinia";
import { ref, computed } from "vue";
import { listen } from "@tauri-apps/api/event";
import type { Game } from "../lib/tauri";
import * as api from "../lib/tauri";

export const useGamesStore = defineStore("games", () => {
  // 状态
  const games = ref<Game[]>([]);
  const loading = ref(false);
  const searchQuery = ref("");
  const selectedGame = ref<Game | null>(null);
  const activeGames = ref<string[]>([]);
  // 封面文件路径映射 (game_id -> 本地文件路径)
  const coverPaths = ref<Record<string, string>>({});
  // 封面 base64 缓存 (game_id -> data URL)
  const coverBase64Cache = ref<Record<string, string>>({});
  // 封面加载状态
  const coversLoading = ref(false);
  // 封面获取进度
  const coverFetchProgress = ref<{ current: number; total: number; game_name: string } | null>(null);
  // 游戏信息获取进度
  const gameInfoFetchProgress = ref<{ current: number; total: number; game_name: string } | null>(null);
  // 筛选状态
  const statusFilter = ref("");
  const genreFilter = ref("");
  const allGenres = ref<string[]>([]);

  // 监听后端游戏停止事件，清理 activeGames
  let unlistenGameStopped: (() => void) | null = null;
  let unlistenCoverProgress: (() => void) | null = null;
  let unlistenGameInfoProgress: (() => void) | null = null;

  async function setupEventListeners() {
    if (unlistenGameStopped) return;
    unlistenGameStopped = await listen<string>("game-stopped", (event) => {
      const gameId = event.payload;
      const idx = activeGames.value.indexOf(gameId);
      if (idx !== -1) {
        activeGames.value.splice(idx, 1);
      }
      // 刷新游戏数据以更新时长
      loadGames();
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
  }

  // 清理事件监听器（应用退出时调用）
  function cleanupEventListeners() {
    if (unlistenGameStopped) {
      unlistenGameStopped();
      unlistenGameStopped = null;
    }
    if (unlistenCoverProgress) {
      unlistenCoverProgress();
      unlistenCoverProgress = null;
    }
    if (unlistenGameInfoProgress) {
      unlistenGameInfoProgress();
      unlistenGameInfoProgress = null;
    }
  }

  // 计算属性
  const filteredGames = computed(() => {
    let result = games.value;

    // 搜索筛选
    if (searchQuery.value) {
      const query = searchQuery.value.toLowerCase();
      result = result.filter((g) => g.name.toLowerCase().includes(query));
    }

    // 状态筛选
    if (statusFilter.value) {
      if (statusFilter.value === "favorites") {
        result = result.filter((g) => g.is_favorite);
      } else {
        result = result.filter((g) => g.status === statusFilter.value);
      }
    }

    // 类型筛选
    if (genreFilter.value) {
      const genre = genreFilter.value.toLowerCase();
      result = result.filter((g) =>
        g.genres.some((gr) => gr.toLowerCase().includes(genre))
      );
    }

    return result;
  });

  // 方法
  let loadGamesLock = false;
  async function loadGames() {
    if (loadGamesLock) return;
    loadGamesLock = true;
    loading.value = true;
    try {
      games.value = await api.getGames({
        sort_by: "last_played",
        sort_order: "desc",
      });
      await loadAllCovers();
      await loadAllGenres();
    } catch (e) {
      console.error("加载游戏列表失败:", e);
    } finally {
      loading.value = false;
      loadGamesLock = false;
    }
    // 后台刷新 exe 版本号（不阻塞 UI）
    // 注意：不重新获取整个列表，避免覆盖用户在两次请求之间的操作
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
      // 批量加载所有封面的 base64 数据
      await loadCoversBatch();
    } catch (e) {
      console.error("加载封面路径失败:", e);
    }
  }

  async function loadCoversBatch() {
    const paths = Object.values(coverPaths.value);
    if (paths.length === 0) {
      coverBase64Cache.value = {};
      return;
    }

    coversLoading.value = true;
    try {
      const result = await api.readCoversBatchAsBase64(paths);
      // 将路径映射转换为 game_id 映射
      const cache: Record<string, string> = {};
      for (const [gameId, filePath] of Object.entries(coverPaths.value)) {
        if (result[filePath]) {
          cache[gameId] = result[filePath];
        }
      }
      coverBase64Cache.value = cache;
    } catch (e) {
      console.error("批量加载封面失败:", e);
    } finally {
      coversLoading.value = false;
    }
  }

  async function addGameManual(name: string, exePath: string) {
    try {
      await api.addGameManual(name, exePath);
      await loadGames();
      fetchCovers().catch(() => {});
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
    return updated;
  }

  async function fetchCovers() {
    try {
      coverFetchProgress.value = null;
      const result = await api.fetchMissingCovers();
      coverFetchProgress.value = null;
      if (result.fetched > 0) {
        await loadGames();
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
    } catch (e) {
      console.error("从 URL 设置封面失败:", e);
      throw e;
    }
  }

  async function launch(gameId: string) {
    try {
      await api.launchGame(gameId);
      if (!activeGames.value.includes(gameId)) {
        activeGames.value.push(gameId);
      }
    } catch (e) {
      console.error("启动游戏失败:", e);
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

  async function removeGame(gameId: string) {
    try {
      await api.deleteGame(gameId);
      games.value = games.value.filter((g) => g.id !== gameId);
      if (selectedGame.value?.id === gameId) {
        selectedGame.value = null;
      }
      // 清理封面缓存
      delete coverPaths.value[gameId];
      delete coverBase64Cache.value[gameId];
    } catch (e) {
      console.error("删除游戏失败:", e);
      throw e;  // 向上传播错误，让调用方可以提示用户
    }
  }

  async function renameGame(gameId: string, newName: string) {
    try {
      await api.renameGame(gameId, newName);
      const idx = games.value.findIndex((g) => g.id === gameId);
      if (idx !== -1) {
        games.value[idx].name = newName;
      }
      if (selectedGame.value?.id === gameId) {
        selectedGame.value.name = newName;
      }
    } catch (e) {
      console.error("重命名失败:", e);
      throw e;
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
    } catch (e) {
      console.error("设置游戏状态失败:", e);
      throw e;
    }
  }

  function selectGame(game: Game) {
    selectedGame.value = game;
  }

  function clearSelection() {
    selectedGame.value = null;
  }

  return {
    games,
    loading,
    searchQuery,
    selectedGame,
    activeGames,
    coverPaths,
    coverBase64Cache,
    coversLoading,
    coverFetchProgress,
    gameInfoFetchProgress,
    statusFilter,
    genreFilter,
    allGenres,
    filteredGames,
    loadGames,
    loadAllCovers,
    loadAllGenres,
    addGameManual,
    fetchGameInfoLlm,
    fetchCovers,
    fetchGameInfo,
    refreshCover,
    setCover,
    fetchCoverOptions,
    setCoverFromUrl,
    launch,
    toggleFav,
    removeGame,
    renameGame,
    updateExePath,
    setGameStatus,
    selectGame,
    clearSelection,
    setupEventListeners,
    cleanupEventListeners,
  };
});
