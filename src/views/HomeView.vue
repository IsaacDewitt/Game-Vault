<script setup lang="ts">
import {
  NSpace,
  NInput,
  NButton,
  NSpin,
  NEmpty,
  NModal,
  NProgress,
  useMessage,
  useDialog,
} from "naive-ui";
import { ref, computed, onMounted, onUnmounted, watch, h } from "vue";
import { useDebounceFn } from "@vueuse/core";
import { listen } from "@tauri-apps/api/event";
import { useGamesStore } from "../stores/games";
import * as api from "../lib/tauri";
import { DEBOUNCE_MS } from "../lib/constants";
import { openAddGame } from "../lib/addGameFlow";
import { useRefreshActions } from "../lib/refreshActions";
import GameCard from "../components/GameCard.vue";
import GameDetail from "../components/GameDetail.vue";
import GameInfoEditModal from "../components/GameInfoEditModal.vue";
import LaunchArgsModal from "../components/LaunchArgsModal.vue";
import ContextMenu from "../components/ContextMenu.vue";
import type { ContextMenuItem } from "../components/ContextMenu.vue";
import { formatPlayTime } from "../lib/format";

const store = useGamesStore();
const message = useMessage();
const dialog = useDialog();
// 刷新动作与侧边栏共用同一份实现（含 loading 状态），右键菜单仍保留入口
const { refreshCovers, refreshAllInfo } = useRefreshActions();

// 监听截图结果事件（全局热键触发截图后由后端推送）
let unlistenScreenshot: (() => void) | null = null;
// 平台游戏的三态事件（转正 / 更新中 / 超时）已上移到全局 store：
// 挂在视图里一旦视图生命周期变化就会丢事件，状态永远清不掉。
// 此处只负责把 store 记下的超时事实弹成一条说明。
// 【2026-11-08】watcher 必须在 `<script setup>` 顶层同步作用域创建：
// 旧版放在 `onMounted(async ...)` 的 `await listen(...)` 之后，此时
// `getCurrentScope()` 已复位，watcher 不挂到组件 effect scope，视图一旦
// 被销毁重建（keep-alive 加 max / 条件渲染）就会残留旧 watcher 重复弹提示。
watch(
  () => store.launchNotice,
  (notice) => {
    if (!notice || notice.kind !== "timeout") return;
    const name = store.games.find((g) => g.id === notice.gameId)?.name ?? "该游戏";
    message.warning(
      `《${name}》未启动成功：等待期间客户端没有拉起游戏进程（可能已取消更新、未登录或启动被中断），本次不计入时长`,
      { duration: 8000 }
    );
    store.clearLaunchNotice();
  }
);

onMounted(async () => {
  // 筛选弹层点外部关闭（挂在 document 上，避免全屏遮罩挡住卡片点击）
  document.addEventListener("mousedown", handleDocMouseDown);

  try {
    unlistenScreenshot = await listen<api.ScreenshotOutcome>("screenshot-taken", (event) => {
      const o = event.payload;
      if (o.outcome === "captured") {
        message.success(`截图已保存到「${o.process_name}」文件夹`);
      } else if (o.outcome === "failed") {
        message.error(`截图失败: ${o.message}`);
      } else if (o.outcome === "no_active_game") {
        message.warning("当前没有从本库启动的游戏在运行，未截图");
      } else if (o.outcome === "foreground_mismatch") {
        message.warning("前台窗口不是从本库启动的游戏，未截图");
      } else if (o.outcome === "no_foreground_window") {
        // 与上一条区别：不是"在别的程序里"，而是系统层面前台窗口为空
        // （全屏独占切换期等瞬态）。多数情况下再按一次即可成功。
        message.warning("未能识别当前前台窗口（系统瞬时无前台窗口），未截图，请再试一次");
      }
    });
  } catch (e) {
    console.error("监听截图事件失败:", e);
  }

  // 检测截图热键是否注册成功（如 F12 被 Steam 等程序占用，启动时提示换键）
  try {
    const hotkeyError = await api.getScreenshotHotkeyStatus();
    if (hotkeyError) {
      message.error(
        `截图快捷键注册失败：${hotkeyError}。该快捷键可能被其他程序（如 Steam）占用，请到「设置 → 截图设置」更换。`,
        { duration: 8000 }
      );
    }
  } catch (e) {
    console.error("检测截图热键状态失败:", e);
  }
});
onUnmounted(() => {
  document.removeEventListener("mousedown", handleDocMouseDown);
  if (unlistenScreenshot) {
    unlistenScreenshot();
    unlistenScreenshot = null;
  }
});

// 重命名游戏弹窗状态
const showRenameModal = ref(false);
const renamingGameId = ref("");
const renameInput = ref("");
// 手动填写游戏信息弹窗状态
const showEditInfoModal = ref(false);
const editingGameId = ref("");
const editingGame = computed(() =>
  editingGameId.value ? store.games.find((g) => g.id === editingGameId.value) ?? null : null
);
// 启动参数弹窗状态（卡片右键菜单入口）
const showLaunchArgsModal = ref(false);
const launchArgsGameId = ref("");
const launchArgsGame = computed(() =>
  launchArgsGameId.value ? store.games.find((g) => g.id === launchArgsGameId.value) ?? null : null
);
// 主页右键菜单状态
const showHomeContextMenu = ref(false);
const homeContextMenuX = ref(0);
const homeContextMenuY = ref(0);

// 搜索框本地值（防抖后才写进 store，避免每次按键重算整个列表）
const searchInput = ref(store.searchQuery);
const applySearch = useDebounceFn((value: string) => {
  store.searchQuery = value;
}, DEBOUNCE_MS);
watch(searchInput, (value) => applySearch(value));

function clearSearch() {
  searchInput.value = "";
  store.searchQuery = "";
}

// 状态筛选（单选，点亮的项再点一次即取消 = 全部）
// 三态模型：未游玩（从未启动）/ 已游玩（启动过）/ 已通关（用户手动标记）。
// 是否「玩过」用 play_time_seconds 判断、是否「通关」用 status=completed 判断，
// 口径唯一真相源是 lib/gameFilter.ts；收藏是独立维度，与三态互斥（选中收藏即不再按状态过滤）。
// 2026-11-08：主页顶部那排按钮收进「筛选」弹层，值域与语义完全不变，只是不再占横排空间。
const statusChipOptions = [
  { label: "收藏", value: "favorites" },
  { label: "未游玩", value: "unplayed" },
  { label: "已游玩", value: "played" },
  { label: "已通关", value: "completed" },
];

function toggleStatus(value: string) {
  store.statusFilter = store.statusFilter === value ? "" : value;
}

// 类型筛选：同样单选，点亮的项再点一次取消
function toggleGenre(genre: string) {
  store.genreFilter = store.genreFilter === genre ? "" : genre;
}

// 排序选项：与手账（ReviewsView）同款两段式——排序字段 + 升降序。
// 排序在前端内存里做（lib/gameFilter.ts），切排序不重新拉库，即时生效。
// 「最近游玩」+ 降序 = 旧行为的默认值，故升级后首屏顺序不变。
const sortOptions = [
  { label: "最近游玩", value: "last_played" },
  { label: "游玩时长", value: "play_time" },
  { label: "添加时间", value: "added_at" },
  { label: "名称", value: "name" },
];

// 「筛选」弹层：点外部关闭
const filterOpen = ref(false);
const filterPopRef = ref<HTMLElement | null>(null);
const activeFilterCount = computed(
  () => (store.statusFilter ? 1 : 0) + (store.genreFilter ? 1 : 0)
);

function resetFilters() {
  store.statusFilter = "";
  store.genreFilter = "";
}

function handleDocMouseDown(e: MouseEvent) {
  if (!filterOpen.value) return;
  if (filterPopRef.value && !filterPopRef.value.contains(e.target as Node)) {
    filterOpen.value = false;
  }
}

// 最近游玩的游戏（取最近 8 个有游玩记录的）
const recentGames = computed(() => {
  return store.games
    .filter((g) => g.last_played && g.play_time_seconds > 0)
    .sort((a, b) => (b.last_played || "").localeCompare(a.last_played || ""))
    .slice(0, 8);
});

function handleHomeContextMenu(e: MouseEvent) {
  // 输入框/文本编辑区域保留原生右键菜单（粘贴/复制等）
  const target = e.target as HTMLElement | null;
  if (target?.closest?.("input, textarea, [contenteditable='true']")) {
    return;
  }
  e.preventDefault();
  e.stopPropagation();
  homeContextMenuX.value = e.clientX;
  homeContextMenuY.value = e.clientY;
  showHomeContextMenu.value = true;
}

const homeContextMenuItems = computed<ContextMenuItem[]>(() => [
  {
    label: "添加游戏",
    icon: "🎮",
    action: () => openAddGame(),
  },
  {
    label: "刷新封面",
    icon: "🖼️",
    action: () => refreshCovers(),
  },
  {
    label: "刷新游戏信息",
    icon: "📝",
    action: () => refreshAllInfo(),
  },
  { label: "", icon: "", action: () => {}, divider: true },
  {
    label: "刷新列表",
    icon: "🔄",
    action: async () => {
      await store.loadGames();
      message.success("游戏列表已刷新");
    },
  },
]);

function handleRenameGame(gameId: string) {
  const game = store.games.find((g) => g.id === gameId);
  if (game) {
    renamingGameId.value = gameId;
    renameInput.value = game.name;
    showRenameModal.value = true;
  }
}

async function handleConfirmRename() {
  const newName = renameInput.value.trim();
  if (!newName) {
    message.warning("请输入游戏名称");
    return;
  }
  try {
    await store.renameGame(renamingGameId.value, newName);
    message.success("重命名成功");
    showRenameModal.value = false;
  } catch (e) {
    message.error("重命名失败");
  }
}

function handleCancelRename() {
  showRenameModal.value = false;
  renamingGameId.value = "";
  renameInput.value = "";
}

function handleSetLaunchArgs(gameId: string) {
  launchArgsGameId.value = gameId;
  showLaunchArgsModal.value = true;
}

/** 启动游戏并提示失败原因（不再静默吞错） */
async function handleLaunchGame(gameId: string) {
  const game = store.games.find((g) => g.id === gameId);
  const gameName = game?.name || "该游戏";
  try {
    await store.launch(gameId);
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    message.error(`启动「${gameName}」失败: ${msg}`);
  }
}

/** 停止计时：只结束 GV 的会话，不关闭游戏进程 */
async function handleStopGame(gameId: string) {
  const game = store.games.find((g) => g.id === gameId);
  const gameName = game?.name || "该游戏";
  try {
    await store.stop(gameId);
    message.success(`已停止「${gameName}」的计时`);
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    message.error(`停止「${gameName}」计时失败: ${msg}`);
  }
}

async function handleOpenScreenshots(gameId: string) {
  try {
    await api.openScreenshotDir(gameId);
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    message.error(`打开截图文件夹失败: ${msg}`);
  }
}

async function handleRefreshInfo(gameId: string) {
  const game = store.games.find((g) => g.id === gameId);
  const gameName = game?.name || "该游戏";
  const loadingMsg = message.loading(`正在为「${gameName}」刷新信息...`);
  try {
    await store.fetchGameInfoLlm(gameId);
    loadingMsg.destroy();
    message.success(`「${gameName}」信息已刷新`);
  } catch (e) {
    loadingMsg.destroy();
    message.error("刷新信息失败，请检查 LLM 配置");
  }
}

function handleEditInfo(gameId: string) {
  editingGameId.value = gameId;
  showEditInfoModal.value = true;
}

async function handleRemoveCover(gameId: string) {
  const game = store.games.find((g) => g.id === gameId);
  const gameName = game?.name || "该游戏";
  dialog.warning({
    title: "删除封面",
    content: `确定要删除「${gameName}」的封面吗？删除后可点击「刷新封面」重新获取。`,
    positiveText: "删除",
    negativeText: "取消",
    onPositiveClick: async () => {
      try {
        await api.removeGameCover(gameId);
        await store.loadGames();
        message.success("封面已删除");
      } catch (e) {
        message.error("删除封面失败");
      }
    },
  });
}

async function handleToggleCompleted(gameId: string) {
  const game = store.games.find((g) => g.id === gameId);
  if (!game) return;

  // 取消通关时按游玩时长还原状态：玩过 → 游玩中，没玩过 → 未游玩
  // （与统计页「有时长即视为游玩中」的推导口径一致，避免一律退回未游玩）
  const newStatus = game.status === "completed"
    ? (game.play_time_seconds > 0 ? "playing" : "unplayed")
    : "completed";
  try {
    await store.setGameStatus(gameId, newStatus);
    message.success(newStatus === "completed" ? "已标记为通关" : "已取消通关状态");
  } catch (e) {
    message.error("设置游戏状态失败");
  }
}

async function handleSetGameStatus(gameId: string, status: string) {
  try {
    await store.setGameStatus(gameId, status);
    const statusText: Record<string, string> = {
      unplayed: "未游玩",
      playing: "游玩中",
      completed: "已通关",
      abandoned: "已弃坑",
    };
    message.success(`游戏状态已更新为：${statusText[status] || status}`);
  } catch (e) {
    message.error("设置游戏状态失败");
  }
}

function handleDeleteGame(gameId: string) {
  const game = store.games.find((g) => g.id === gameId);
  const gameName = game?.name || "该游戏";
  // 有封面才需要问「留不留图」；没有封面走普通确认框
  const hasCover = !!store.coverPaths[gameId];

  const doDelete = async (keepCover: boolean) => {
    // 自定义 action 的弹窗不会被 naive-ui 自动关闭（2026-09-12 修正）：
    // 不在删除后 destroy，弹窗会一直留着，用户可以反复点删除/留图按钮重复触发。
    d?.destroy();
    try {
      await store.removeGame(gameId, keepCover);
      message.success(
        keepCover
          ? `已删除「${gameName}」，历史游玩记录与封面图片已保留`
          : `已删除「${gameName}」，历史游玩记录已保留`
      );
    } catch (e) {
      message.error("删除失败");
    }
  };

  let d: ReturnType<typeof dialog.warning>;
  d = dialog.warning({
    title: "确认删除",
    content:
      `确定要删除「${gameName}」吗？此操作不可撤销，但记录会保留——仅从库中移除条目与启动入口，历史游玩统计、已解锁成就与截图文件都不受影响。` +
      (hasCover ? "封面图片请选择处理方式：" : ""),
    // 用 action 自定义按钮区：删除前每次都问一次封面的去留
    action: () =>
      hasCover
        ? h(NSpace, { justify: "end" }, () => [
            h(NButton, { size: "small", onClick: () => d.destroy() }, () => "取消"),
            h(
              NButton,
              { size: "small", type: "primary", ghost: true, onClick: () => doDelete(true) },
              () => "删除，保留封面"
            ),
            h(
              NButton,
              { size: "small", type: "error", onClick: () => doDelete(false) },
              () => "删除，一并删图"
            ),
          ])
        : h(NSpace, { justify: "end" }, () => [
            h(NButton, { size: "small", onClick: () => d.destroy() }, () => "取消"),
            h(NButton, { size: "small", type: "error", onClick: () => doDelete(true) }, () => "删除"),
          ]),
  });
}

</script>

<template>
  <div class="home-view" @contextmenu="handleHomeContextMenu">
    <!-- 最近游玩条带 -->
    <div v-if="recentGames.length > 0" class="recent-section">
      <div class="recent-header">
        <span class="recent-title">最近游玩</span>
      </div>
      <div class="recent-strip">
        <div
          v-for="game in recentGames"
          :key="game.id"
          class="recent-card"
          @click="store.selectGame(game)"
        >
          <div class="recent-cover">
            <img
              v-if="store.coverSrc(game.id)"
              :src="store.coverSrc(game.id)!"
              :alt="game.name"
              loading="lazy"
            />
            <div v-else class="recent-cover-placeholder">{{ game.name.charAt(0) }}</div>
            <button
              class="recent-launch"
              @click.stop="handleLaunchGame(game.id)"
              title="启动"
            >▶</button>
          </div>
          <div class="recent-info">
            <div class="recent-name">{{ game.name }}</div>
            <div class="recent-time">{{ formatPlayTime(game.play_time_seconds) }}</div>
          </div>
        </div>
      </div>
    </div>

    <!-- 顶部工具栏：搜索 + 排序 + 筛选（样式参照 Cine Vault 顶栏，见 .library-toolbar） -->
    <div class="library-toolbar">
      <div class="search-box">
        <svg
          class="search-ico"
          width="15"
          height="15"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
        >
          <circle cx="11" cy="11" r="7" />
          <path d="M21 21l-4.35-4.35" />
        </svg>
        <input v-model="searchInput" placeholder="搜索游戏名称…" spellcheck="false" />
        <button v-if="searchInput" class="search-clear" title="清空" @click="clearSearch">✕</button>
      </div>

      <!-- 排序：字段下拉 + ▲▼ 方向切换（内存排序，切换即时生效，不重新拉库） -->
      <div class="sort-combo">
        <div class="sort-selwrap">
          <select v-model="store.sortBy" class="sort-sel" aria-label="排序方式">
            <option v-for="opt in sortOptions" :key="opt.value" :value="opt.value">
              {{ opt.label }}
            </option>
          </select>
        </div>
        <div class="sort-dir">
          <button
            :class="{ on: store.sortOrder === 'asc' }"
            title="升序"
            aria-label="升序"
            :aria-pressed="store.sortOrder === 'asc'"
            @click="store.sortOrder = 'asc'"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
              <path d="M6 15l6-6 6 6" />
            </svg>
          </button>
          <button
            :class="{ on: store.sortOrder === 'desc' }"
            title="降序"
            aria-label="降序"
            :aria-pressed="store.sortOrder === 'desc'"
            @click="store.sortOrder = 'desc'"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
              <path d="M6 9l6 6 6-6" />
            </svg>
          </button>
        </div>
      </div>

      <!-- 筛选漏斗 + 弹层：状态与类型都收在这里，顶部不再摆一排按钮 -->
      <div ref="filterPopRef" class="filter-anchor">
        <button
          class="filter-btn"
          :class="{ active: activeFilterCount > 0 }"
          :aria-expanded="filterOpen"
          aria-haspopup="dialog"
          @click="filterOpen = !filterOpen"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
            <path d="M3 5h18l-7 8v5.5l-4 2V13z" />
          </svg>
          筛选
          <span v-if="activeFilterCount > 0" class="filter-badge">{{ activeFilterCount }}</span>
        </button>

        <transition name="fb-pop">
          <div v-if="filterOpen" class="filter-pop" role="dialog" aria-label="筛选">
            <div class="fb-title">游戏状态</div>
            <div class="fb-chips">
              <button
                v-for="opt in statusChipOptions"
                :key="opt.value"
                class="fb-chip"
                :class="{ on: store.statusFilter === opt.value }"
                :aria-pressed="store.statusFilter === opt.value"
                @click="toggleStatus(opt.value)"
              >
                <span class="fb-ck">
                  <svg viewBox="0 0 24 24" fill="none" stroke-width="3" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M4 12l6 6L20 6" />
                  </svg>
                </span>
                {{ opt.label }}
              </button>
            </div>

            <div class="fb-title fb-title-gap">游戏类型</div>
            <div class="fb-chips fb-genres">
              <button
                v-for="genre in store.allGenres"
                :key="genre"
                class="fb-chip"
                :class="{ on: store.genreFilter === genre }"
                :aria-pressed="store.genreFilter === genre"
                @click="toggleGenre(genre)"
              >
                <span class="fb-ck">
                  <svg viewBox="0 0 24 24" fill="none" stroke-width="3" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M4 12l6 6L20 6" />
                  </svg>
                </span>
                {{ genre }}
              </button>
              <span v-if="store.allGenres.length === 0" class="fb-empty">库里还没有类型信息</span>
            </div>

            <div class="fb-actions">
              <button @click="resetFilters">重置筛选</button>
            </div>
          </div>
        </transition>
      </div>
    </div>

    <!-- 封面获取进度条 -->
    <div v-if="store.coverFetchProgress" class="cover-progress">
      <n-progress
        type="line"
        :percentage="store.coverFetchProgress.total > 0 ? Math.round((store.coverFetchProgress.current / store.coverFetchProgress.total) * 100) : 0"
        :show-indicator="true"
        processing
      />
      <span class="progress-text">
        正在获取封面 ({{ store.coverFetchProgress.current }}/{{ store.coverFetchProgress.total }}):
        {{ store.coverFetchProgress.game_name }}
      </span>
    </div>

    <!-- 游戏信息获取进度条 -->
    <div v-if="store.gameInfoFetchProgress" class="cover-progress">
      <n-progress
        type="line"
        :percentage="store.gameInfoFetchProgress.total > 0 ? Math.round((store.gameInfoFetchProgress.current / store.gameInfoFetchProgress.total) * 100) : 0"
        :show-indicator="true"
        processing
      />
      <span class="progress-text">
        正在获取游戏信息 ({{ store.gameInfoFetchProgress.current }}/{{ store.gameInfoFetchProgress.total }}):
        {{ store.gameInfoFetchProgress.game_name }}
      </span>
    </div>

    <!-- 游戏内容区 -->
    <div class="content-area">
      <!-- 加载中 -->
      <div v-if="store.loading" class="loading">
        <n-spin size="large" />
        <p>加载游戏列表...</p>
      </div>

      <!-- 空状态 -->
      <div v-else-if="store.filteredGames.length === 0" class="empty">
        <n-empty
          :description="store.games.length > 0 ? '没有符合筛选条件的游戏' : '还没有游戏，点左下角「添加游戏」开始'"
        >
          <template #extra v-if="store.games.length === 0">
            <n-button type="primary" @click="openAddGame()">
              添加游戏
            </n-button>
          </template>
        </n-empty>
      </div>

      <!-- 游戏网格 -->
      <div v-else class="game-grid">
        <GameCard
          v-for="game in store.filteredGames"
          :key="game.id"
          :game="game"
          :is-active="store.activeGames.includes(game.id)"
          :launch-state="store.launchState(game.id)"
          :save-path-exists="Object.keys(store.savePathStatus).length > 0 ? store.savePathStatus[game.id] : undefined"
          @click="store.selectGame(game)"
          @launch="handleLaunchGame(game.id)"
          @favorite="store.toggleFav(game.id)"
          @delete="handleDeleteGame(game.id)"
          @rename="handleRenameGame(game.id)"
          @set-launch-args="handleSetLaunchArgs(game.id)"
          @refresh-info="handleRefreshInfo(game.id)"
          @edit-info="handleEditInfo(game.id)"
          @remove-cover="handleRemoveCover(game.id)"
          @toggle-completed="handleToggleCompleted(game.id)"
          @open-screenshots="handleOpenScreenshots(game.id)"
        />
      </div>
    </div>

    <!-- 游戏详情面板 -->
    <GameDetail
      v-if="store.selectedGame"
      :game="store.selectedGame"
      :is-active="store.activeGames.includes(store.selectedGame.id)"
      :launch-state="store.launchState(store.selectedGame.id)"
      @close="store.clearSelection()"
      @launch="handleLaunchGame(store.selectedGame!.id)"
      @stop="handleStopGame(store.selectedGame!.id)"
      @favorite="store.toggleFav(store.selectedGame!.id)"
      @delete="handleDeleteGame(store.selectedGame!.id)"
      @set-status="handleSetGameStatus(store.selectedGame!.id, $event)"
    />

    <!-- 重命名游戏弹窗 -->
    <n-modal
      :show="showRenameModal"
      preset="card"
      title="重命名游戏"
      style="width: 450px"
      :closable="true"
      @close="handleCancelRename()"
    >
      <p style="margin-bottom: 12px; color: #999;">
        请输入新的游戏名称：
      </p>
      <n-input
        v-model:value="renameInput"
        placeholder="游戏名称"
        @keyup.enter="handleConfirmRename()"
      />
      <template #footer>
        <n-space justify="end">
          <n-button @click="handleCancelRename()">取消</n-button>
          <n-button type="primary" @click="handleConfirmRename()">确认修改</n-button>
        </n-space>
      </template>
    </n-modal>

    <!-- 手动填写游戏信息弹窗 -->
    <GameInfoEditModal
      v-if="showEditInfoModal && editingGame"
      :show="showEditInfoModal"
      :game="editingGame"
      @close="showEditInfoModal = false"
      @saved="showEditInfoModal = false"
    />

    <!-- 启动参数弹窗（卡片右键菜单入口；详情面板自持另一个入口） -->
    <LaunchArgsModal
      v-if="showLaunchArgsModal && launchArgsGame"
      :show="showLaunchArgsModal"
      :game="launchArgsGame"
      @close="showLaunchArgsModal = false"
      @saved="showLaunchArgsModal = false"
    />

    <!-- 主页右键菜单 -->
    <ContextMenu
      v-if="showHomeContextMenu"
      :items="homeContextMenuItems"
      :x="homeContextMenuX"
      :y="homeContextMenuY"
      @close="showHomeContextMenu = false"
    />
  </div>
</template>

<style scoped>
/* 纵向三段：最近游玩 / 工具栏 / 内容区。
   内容区用 flex:1 吃掉剩余高度——旧版是 calc(100vh - 260px) 的硬编码，
   工具栏一改高就会算错（多出空白或被裁掉），这里顺手收敛掉。 */
.home-view {
  position: relative;
  height: calc(100vh - 48px);
  display: flex;
  flex-direction: column;
}

/* 最近游玩区域 */
.recent-section {
  margin-bottom: 16px;
}

.recent-header {
  margin-bottom: 8px;
}

.recent-title {
  font-size: 14px;
  font-weight: 600;
  color: #aaa;
}

.recent-strip {
  display: flex;
  gap: 12px;
  overflow-x: auto;
  padding-bottom: 8px;
  scrollbar-width: thin;
  scrollbar-color: rgba(255, 255, 255, 0.15) transparent;
}

.recent-strip::-webkit-scrollbar {
  height: 4px;
}

.recent-strip::-webkit-scrollbar-thumb {
  background: rgba(255, 255, 255, 0.15);
  border-radius: 2px;
}

.recent-card {
  flex-shrink: 0;
  width: 110px;
  cursor: pointer;
  border-radius: 8px;
  overflow: hidden;
  background: rgba(255, 255, 255, 0.05);
  transition: transform 0.2s, box-shadow 0.2s;
}

.recent-card:hover {
  transform: translateY(-2px);
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
}

.recent-cover {
  position: relative;
  width: 100%;
  aspect-ratio: 3/4;
  overflow: hidden;
  background: #2a2a3e;
}

.recent-cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.recent-cover-placeholder {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 28px;
  font-weight: 700;
  color: rgba(255, 255, 255, 0.15);
  background: linear-gradient(135deg, #2a2a3e 0%, #1a1a2e 100%);
}

.recent-launch {
  position: absolute;
  bottom: 4px;
  right: 4px;
  width: 24px;
  height: 24px;
  border-radius: 50%;
  border: none;
  background: var(--accent-color, #6366f1);
  color: white;
  font-size: 10px;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  opacity: 0;
  transition: opacity 0.2s;
}

.recent-card:hover .recent-launch {
  opacity: 1;
}

.recent-info {
  padding: 6px 8px;
}

.recent-name {
  font-size: 11px;
  font-weight: 500;
  color: #ddd;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.recent-time {
  font-size: 10px;
  color: #888;
  margin-top: 2px;
}


/* ============================================
   顶部工具栏（搜索 + 排序 + 筛选）
   参照 Cine Vault 顶栏：内嵌图标的搜索框、排序组合控件（原生 select + ▲▼ 双键）、
   筛选收进漏斗弹层。原生 select 靠 color-scheme 让下拉列表跟随深/浅主题。
   ============================================ */
.library-toolbar {
  --tb-bg: rgba(255, 255, 255, 0.045);
  --tb-bg-hover: rgba(255, 255, 255, 0.08);
  --tb-border: rgba(255, 255, 255, 0.1);
  --tb-border-strong: rgba(255, 255, 255, 0.2);
  --tb-text: #d6d8e2;
  --tb-text-dim: #85889a;
  --tb-accent: var(--accent-color, #6366f1);
  --tb-accent-dim: color-mix(in srgb, var(--tb-accent) 16%, transparent);

  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 16px;
  flex-shrink: 0;
  position: relative;
  z-index: 40; /* 弹层要压住卡片（卡片 hover 用 z-index:2） */
}

/* ---- 搜索框 ---- */
.search-box {
  flex: 0 1 360px;
  min-width: 180px;
  display: flex;
  align-items: center;
  gap: 8px;
  height: 34px;
  padding: 0 12px;
  background: var(--tb-bg);
  border: 1px solid var(--tb-border);
  border-radius: 10px;
  transition: border-color 0.15s, box-shadow 0.15s;
}

.search-box:focus-within {
  border-color: var(--tb-accent);
  box-shadow: 0 0 0 3px var(--tb-accent-dim);
}

.search-ico {
  color: var(--tb-text-dim);
  flex-shrink: 0;
}

.search-box input {
  flex: 1;
  min-width: 0;
  height: 100%;
  border: none;
  outline: none;
  background: transparent;
  color: inherit;
  font-family: inherit;
  font-size: 13.5px;
}

.search-box input::placeholder {
  color: var(--tb-text-dim);
}

.search-clear {
  flex-shrink: 0;
  width: 18px;
  height: 18px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 50%;
  background: var(--tb-bg-hover);
  color: var(--tb-text-dim);
  font-size: 10px;
  line-height: 1;
  cursor: pointer;
}

.search-clear:hover {
  color: var(--tb-text);
}

/* ---- 排序组合：字段下拉 + 方向双键 ---- */
.sort-combo {
  display: flex;
  align-items: center;
  gap: 5px;
  margin-left: auto;
}

.sort-selwrap {
  position: relative;
}

.sort-selwrap::after {
  content: "▾";
  position: absolute;
  right: 9px;
  top: 50%;
  transform: translateY(-50%);
  color: var(--tb-text-dim);
  font-size: 10px;
  pointer-events: none;
}

.sort-sel {
  appearance: none;
  -webkit-appearance: none;
  color-scheme: dark;
  height: 34px;
  padding: 0 24px 0 10px;
  background: var(--tb-bg);
  border: 1px solid var(--tb-border);
  border-radius: 8px;
  color: var(--tb-text);
  font-family: inherit;
  font-size: 12.5px;
  cursor: pointer;
  transition: border-color 0.13s;
}

.sort-sel:hover {
  border-color: var(--tb-border-strong);
}

.sort-sel:focus {
  outline: none;
  border-color: var(--tb-accent);
}

.sort-dir {
  display: flex;
  flex-direction: column;
  height: 34px;
  border: 1px solid var(--tb-border);
  border-radius: 8px;
  background: var(--tb-bg);
  overflow: hidden;
}

.sort-dir button {
  flex: 1;
  width: 24px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  background: transparent;
  color: var(--tb-text-dim);
  cursor: pointer;
  transition: background 0.12s, color 0.12s;
}

.sort-dir button:hover {
  background: var(--tb-bg-hover);
}

.sort-dir button.on {
  color: var(--tb-accent);
  background: var(--tb-accent-dim);
}

.sort-dir svg {
  width: 11px;
  height: 11px;
}

/* ---- 筛选漏斗 ---- */
.filter-anchor {
  position: relative;
}

.filter-btn {
  position: relative;
  display: flex;
  align-items: center;
  gap: 7px;
  height: 34px;
  padding: 0 12px;
  background: var(--tb-bg);
  border: 1px solid var(--tb-border);
  border-radius: 8px;
  color: var(--tb-text);
  font-family: inherit;
  font-size: 12.5px;
  cursor: pointer;
  transition: border-color 0.13s, color 0.13s;
}

.filter-btn:hover {
  border-color: var(--tb-border-strong);
}

.filter-btn.active {
  border-color: color-mix(in srgb, var(--tb-accent) 45%, transparent);
  color: var(--tb-accent);
}

.filter-btn svg {
  width: 13px;
  height: 13px;
}

.filter-badge {
  position: absolute;
  top: -6px;
  right: -6px;
  min-width: 16px;
  height: 16px;
  padding: 0 4px;
  border-radius: 8px;
  background: var(--tb-accent);
  color: #fff;
  font-size: 10px;
  font-weight: 700;
  display: flex;
  align-items: center;
  justify-content: center;
  font-variant-numeric: tabular-nums;
}

/* ---- 筛选弹层 ---- */
.filter-pop {
  position: absolute;
  top: 42px;
  right: 0;
  width: 368px;
  padding: 14px 16px;
  background: #1e1f33;
  border: 1px solid var(--tb-border-strong);
  border-radius: 12px;
  box-shadow: 0 16px 40px rgba(0, 0, 0, 0.45);
  z-index: 60;
}

.fb-pop-enter-active,
.fb-pop-leave-active {
  transition: opacity 0.14s ease, transform 0.14s ease;
}

.fb-pop-enter-from,
.fb-pop-leave-to {
  opacity: 0;
  transform: translateY(-4px);
}

.fb-title {
  font-size: 11px;
  color: var(--tb-text-dim);
  letter-spacing: 1px;
  margin-bottom: 9px;
}

.fb-title-gap {
  margin-top: 14px;
}

.fb-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.fb-genres {
  max-height: 168px;
  overflow-y: auto;
  padding-right: 2px;
  scrollbar-width: thin;
  scrollbar-color: rgba(255, 255, 255, 0.16) transparent;
}

.fb-empty {
  font-size: 12px;
  color: var(--tb-text-dim);
}

.fb-chip {
  display: flex;
  align-items: center;
  gap: 5px;
  height: 30px;
  padding: 0 11px;
  border-radius: 999px;
  border: 1px solid var(--tb-border);
  background: var(--tb-bg);
  color: var(--tb-text-dim);
  font-family: inherit;
  font-size: 12.5px;
  cursor: pointer;
  transition: border-color 0.13s, color 0.13s, background 0.13s;
}

.fb-chip:hover {
  border-color: var(--tb-border-strong);
}

.fb-chip.on {
  background: var(--tb-accent-dim);
  border-color: color-mix(in srgb, var(--tb-accent) 45%, transparent);
  color: var(--tb-accent);
}

.fb-ck {
  width: 13px;
  height: 13px;
  flex-shrink: 0;
  border-radius: 4px;
  border: 1.5px solid var(--tb-text-dim);
  display: flex;
  align-items: center;
  justify-content: center;
  color: transparent;
  transition: background 0.13s, border-color 0.13s, color 0.13s;
}

.fb-ck svg {
  width: 9px;
  height: 9px;
  fill: none;
  stroke: currentColor;
}

.fb-chip.on .fb-ck {
  background: var(--tb-accent);
  border-color: var(--tb-accent);
  color: #fff;
}

.fb-actions {
  display: flex;
  justify-content: flex-end;
  margin-top: 14px;
  padding-top: 10px;
  border-top: 1px solid var(--tb-border);
}

.fb-actions button {
  border: none;
  background: transparent;
  color: var(--tb-text-dim);
  font-family: inherit;
  font-size: 12px;
  padding: 4px 10px;
  border-radius: 6px;
  cursor: pointer;
  transition: color 0.12s, background 0.12s;
}

.fb-actions button:hover {
  color: var(--tb-text);
  background: var(--tb-bg-hover);
}

/* 亮色主题：同一套控件换一套灰度，弹层改白底 */
.light-theme .library-toolbar {
  --tb-bg: rgba(0, 0, 0, 0.03);
  --tb-bg-hover: rgba(0, 0, 0, 0.06);
  --tb-border: rgba(0, 0, 0, 0.1);
  --tb-border-strong: rgba(0, 0, 0, 0.2);
  --tb-text: #333;
  --tb-text-dim: #7c818f;
}

.light-theme .sort-sel {
  color-scheme: light;
}

.light-theme .filter-pop {
  background: #fff;
  box-shadow: 0 16px 40px rgba(0, 0, 0, 0.12);
}

.light-theme .fb-genres {
  scrollbar-color: rgba(0, 0, 0, 0.16) transparent;
}

.cover-progress {
  margin-bottom: 16px;
  padding: 12px;
  background: rgba(255, 255, 255, 0.05);
  border-radius: 8px;
}

.progress-text {
  display: block;
  margin-top: 8px;
  font-size: 12px;
  color: #888;
}

.content-area {
  flex: 1;
  min-height: 0;
  overflow-y: auto;

  /* Dark scrollbar to match project theme */
  scrollbar-width: thin;
  scrollbar-color: rgba(255, 255, 255, 0.2) transparent;
}

.content-area::-webkit-scrollbar {
  width: 6px;
}

.content-area::-webkit-scrollbar-track {
  background: transparent;
}

.content-area::-webkit-scrollbar-thumb {
  background: rgba(255, 255, 255, 0.2);
  border-radius: 3px;
}

.content-area::-webkit-scrollbar-thumb:hover {
  background: rgba(255, 255, 255, 0.35);
}

.loading,
.empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  height: 400px;
  gap: 16px;
}

.game-grid {
  display: grid;
  /* 列数算法**保持原样**（auto-fill 按 180+16 算列，1400 窗口下 6 列 × 185px）。
     卡片宽度上限不在这里设：minmax 的 max 值会参与列数计算，一改列数就掉档
     （6 列 → 5 列，卡片被撑大、观感全变）。上限放在 GameCard 的 max-width 上，
     常规窗口下等于没生效，只在"窗口很窄、列数掉到 2~3 列被撑到 260px+"时兜底。 */
  grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
  gap: 16px;
  padding-bottom: 24px;
}
</style>
