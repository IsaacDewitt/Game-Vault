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
