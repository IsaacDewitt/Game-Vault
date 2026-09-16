<script setup lang="ts">
import {
  NSpace,
  NInput,
  NButton,
  NIcon,
  NSpin,
  NEmpty,
  NModal,
  NProgress,
  NSelect,
  NButtonGroup,
  NDropdown,
  NTooltip,
  useMessage,
  useDialog,
  type DropdownOption,
} from "naive-ui";
import { SearchOutline, CloudDownloadOutline, AddOutline, DocumentTextOutline, FolderOutline, RefreshOutline, InformationCircleOutline } from "@vicons/ionicons5";
import { open } from "@tauri-apps/plugin-dialog";
import { ref, computed, onMounted, onUnmounted, h } from "vue";
import { useDebounceFn } from "@vueuse/core";
import { listen } from "@tauri-apps/api/event";
import { useGamesStore } from "../stores/games";
import * as api from "../lib/tauri";
import { DEBOUNCE_MS } from "../lib/constants";
import GameCard from "../components/GameCard.vue";
import GameDetail from "../components/GameDetail.vue";
import GameInfoEditModal from "../components/GameInfoEditModal.vue";
import PlatformImportModal from "../components/PlatformImportModal.vue";
import ContextMenu from "../components/ContextMenu.vue";
import type { ContextMenuItem } from "../components/ContextMenu.vue";
import { formatPlayTime } from "../lib/format";

const store = useGamesStore();
const message = useMessage();
const dialog = useDialog();

// 监听截图结果事件（全局热键触发截图后由后端推送）
let unlistenScreenshot: (() => void) | null = null;
// 平台游戏：待命转正 / 待命窗口内未现身（后端 lib.rs 的 tick.activated / tick.arm_timeouts）
let unlistenPlatformStarted: (() => void) | null = null;
let unlistenPlatformTimeout: (() => void) | null = null;
// 与后端 PLATFORM_ARM_WINDOW_SECS（300 秒）保持一致，仅用于提示文案
const PLATFORM_ARM_WINDOW_TEXT = "5 分钟";
onMounted(async () => {
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
      }
    });
  } catch (e) {
    console.error("监听截图事件失败:", e);
  }

  // 平台游戏（Steam / Epic）由客户端异步拉起：待命转正后立刻把卡片标成"运行中"，
  // 不必等下一次列表刷新；否则点了启动到游戏现身之间，界面一直像没反应。
  try {
    unlistenPlatformStarted = await listen<string[]>("platform-game-started", (event) => {
      for (const id of event.payload) {
        if (!store.activeGames.includes(id)) {
          store.activeGames.push(id);
        }
      }
    });
  } catch (e) {
    console.error("监听平台游戏启动事件失败:", e);
  }

  // 待命窗口内没等到游戏进程（正在更新 / 客户端未登录 / 用户取消）→ 必须给个交代：
  // 此前后端只写了日志，界面毫无反馈，用户会以为"点了启动什么也没发生"。
  try {
    unlistenPlatformTimeout = await listen<string>("platform-launch-timeout", (event) => {
      const gameId = event.payload;
      const idx = store.activeGames.indexOf(gameId);
      if (idx !== -1) {
        store.activeGames.splice(idx, 1);
      }
      const name = store.games.find((g) => g.id === gameId)?.name ?? "该游戏";
      message.warning(
        `《${name}》启动超时：${PLATFORM_ARM_WINDOW_TEXT}内没等到游戏进程（可能正在更新、客户端未登录或已取消），本次不计入时长`,
        { duration: 8000 }
      );
    });
  } catch (e) {
    console.error("监听平台启动超时事件失败:", e);
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
  if (unlistenScreenshot) {
    unlistenScreenshot();
    unlistenScreenshot = null;
  }
  if (unlistenPlatformStarted) {
    unlistenPlatformStarted();
    unlistenPlatformStarted = null;
  }
  if (unlistenPlatformTimeout) {
    unlistenPlatformTimeout();
    unlistenPlatformTimeout = null;
  }
});

// 添加游戏弹窗状态
const showNameModal = ref(false);
const pendingExePath = ref("");
const gameNameInput = ref("");
// 游戏来源选择 + 平台导入（Steam / Epic）
const showPlatformPicker = ref(false);
const showPlatformImport = ref(false);
const importPlatform = ref("steam");
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
// 封面获取 loading 状态
const refreshingCovers = ref(false);
// 游戏信息获取 loading 状态
const refreshingInfo = ref(false);

// 主页右键菜单状态
const showHomeContextMenu = ref(false);
const homeContextMenuX = ref(0);
const homeContextMenuY = ref(0);

// 状态筛选选项
// 三态模型：未游玩（从未启动）/ 已游玩（启动过）/ 已通关（用户手动标记）。
// 是否「玩过」用 play_time_seconds 判断、是否「通关」用 status=completed 判断，
// 与后端 get_status_stats 的智能推导口径一致；收藏是独立维度，不参与三态互斥。
const statusOptions = [
  { label: "全部", value: "" },
  { label: "收藏", value: "favorites" },
  { label: "未游玩", value: "unplayed" },
  { label: "已游玩", value: "played" },
  { label: "已通关", value: "completed" },
];

// 类型筛选选项（从 store 动态加载）
const genreSelectOptions = computed(() => [
  { label: "全部类型", value: "" },
  ...store.allGenres.map((g) => ({ label: g, value: g })),
]);

// 「刷新」合并下拉菜单：把刷新封面 / 刷新游戏信息 / 检查存档三项收进一个按钮
const refreshMenuOptions: DropdownOption[] = [
  {
    label: "刷新封面",
    key: "covers",
    icon: () => h(NIcon, null, { default: () => h(CloudDownloadOutline) }),
  },
  {
    label: "刷新游戏信息",
    key: "info",
    icon: () => h(NIcon, null, { default: () => h(DocumentTextOutline) }),
  },
  {
    label: "检查存档",
    key: "saves",
    icon: () => h(NIcon, null, { default: () => h(FolderOutline) }),
  },
];

// 下拉菜单项点击分发到对应刷新动作
function handleRefreshSelect(key: string | number) {
  if (key === "covers") {
    handleRefreshCovers();
  } else if (key === "info") {
    handleRefreshAllInfo();
  } else if (key === "saves") {
    store.checkSavePaths();
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
    action: () => handleAddGame(),
  },
  {
    label: "刷新封面",
    icon: "🖼️",
    action: () => handleRefreshCovers(),
  },
  {
    label: "刷新游戏信息",
    icon: "📝",
    action: () => handleRefreshAllInfo(),
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

/** 点击「添加游戏」：先让老爷选游戏来源（本地 / Steam / Epic） */
function handleAddGame() {
  showPlatformPicker.value = true;
}

/** 来源选「本地游戏」→ 走原有的选择 exe 流程 */
function handlePickLocalExe() {
  showPlatformPicker.value = false;
  pickExeFile();
}

/** 来源选 Steam / Epic → 打开平台导入弹窗 */
function handlePickPlatform(platform: string) {
  showPlatformPicker.value = false;
  importPlatform.value = platform;
  showPlatformImport.value = true;
}

/** 选择本地游戏的 exe 文件（原「添加游戏」流程） */
async function pickExeFile() {
  try {
    const selected = await open({
      multiple: false,
      title: "选择游戏程序",
      filters: [
        {
          name: "可执行文件",
          extensions: ["exe"],
        },
      ],
    });
    if (selected) {
      const exePath = selected as string;
      // 从 exe 路径提取文件名（不含扩展名）作为默认游戏名
      const fileName = exePath.split(/[/\\]/).pop() || "";
      const defaultName = fileName.replace(/\.exe$/i, "");
      pendingExePath.value = exePath;
      gameNameInput.value = defaultName;
      showNameModal.value = true;
    }
  } catch (e) {
    console.error(e);
    message.error("选择文件失败");
  }
}

async function handleConfirmAddGame() {
  const name = gameNameInput.value.trim();
  if (!name) {
    message.warning("请输入游戏名称");
    return;
  }
  try {
    await store.addGameManual(name, pendingExePath.value);
    message.success(`已添加游戏: ${name}`);
    showNameModal.value = false;
    pendingExePath.value = "";
    gameNameInput.value = "";
  } catch (e) {
    message.error("添加游戏失败");
  }
}

function handleCancelAddGame() {
  showNameModal.value = false;
  pendingExePath.value = "";
  gameNameInput.value = "";
}

/** 平台导入完成后刷新列表（导入是后端批量入库，前端需重新拉取） */
async function handlePlatformImported() {
  try {
    await store.loadGames();
  } catch (e) {
    console.error("刷新游戏列表失败:", e);
  }
}

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

async function handleOpenScreenshots(gameId: string) {
  try {
    await api.openScreenshotDir(gameId);
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    message.error(`打开截图文件夹失败: ${msg}`);
  }
}

// 搜索去抖动（300ms）
const handleSearch = useDebounceFn((value: string) => {
  store.searchQuery = value;
}, DEBOUNCE_MS);

async function handleRefreshCovers() {
  refreshingCovers.value = true;
  try {
    const result = await store.fetchCovers();

    // 检查是否有 API Key 认证失败
    const authError = result.errors.find((e: string) =>
      e.includes("API Key 无效") || e.includes("401") || e.includes("403")
    );

    if (authError) {
      message.error("SteamGridDB API Key 无效，请在设置中重新配置");
    } else if (result.fetched > 0) {
      message.success(`已获取 ${result.fetched} 个游戏的封面`);
    } else if (result.total > 0) {
      message.warning(
        `${result.total} 个游戏缺少封面，但在 SteamGridDB 中未找到。可尝试手动设置封面`
      );
      result.errors.forEach((e: string) => console.warn("封面获取:", e));
    } else if (result.errors.length === 0) {
      message.info("所有游戏封面已是最新");
    } else {
      message.error(result.errors[0]);
      result.errors.slice(1).forEach((e: string) => console.warn("封面获取:", e));
    }
  } catch (e) {
    message.error("获取封面失败");
  } finally {
    refreshingCovers.value = false;
  }
}

async function handleRefreshAllInfo() {
  refreshingInfo.value = true;
  try {
    const result = await store.fetchGameInfo();

    if (result.fetched > 0) {
      message.success(`已获取 ${result.fetched} 个游戏的信息`);
    } else if (result.total > 0) {
      message.warning(
        `${result.total} 个游戏缺少信息，但获取失败。请检查 LLM 配置`
      );
      result.errors.forEach((e: string) => console.warn("游戏信息获取:", e));
    } else if (result.errors.length === 0) {
      message.info("所有游戏信息已是最新");
    } else {
      message.error(result.errors[0]);
      result.errors.slice(1).forEach((e: string) => console.warn("游戏信息获取:", e));
    }
  } catch (e) {
    message.error("获取游戏信息失败，请检查 LLM 配置");
  } finally {
    refreshingInfo.value = false;
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

    <!-- 顶部工具栏 -->
    <div class="toolbar">
      <n-space align="center" justify="space-between" style="width: 100%">
        <n-space align="center">
          <n-input
            placeholder="搜索游戏..."
            clearable
            style="width: 240px"
            @update:value="handleSearch"
          >
            <template #prefix>
              <n-icon :component="SearchOutline" />
            </template>
          </n-input>

          <!-- 状态筛选 -->
          <n-button-group>
            <n-button
              v-for="opt in statusOptions"
              :key="opt.value"
              :type="store.statusFilter === opt.value ? 'primary' : 'default'"
              size="small"
              @click="store.statusFilter = opt.value"
            >
              {{ opt.label }}
            </n-button>
          </n-button-group>

          <!-- 类型筛选 -->
          <n-select
            v-model:value="store.genreFilter"
            :options="genreSelectOptions"
            placeholder="游戏类型"
            clearable
            style="width: 150px"
            size="small"
          />
        </n-space>

        <n-space align="center">
          <!-- 「刷新」合并下拉：三个刷新动作收进一个按钮 -->
          <n-dropdown trigger="click" :options="refreshMenuOptions" @select="handleRefreshSelect">
            <n-button :loading="refreshingCovers || refreshingInfo || store.checkingSavePaths">
              <template #icon>
                <n-icon :component="RefreshOutline" />
              </template>
              刷新
            </n-button>
          </n-dropdown>

          <!-- 圈感叹号：悬停解释每个刷新动作具体做什么 -->
          <n-tooltip trigger="hover" placement="bottom-end" :style="{ maxWidth: '340px', whiteSpace: 'normal' }">
            <template #trigger>
              <n-button quaternary circle>
                <template #icon>
                  <n-icon :component="InformationCircleOutline" />
                </template>
              </n-button>
            </template>
            <div style="font-size: 12px; line-height: 1.8; text-align: left;">
              <div><b>刷新封面</b> — 从 SteamGridDB 下载缺少封面的游戏封面图</div>
              <div><b>刷新游戏信息</b> — 调用 AI 补全游戏简介、类型、发售日期等资料</div>
              <div><b>检查存档</b> — 检测各游戏是否已正确配置存档目录</div>
            </div>
          </n-tooltip>

          <n-button type="primary" @click="handleAddGame">
            <template #icon>
              <n-icon :component="AddOutline" />
            </template>
            添加游戏
          </n-button>
        </n-space>
      </n-space>
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
        <n-empty :description="store.games.length > 0 ? '没有符合筛选条件的游戏' : '还没有游戏，点击上方按钮添加'">
          <template #extra v-if="store.games.length === 0">
            <n-button type="primary" @click="handleAddGame">
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
          :save-path-exists="Object.keys(store.savePathStatus).length > 0 ? store.savePathStatus[game.id] : undefined"
          @click="store.selectGame(game)"
          @launch="handleLaunchGame(game.id)"
          @favorite="store.toggleFav(game.id)"
          @delete="handleDeleteGame(game.id)"
          @rename="handleRenameGame(game.id)"
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
      @close="store.clearSelection()"
      @launch="handleLaunchGame(store.selectedGame!.id)"
      @favorite="store.toggleFav(store.selectedGame!.id)"
      @delete="handleDeleteGame(store.selectedGame!.id)"
      @set-status="handleSetGameStatus(store.selectedGame!.id, $event)"
    />

    <!-- 游戏来源选择弹窗：本地 / Steam / Epic -->
    <n-modal
      :show="showPlatformPicker"
      preset="card"
      title="添加游戏"
      class="source-picker-modal"
      :closable="true"
      @close="showPlatformPicker = false"
    >
      <p style="margin-bottom: 14px; color: #999; font-size: 13px;">
        请选择游戏来源：
      </p>
      <div class="source-list">
        <button class="source-item" @click="handlePickLocalExe()">
          <span class="source-icon">🖥️</span>
          <span class="source-text">
            <span class="source-title">本地游戏</span>
            <span class="source-desc">手动选择游戏 exe，由本应用直接启动</span>
          </span>
        </button>
        <button class="source-item" @click="handlePickPlatform('steam')">
          <span class="source-icon">🎮</span>
          <span class="source-text">
            <span class="source-title">Steam 游戏</span>
            <span class="source-desc">扫描本机 Steam 库，交由 Steam 启动并记录时长</span>
          </span>
        </button>
        <button class="source-item" @click="handlePickPlatform('epic')">
          <span class="source-icon">🕹️</span>
          <span class="source-text">
            <span class="source-title">Epic 游戏</span>
            <span class="source-desc">扫描本机 Epic 库，交由 Epic 启动、记录时长并支持截图</span>
          </span>
        </button>
      </div>
    </n-modal>

    <!-- 平台游戏导入弹窗 -->
    <PlatformImportModal
      :show="showPlatformImport"
      :platform="importPlatform"
      @close="showPlatformImport = false"
      @imported="handlePlatformImported()"
    />

    <!-- 输入游戏名称弹窗 -->
    <n-modal
      :show="showNameModal"
      preset="card"
      title="添加游戏"
      style="width: 450px"
      :closable="true"
      @close="handleCancelAddGame()"
    >
      <p style="margin-bottom: 12px; color: #999;">
        请输入游戏名称：
      </p>
      <n-input
        v-model:value="gameNameInput"
        placeholder="游戏名称"
        @keyup.enter="handleConfirmAddGame()"
      />
      <template #footer>
        <n-space justify="end">
          <n-button @click="handleCancelAddGame()">取消</n-button>
          <n-button type="primary" @click="handleConfirmAddGame()">确认添加</n-button>
        </n-space>
      </template>
    </n-modal>

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

<!-- 非 scoped：弹窗被 teleport 到 body，scoped 选择器无法命中内部卡片 -->
<style>
/* 「添加游戏」来源选择弹窗宽度：Modal 会把 class 透传到内部卡片 .n-modal 上
   （与 .n-modal 同属一个元素，故此处用复合选择器）。卡片默认按内容撑满全宽，须显式定宽。 */
.n-modal.source-picker-modal {
  width: 440px;
  max-width: 92vw;
}
</style>

<style scoped>
.home-view {
  position: relative;
  height: calc(100vh - 48px);
}

/* 游戏来源选择弹窗 */
.source-list {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.source-item {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 12px 14px;
  text-align: left;
  background: rgba(128, 128, 128, 0.08);
  border: 1px solid rgba(128, 128, 128, 0.18);
  border-radius: 8px;
  cursor: pointer;
  transition: all 0.15s;
  font-family: inherit;
  color: inherit;
}

.source-item:hover {
  border-color: #6366f1;
  background: rgba(99, 102, 241, 0.1);
}

.source-icon {
  font-size: 22px;
  line-height: 1;
  flex-shrink: 0;
}

.source-text {
  display: flex;
  flex-direction: column;
  gap: 3px;
  min-width: 0;
}

.source-title {
  font-size: 14px;
  font-weight: 600;
}

.source-desc {
  font-size: 11.5px;
  color: #888;
  line-height: 1.4;
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

.toolbar {
  margin-bottom: 16px;
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
  height: calc(100vh - 260px);
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
  grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
  gap: 16px;
  padding-bottom: 24px;
}
</style>
