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
  useMessage,
  useDialog,
} from "naive-ui";
import { SearchOutline, CloudDownloadOutline, AddOutline } from "@vicons/ionicons5";
import { open } from "@tauri-apps/plugin-dialog";
import { ref, computed } from "vue";
import { useDebounceFn } from "@vueuse/core";
import { useGamesStore } from "../stores/games";
import * as api from "../lib/tauri";
import GameCard from "../components/GameCard.vue";
import GameDetail from "../components/GameDetail.vue";
import ContextMenu from "../components/ContextMenu.vue";
import type { ContextMenuItem } from "../components/ContextMenu.vue";

const store = useGamesStore();
const message = useMessage();
const dialog = useDialog();

// 添加游戏弹窗状态
const showNameModal = ref(false);
const pendingExePath = ref("");
const gameNameInput = ref("");
// 重命名游戏弹窗状态
const showRenameModal = ref(false);
const renamingGameId = ref("");
const renameInput = ref("");
// 封面获取 loading 状态
const refreshingCovers = ref(false);

// 主页右键菜单状态
const showHomeContextMenu = ref(false);
const homeContextMenuX = ref(0);
const homeContextMenuY = ref(0);

function handleHomeContextMenu(e: MouseEvent) {
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

async function handleAddGame() {
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

// 搜索去抖动（300ms）
const handleSearch = useDebounceFn((value: string) => {
  store.searchQuery = value;
}, 300);

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

  const newStatus = game.status === "completed" ? "unplayed" : "completed";
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
  dialog.warning({
    title: "确认删除",
    content: `确定要删除「${gameName}」吗？此操作不可撤销，游戏的游玩记录将一并删除。`,
    positiveText: "删除",
    negativeText: "取消",
    onPositiveClick: async () => {
      try {
        await store.removeGame(gameId);
        message.success("已删除游戏");
      } catch (e) {
        message.error("删除失败");
      }
    },
  });
}
</script>

<template>
  <div class="home-view" @contextmenu="handleHomeContextMenu">
    <!-- 顶部工具栏 -->
    <div class="toolbar">
      <n-space align="center" justify="space-between" style="width: 100%">
        <n-space align="center">
          <n-input
            placeholder="搜索游戏..."
            clearable
            style="width: 300px"
            @update:value="handleSearch"
          >
            <template #prefix>
              <n-icon :component="SearchOutline" />
            </template>
          </n-input>
        </n-space>

        <n-space>
          <n-button @click="handleRefreshCovers" :loading="refreshingCovers">
            <template #icon>
              <n-icon :component="CloudDownloadOutline" />
            </template>
            刷新封面
          </n-button>
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
        :percentage="Math.round((store.coverFetchProgress.current / store.coverFetchProgress.total) * 100)"
        :show-indicator="true"
        processing
      />
      <span class="progress-text">
        正在获取封面 ({{ store.coverFetchProgress.current }}/{{ store.coverFetchProgress.total }}):
        {{ store.coverFetchProgress.game_name }}
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
        <n-empty description="还没有游戏，点击上方按钮添加">
          <template #extra>
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
          @click="store.selectGame(game)"
          @launch="store.launch(game.id)"
          @favorite="store.toggleFav(game.id)"
          @delete="handleDeleteGame(game.id)"
          @rename="handleRenameGame(game.id)"
          @refresh-info="handleRefreshInfo(game.id)"
          @remove-cover="handleRemoveCover(game.id)"
          @toggle-completed="handleToggleCompleted(game.id)"
        />
      </div>
    </div>

    <!-- 游戏详情面板 -->
    <GameDetail
      v-if="store.selectedGame"
      :game="store.selectedGame"
      :is-active="store.activeGames.includes(store.selectedGame.id)"
      @close="store.clearSelection()"
      @launch="store.launch(store.selectedGame!.id)"
      @favorite="store.toggleFav(store.selectedGame!.id)"
      @delete="handleDeleteGame(store.selectedGame!.id)"
      @set-status="handleSetGameStatus(store.selectedGame!.id, $event)"
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
.home-view {
  position: relative;
  height: calc(100vh - 48px);
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
  height: calc(100vh - 140px);
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
