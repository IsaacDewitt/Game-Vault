<script setup lang="ts">
import { ref, toRef, computed, watch, onMounted, onUnmounted } from "vue";
import {
  NDrawer,
  NDrawerContent,
  NButton,
  NIcon,
  NSpace,
  NDivider,
  NGrid,
  NGi,
  NTooltip,
  NProgress,
  NDropdown,
  useMessage,
} from "naive-ui";
import {
  PlayOutline,
  HeartOutline,
  Heart,
  TrashOutline,
  GameControllerOutline,
  ImageOutline,
  CheckmarkCircleOutline,
  TrophyOutline,
  TimeOutline,
  FolderOpenOutline,
  CreateOutline,
  AddOutline,
  CloseOutline,
} from "@vicons/ionicons5";
import CoverPickerModal from "./CoverPickerModal.vue";
import GameInfoEditModal from "./GameInfoEditModal.vue";
import { open } from "@tauri-apps/plugin-dialog";
import type { Game, PlaySessionDetail } from "../lib/tauri";
import * as api from "../lib/tauri";
import { formatPlayTime, formatDate } from "../lib/format";
import { useGamesStore } from "../stores/games";
import { useCoverImage } from "../lib/useCoverImage";

const props = defineProps<{
  game: Game;
  isActive?: boolean;
}>();

const emit = defineEmits<{
  close: [];
  launch: [];
  favorite: [];
  delete: [];
  setStatus: [status: string];
}>();

// 阻止 drawer 遮罩层的右键默认菜单（Naive UI 内部渲染的 mask 元素，Vue 的 @contextmenu.prevent 管不到）
const preventMaskContextMenu = (e: MouseEvent) => e.preventDefault();
onMounted(() => document.addEventListener("contextmenu", preventMaskContextMenu));
onUnmounted(() => document.removeEventListener("contextmenu", preventMaskContextMenu));

const store = useGamesStore();
const message = useMessage();
const fetchingLlm = ref(false);

// 封面选择器状态
const showCoverPicker = ref(false);
// 手动填写信息弹窗状态
const showEditInfoModal = ref(false);


// 最近游玩记录
const recentSessions = ref<PlaySessionDetail[]>([]);

watch(
  () => props.game.id,
  async (gameId) => {
    if (gameId) {
      try {
        recentSessions.value = await api.getPlaySessions(gameId, 3);
      } catch (e) {
        console.error("获取最近游玩记录失败:", e);
        recentSessions.value = [];
      }
    }
  },
  { immediate: true }
);

// HLTB 数据计算
const hasHltb = computed(() =>
  props.game.hltb_main_story != null || props.game.hltb_main_extra != null || props.game.hltb_completionist != null
);

// 实际游玩时长（分钟）
const playedMinutes = computed(() => Math.round(props.game.play_time_seconds / 60));

// 主线进度百分比
const hltbProgress = computed(() => {
  if (!props.game.hltb_main_story || props.game.hltb_main_story === 0) return 0;
  return Math.min(100, Math.round((playedMinutes.value / props.game.hltb_main_story) * 100));
});

// 预计剩余时间（基于主线）
const hltbRemaining = computed(() => {
  if (!props.game.hltb_main_story) return null;
  const remaining = props.game.hltb_main_story - playedMinutes.value;
  if (remaining <= 0) return 0;
  return remaining;
});

function formatMinutes(minutes: number): string {
  if (minutes < 60) return `${minutes}分钟`;
  const hours = Math.floor(minutes / 60);
  const mins = minutes % 60;
  if (mins === 0) return `${hours}小时`;
  return `${hours}小时${mins}分钟`;
}

const { coverImage, showPlaceholder, handleImageError } = useCoverImage(toRef(props, "game"));

// 标签颜色映射
const genreColorMap: Record<string, string> = {
  '动作': '#e74c3c',
  'Action': '#e74c3c',
  '冒险': '#e67e22',
  'Adventure': '#e67e22',
  '角色扮演': '#9b59b6',
  'RPG': '#9b59b6',
  '策略': '#3498db',
  'Strategy': '#3498db',
  '射击': '#e74c3c',
  'Shooter': '#e74c3c',
  '模拟': '#1abc9c',
  'Simulation': '#1abc9c',
  '体育': '#2ecc71',
  'Sports': '#2ecc71',
  '竞速': '#f39c12',
  'Racing': '#f39c12',
  '益智': '#f1c40f',
  'Puzzle': '#f1c40f',
  '恐怖': '#8e44ad',
  'Horror': '#8e44ad',
  '独立': '#95a5a6',
  'Indie': '#95a5a6',
  '休闲': '#1abc9c',
  'Casual': '#1abc9c',
  '格斗': '#c0392b',
  'Fighting': '#c0392b',
  '平台': '#2980b9',
  'Platform': '#2980b9',
  '解谜': '#f39c12',
  '开放世界': '#27ae60',
  'Open World': '#27ae60',
  '生存': '#d35400',
  'Survival': '#d35400',
  '多人': '#2980b9',
  'Multiplayer': '#2980b9',
  '单人': '#7f8c8d',
  'Singleplayer': '#7f8c8d',
  '奇幻': '#8e44ad',
  'Fantasy': '#8e44ad',
  '科幻': '#2c3e50',
  'Sci-Fi': '#2c3e50',
  '历史': '#795548',
  'Historical': '#795548',
};

// 预设调色板，用于未匹配的标签
const fallbackColors = ['#e74c3c', '#e67e22', '#f1c40f', '#2ecc71', '#3498db', '#9b59b6', '#1abc9c', '#e91e63'];

function getGenreColor(genre: string): string {
  if (genreColorMap[genre]) return genreColorMap[genre];
  // 简单哈希，确保同一标签始终同色
  let hash = 0;
  for (let i = 0; i < genre.length; i++) {
    hash = genre.charCodeAt(i) + ((hash << 5) - hash);
  }
  return fallbackColors[Math.abs(hash) % fallbackColors.length];
}

async function handleFetchInfoLlm() {
  fetchingLlm.value = true;
  try {
    await store.fetchGameInfoLlm(props.game.id);
    message.success("已通过 LLM 获取游戏信息");
  } catch (e) {
    message.error("LLM 获取失败: " + (e as Error).toString());
  } finally {
    fetchingLlm.value = false;
  }
}

async function handleChangeCover() {
  try {
    const selected = await open({
      multiple: false,
      filters: [
        {
          name: "图片",
          extensions: ["jpg", "jpeg", "png", "webp"],
        },
      ],
      title: "选择封面图片",
    });
    if (selected) {
      await store.setCover(props.game.id, selected as string);
    }
  } catch (e) {
    console.error("更换封面失败:", e);
  }
}

// 封面来源下拉菜单
const coverDropdownOptions = [
  { label: "从本地选择", key: "local" },
  { label: "从 SteamGridDB 选择", key: "steamgriddb" },
];

function handleCoverDropdown(key: string) {
  if (key === "local") {
    handleChangeCover();
  } else if (key === "steamgriddb") {
    showCoverPicker.value = true;
  }
}

async function handleOpenExeFolder() {
  if (!props.game.exe_path) return;
  try {
    await api.openSavePath(props.game.exe_path);
  } catch (e) {
    message.error("打开文件夹失败: " + (e as Error).toString());
  }
}

async function handleChangeExePath() {
  try {
    const selected = await open({
      multiple: false,
      filters: [
        {
          name: "可执行文件",
          extensions: ["exe"],
        },
      ],
      title: "选择游戏可执行文件",
    });
    if (selected) {
      await store.updateExePath(props.game.id, selected as string);
      message.success("可执行文件路径已更新，版本号已刷新");
    }
  } catch (e) {
    message.error("更新可执行文件路径失败: " + (e as Error).toString());
  }
}

// 存档路径相关方法
async function handleOpenSavePath(path: string) {
  try {
    await api.openSavePath(path);
  } catch (e) {
    message.error("打开路径失败: " + (e as Error).toString());
  }
}

async function handleChangeSavePath(index: number) {
  try {
    const selected = await open({
      multiple: false,
      directory: true,
      title: "选择存档文件夹",
    });
    if (selected) {
      const paths = [...(props.game.save_paths || [])];
      paths[index] = selected as string;
      await api.updateSavePaths(props.game.id, paths);
      // 更新本地游戏数据
      const updated = await api.getGameDetail(props.game.id);
      if (updated) {
        const idx = store.games.findIndex((g) => g.id === props.game.id);
        if (idx !== -1) {
          store.games[idx] = updated;
        }
        if (store.selectedGame?.id === props.game.id) {
          store.selectedGame = updated;
        }
      }
      message.success("存档路径已更新");
    }
  } catch (e) {
    message.error("修改路径失败: " + (e as Error).toString());
  }
}

async function handleAddSavePath() {
  try {
    const selected = await open({
      multiple: false,
      directory: true,
      title: "选择存档文件夹",
    });
    if (selected) {
      const paths = [...(props.game.save_paths || []), selected as string];
      await api.updateSavePaths(props.game.id, paths);
      // 更新本地游戏数据
      const updated = await api.getGameDetail(props.game.id);
      if (updated) {
        const idx = store.games.findIndex((g) => g.id === props.game.id);
        if (idx !== -1) {
          store.games[idx] = updated;
        }
        if (store.selectedGame?.id === props.game.id) {
          store.selectedGame = updated;
        }
      }
      message.success("存档路径已添加");
    }
  } catch (e) {
    message.error("添加路径失败: " + (e as Error).toString());
  }
}

async function handleRemoveSavePath(index: number) {
  const paths = [...(props.game.save_paths || [])];
  paths.splice(index, 1);
  try {
    await api.updateSavePaths(props.game.id, paths);
    // 更新本地游戏数据
    const updated = await api.getGameDetail(props.game.id);
    if (updated) {
      const idx = store.games.findIndex((g) => g.id === props.game.id);
      if (idx !== -1) {
        store.games[idx] = updated;
      }
      if (store.selectedGame?.id === props.game.id) {
        store.selectedGame = updated;
      }
    }
    message.success("存档路径已删除");
  } catch (e) {
    message.error("删除失败: " + (e as Error).toString());
  }
}
</script>

<template>
  <n-drawer
    :show="true"
    :width="400"
    placement="right"
    @update:show="(val) => !val && emit('close')"
    @contextmenu.prevent
  >
    <n-drawer-content :native-scrollbar="false" @contextmenu.prevent>
      <template #header>
        <div class="detail-header">
          <span>{{ game.name }}</span>
        </div>
      </template>

      <!-- 封面图 -->
      <div class="cover-section">
        <img
          v-if="!showPlaceholder && coverImage"
          :src="coverImage"
          :alt="game.name"
          class="cover-img"
          @error="handleImageError"
        />
        <div v-else class="cover-placeholder">
          <n-icon :component="GameControllerOutline" :size="48" />
        </div>
        <!-- 标签角标 -->
        <div v-if="game.genres && game.genres.length > 0" class="cover-tags">
          <span
            v-for="genre in game.genres"
            :key="genre"
            class="cover-tag"
            :style="{ background: getGenreColor(genre) }"
          >
            {{ genre }}
          </span>
        </div>
      </div>
      <!-- 更换封面按钮 -->
      <div class="change-cover-section">
        <n-dropdown
          :options="coverDropdownOptions"
          trigger="click"
          @select="handleCoverDropdown"
        >
          <n-button size="small" quaternary>
            <template #icon>
              <n-icon :component="ImageOutline" />
            </template>
            更换封面
          </n-button>
        </n-dropdown>
      </div>

      <!-- 操作按钮 -->
      <div class="actions-section">
        <n-space>
          <n-button type="primary" size="large" @click="emit('launch')">
            <template #icon>
              <n-icon :component="PlayOutline" />
            </template>
            {{ isActive ? '游玩中' : '启动游戏' }}
          </n-button>
          <n-button
            :type="game.status === 'completed' ? 'success' : 'default'"
            @click="emit('setStatus', game.status === 'completed' ? 'unplayed' : 'completed')"
          >
            <template #icon>
              <n-icon :component="game.status === 'completed' ? CheckmarkCircleOutline : TrophyOutline" />
            </template>
            {{ game.status === 'completed' ? '已通关' : '标记通关' }}
          </n-button>
          <n-button
            :type="game.is_favorite ? 'error' : 'default'"
            @click="emit('favorite')"
          >
            <template #icon>
              <n-icon :component="game.is_favorite ? Heart : HeartOutline" />
            </template>
          </n-button>
          <n-button @click="emit('delete')">
            <template #icon>
              <n-icon :component="TrashOutline" />
            </template>
          </n-button>
        </n-space>
      </div>

      <n-divider />

      <!-- 统计信息 -->
      <div class="stats-section">
        <n-grid :cols="2" :x-gap="12" :y-gap="12">
          <n-gi>
            <div class="stat-card">
              <div class="stat-label">游玩时长</div>
              <div class="stat-value">{{ formatPlayTime(game.play_time_seconds) }}</div>
            </div>
          </n-gi>
          <n-gi>
            <div class="stat-card">
              <div class="stat-label">启动次数</div>
              <div class="stat-value">{{ game.play_count }}</div>
            </div>
          </n-gi>
          <n-gi>
            <div class="stat-card">
              <div class="stat-label">上次游玩</div>
              <div class="stat-value">{{ formatDate(game.last_played) }}</div>
            </div>
          </n-gi>
          <n-gi>
            <div class="stat-card">
              <div class="stat-label">入库时间</div>
              <div class="stat-value">{{ formatDate(game.added_at) }}</div>
            </div>
          </n-gi>
        </n-grid>
      </div>

      <!-- HLTB 通关时长信息 -->
      <div v-if="hasHltb" class="hltb-section">
        <div class="section-title">
          <n-icon :component="TimeOutline" size="14" />
          预估通关时长
        </div>
        <n-grid :cols="3" :x-gap="8" :y-gap="8">
          <n-gi v-if="game.hltb_main_story">
            <div class="hltb-card">
              <div class="hltb-label">主线</div>
              <div class="hltb-value">{{ formatMinutes(game.hltb_main_story) }}</div>
            </div>
          </n-gi>
          <n-gi v-if="game.hltb_main_extra">
            <div class="hltb-card">
              <div class="hltb-label">主线+支线</div>
              <div class="hltb-value">{{ formatMinutes(game.hltb_main_extra) }}</div>
            </div>
          </n-gi>
          <n-gi v-if="game.hltb_completionist">
            <div class="hltb-card">
              <div class="hltb-label">完美通关</div>
              <div class="hltb-value">{{ formatMinutes(game.hltb_completionist) }}</div>
            </div>
          </n-gi>
        </n-grid>
        <!-- 主线进度 -->
        <div v-if="game.hltb_main_story && playedMinutes > 0" class="hltb-progress">
          <div class="hltb-progress-header">
            <span>主线进度</span>
            <span>{{ hltbProgress }}%</span>
          </div>
          <n-progress
            type="line"
            :percentage="hltbProgress"
            :show-indicator="false"
            :height="8"
            :border-radius="4"
          />
          <div class="hltb-progress-footer">
            <span>已玩 {{ formatMinutes(playedMinutes) }}</span>
            <span v-if="hltbRemaining !== null && hltbRemaining > 0">
              剩余约 {{ formatMinutes(hltbRemaining) }}
            </span>
            <span v-else-if="hltbRemaining === 0" style="color: #22c55e">已通关！</span>
          </div>
        </div>
      </div>

      <!-- 最近游玩记录 -->
      <div v-if="recentSessions.length > 0" class="recent-sessions-section">
        <div class="section-title">
          <n-icon :component="TimeOutline" size="14" />
          最近游玩记录
        </div>
        <div class="session-list">
          <div
            v-for="session in recentSessions"
            :key="session.id"
            class="session-item"
          >
            <span class="session-date">{{ formatDate(session.start_time) }}</span>
            <span class="session-duration">{{ formatPlayTime(session.duration_seconds) }}</span>
          </div>
        </div>
      </div>

      <n-divider />

      <!-- 获取游戏信息按钮 -->
      <div v-if="!game.description" class="fetch-info-section">
        <n-space>
          <n-button
            size="small"
            type="primary"
            :loading="fetchingLlm"
            @click="handleFetchInfoLlm"
          >
            使用 LLM 获取
          </n-button>
          <n-button
            size="small"
            @click="showEditInfoModal = true"
          >
            手动填写
          </n-button>
        </n-space>
      </div>

      <!-- 游戏介绍 -->
      <div v-if="game.description" class="description-section">
        <p class="description">{{ game.description }}</p>
      </div>

      <!-- 游戏信息 -->
      <div class="info-section">
        <div class="info-row" v-if="game.developer">
          <span class="info-label">开发商</span>
          <span class="info-value">{{ game.developer }}</span>
        </div>
        <div class="info-row" v-if="game.publisher">
          <span class="info-label">发行商</span>
          <span class="info-value">{{ game.publisher }}</span>
        </div>
        <div class="info-row" v-if="game.release_date">
          <span class="info-label">发行日期</span>
          <span class="info-value">{{ game.release_date }}</span>
        </div>
        <div class="info-row clickable" v-if="game.exe_path" @click="handleOpenExeFolder">
          <span class="info-label">可执行文件</span>
          <span class="info-value path">{{ game.exe_path }}</span>
          <n-tooltip trigger="hover">
            <template #trigger>
              <n-icon :component="CreateOutline" size="14" class="edit-icon" @click.stop="handleChangeExePath" />
            </template>
            修改路径
          </n-tooltip>
        </div>
        <div class="info-row" v-if="game.exe_version">
          <span class="info-label">游戏版本</span>
          <span class="info-value">{{ game.exe_version }}</span>
        </div>
      </div>

      <!-- 存档路径 -->
      <div class="save-paths-section">
        <div class="section-title">
          <n-icon :component="FolderOpenOutline" size="14" />
          存档路径
          <n-button
            size="tiny"
            quaternary
            @click="handleAddSavePath"
            style="margin-left: auto"
          >
            <template #icon>
              <n-icon :component="AddOutline" />
            </template>
            添加
          </n-button>
        </div>

        <div v-if="game.save_paths && game.save_paths.length > 0">
          <div
            v-for="(path, index) in game.save_paths"
            :key="index"
            class="save-path-item clickable"
            @click="handleOpenSavePath(path)"
          >
            <span class="save-path-text" :title="path">{{ path }}</span>
            <n-icon
              :component="CreateOutline"
              size="14"
              class="edit-icon"
              @click.stop="handleChangeSavePath(index)"
            />
            <n-icon
              :component="CloseOutline"
              size="14"
              class="delete-icon"
              @click.stop="handleRemoveSavePath(index)"
            />
          </div>
        </div>
        <div v-else class="save-path-empty">
          暂无存档路径信息
        </div>
      </div>

    </n-drawer-content>
  </n-drawer>

  <!-- 封面选择器弹窗 -->
  <CoverPickerModal
    :show="showCoverPicker"
    :game-id="game.id"
    :game-name="game.name"
    @close="showCoverPicker = false"
    @cover-changed="emit('close')"
  />

  <!-- 手动填写游戏信息弹窗 -->
  <GameInfoEditModal
    :show="showEditInfoModal"
    :game="game"
    @close="showEditInfoModal = false"
    @saved="showEditInfoModal = false"
  />
</template>

<style scoped>
.detail-header {
  display: flex;
  align-items: center;
  gap: 8px;
}

.cover-section {
  margin-bottom: 8px;
  border-radius: 8px;
  overflow: hidden;
  position: relative;
}

.cover-tags {
  position: absolute;
  top: 8px;
  left: 8px;
  display: flex;
  gap: 4px;
  flex-wrap: wrap;
}

.cover-tag {
  font-size: 11px;
  color: #fff;
  padding: 2px 8px;
  border-radius: 4px;
}

.cover-img {
  width: 100%;
  height: auto;
  display: block;
}

.cover-placeholder {
  width: 100%;
  height: 200px;
  background: #2a2a3e;
  display: flex;
  align-items: center;
  justify-content: center;
  color: #555;
}

.change-cover-section {
  display: flex;
  justify-content: flex-end;
  margin-bottom: 16px;
}

.actions-section {
  margin-bottom: 16px;
}

.stats-section {
  margin-bottom: 16px;
}

.stat-card {
  background: rgba(255, 255, 255, 0.05);
  border-radius: 8px;
  padding: 12px;
  text-align: center;
}

.stat-label {
  font-size: 12px;
  color: #888;
  margin-bottom: 4px;
}

.stat-value {
  font-size: 16px;
  font-weight: 600;
  color: #e0e0e0;
}

.info-section {
  margin-bottom: 16px;
}

.info-row {
  display: flex;
  justify-content: space-between;
  padding: 8px 0;
  border-bottom: 1px solid rgba(255, 255, 255, 0.05);
}

.info-label {
  color: #888;
  font-size: 13px;
}

.info-value {
  color: #e0e0e0;
  font-size: 13px;
  max-width: 60%;
  text-align: right;
}

.info-value.path {
  font-size: 11px;
  word-break: break-all;
}

.info-row.clickable {
  cursor: pointer;
  align-items: center;
  gap: 8px;
  transition: background-color 0.2s;
  border-radius: 4px;
  padding: 8px 4px;
}

.info-row.clickable:hover {
  background-color: rgba(255, 255, 255, 0.05);
}

.info-row.clickable .edit-icon {
  opacity: 0.5;
  transition: opacity 0.2s;
  flex-shrink: 0;
  color: #888;
  cursor: pointer;
}

.info-row.clickable:hover .edit-icon {
  opacity: 1;
}

.description {
  font-size: 13px;
  line-height: 1.6;
  color: #aaa;
}

.fetch-info-section {
  margin-bottom: 16px;
}

.description-section {
  margin-bottom: 16px;
}

.hltb-section {
  margin-bottom: 16px;
}

.section-title {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  font-weight: 600;
  color: #aaa;
  margin-bottom: 10px;
}

.hltb-card {
  background: rgba(255, 255, 255, 0.05);
  border-radius: 6px;
  padding: 8px;
  text-align: center;
}

.hltb-label {
  font-size: 11px;
  color: #888;
  margin-bottom: 2px;
}

.hltb-value {
  font-size: 13px;
  font-weight: 600;
  color: #e0e0e0;
}

.hltb-progress {
  margin-top: 10px;
  padding: 10px;
  background: rgba(255, 255, 255, 0.03);
  border-radius: 6px;
}

.hltb-progress-header {
  display: flex;
  justify-content: space-between;
  font-size: 12px;
  color: #aaa;
  margin-bottom: 6px;
}

.hltb-progress-footer {
  display: flex;
  justify-content: space-between;
  font-size: 11px;
  color: #888;
  margin-top: 6px;
}

.recent-sessions-section {
  margin-bottom: 16px;
}

.session-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.session-item {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 8px 12px;
  background: rgba(255, 255, 255, 0.05);
  border-radius: 6px;
}

.session-date {
  font-size: 13px;
  color: #aaa;
}

.session-duration {
  font-size: 13px;
  font-weight: 600;
  color: #e0e0e0;
}

.save-paths-section {
  margin-bottom: 16px;
}

.save-path-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 4px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.05);
  border-radius: 4px;
  cursor: pointer;
  transition: background-color 0.2s;
}

.save-path-item:hover {
  background-color: rgba(255, 255, 255, 0.05);
}

.save-path-item .edit-icon,
.save-path-item .delete-icon {
  opacity: 0.3;
  transition: opacity 0.2s;
  flex-shrink: 0;
  color: #888;
  cursor: pointer;
}

.save-path-item:hover .edit-icon,
.save-path-item:hover .delete-icon {
  opacity: 1;
}

.save-path-item .delete-icon:hover {
  color: #e74c3c;
}

.save-path-text {
  flex: 1;
  font-size: 11px;
  color: #aaa;
  word-break: break-all;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.save-path-empty {
  font-size: 12px;
  color: #666;
  padding: 8px 0;
}
</style>
