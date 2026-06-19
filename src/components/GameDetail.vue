<script setup lang="ts">
import { ref, toRef, computed } from "vue";
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
} from "@vicons/ionicons5";
import { open } from "@tauri-apps/plugin-dialog";
import type { Game } from "../lib/tauri";
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

const store = useGamesStore();
const message = useMessage();
const fetchingLlm = ref(false);

// HLTB 数据计算
const hasHltb = computed(() =>
  props.game.hltb_main_story || props.game.hltb_main_extra || props.game.hltb_completionist
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

const { coverImage, imgFailed, handleImageError } = useCoverImage(toRef(props, "game"));

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
</script>

<template>
  <n-drawer
    :show="true"
    :width="400"
    placement="right"
    @update:show="(val) => !val && emit('close')"
  >
    <n-drawer-content :native-scrollbar="false">
      <template #header>
        <div class="detail-header">
          <span>{{ game.name }}</span>
        </div>
      </template>

      <!-- 封面图 -->
      <div class="cover-section">
        <img
          v-if="coverImage && !imgFailed"
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
        <n-tooltip trigger="hover">
          <template #trigger>
            <n-button
              size="small"
              quaternary
              @click="handleChangeCover"
            >
              <template #icon>
                <n-icon :component="ImageOutline" />
              </template>
              更换封面
            </n-button>
          </template>
          选择本地图片作为封面
        </n-tooltip>
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

      <n-divider />

      <!-- 获取游戏信息按钮 -->
      <div v-if="!game.description" class="fetch-info-section">
        <n-button
          size="small"
          type="primary"
          :loading="fetchingLlm"
          @click="handleFetchInfoLlm"
        >
          获取游戏信息
        </n-button>
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
        <div class="info-row" v-if="game.exe_path">
          <span class="info-label">可执行文件</span>
          <span class="info-value path">{{ game.exe_path }}</span>
        </div>
        <div class="info-row" v-if="game.install_path">
          <span class="info-label">安装路径</span>
          <span class="info-value path">{{ game.install_path }}</span>
        </div>
      </div>

    </n-drawer-content>
  </n-drawer>
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
</style>
