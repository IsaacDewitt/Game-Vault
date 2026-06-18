<script setup lang="ts">
import { computed, ref, toRef } from "vue";
import { NIcon, NEllipsis } from "naive-ui";
import { HeartOutline, Heart, PlayOutline } from "@vicons/ionicons5";
import type { Game } from "../lib/tauri";
import { formatPlayTime } from "../lib/format";
import { useCoverImage } from "../lib/useCoverImage";
import ContextMenu from "./ContextMenu.vue";
import type { ContextMenuItem } from "./ContextMenu.vue";

const props = defineProps<{
  game: Game;
  isActive?: boolean;
}>();

const emit = defineEmits<{
  click: [];
  launch: [];
  favorite: [];
  delete: [];
  rename: [];
  refreshInfo: [];
  removeCover: [];
}>();

const showContextMenu = ref(false);
const contextMenuX = ref(0);
const contextMenuY = ref(0);

const { coverImage, imgFailed, handleImageError } = useCoverImage(toRef(props, "game"));

const contextMenuItems = computed<ContextMenuItem[]>(() => [
  {
    label: "启动游戏",
    icon: "▶",
    action: () => emit("launch"),
  },
  {
    label: props.game.is_favorite ? "取消收藏" : "添加收藏",
    icon: props.game.is_favorite ? "💔" : "❤️",
    action: () => emit("favorite"),
  },
  { label: "", action: () => {}, divider: true },
  {
    label: "查看详情",
    icon: "📋",
    action: () => emit("click"),
  },
  {
    label: "重命名游戏",
    icon: "✏️",
    action: () => emit("rename"),
  },
  {
    label: "刷新信息",
    icon: "🔄",
    action: () => emit("refreshInfo"),
  },
  {
    label: "删除封面",
    icon: "🖼️",
    action: () => emit("removeCover"),
  },
  { label: "", action: () => {}, divider: true },
  {
    label: "删除游戏",
    icon: "🗑️",
    action: () => emit("delete"),
    danger: true,
  },
]);

function handleContextMenu(e: MouseEvent) {
  e.preventDefault();
  e.stopPropagation();
  contextMenuX.value = e.clientX;
  contextMenuY.value = e.clientY;
  showContextMenu.value = true;
}
</script>

<template>
  <div
    class="game-card"
    :class="{ active: isActive }"
    @click="emit('click')"
    @contextmenu="handleContextMenu"
  >
    <!-- 封面图 -->
    <div class="cover">
      <img
        v-if="coverImage && !imgFailed"
        :src="coverImage"
        :alt="game.name"
        loading="lazy"
        @error="handleImageError"
      />
      <div v-else class="cover-placeholder">
        <span>{{ game.name.charAt(0) }}</span>
      </div>

      <!-- 操作按钮 -->
      <div class="actions">
        <button class="action-btn launch-btn" @click.stop="emit('launch')" title="启动">
          <n-icon :component="PlayOutline" />
        </button>
        <button
          class="action-btn fav-btn"
          :class="{ favorited: game.is_favorite }"
          @click.stop="emit('favorite')"
          title="收藏"
        >
          <n-icon :component="game.is_favorite ? Heart : HeartOutline" />
        </button>
      </div>

      <!-- 正在游玩 -->
      <div v-if="isActive" class="playing-badge">
        <span class="pulse"></span> 游玩中
      </div>
    </div>

    <!-- 游戏信息 -->
    <div class="info">
      <div class="name">
        <n-ellipsis :tooltip="false">{{ game.name }}</n-ellipsis>
      </div>
      <div class="meta">
        <span v-if="game.play_time_seconds > 0" class="play-time">
          {{ formatPlayTime(game.play_time_seconds) }}
        </span>
      </div>
    </div>
  </div>

  <!-- 右键菜单 -->
  <ContextMenu
    v-if="showContextMenu"
    :items="contextMenuItems"
    :x="contextMenuX"
    :y="contextMenuY"
    @close="showContextMenu = false"
  />
</template>

<style scoped>
.game-card {
  cursor: pointer;
  border-radius: 8px;
  overflow: hidden;
  background: rgba(255, 255, 255, 0.05);
  transition: all 0.2s ease;
  position: relative;
}

.game-card:hover {
  transform: translateY(-4px);
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);
}

.game-card.active {
  border: 2px solid #6366f1;
}

.cover {
  position: relative;
  width: 100%;
  aspect-ratio: 3/4;
  overflow: hidden;
  background: #2a2a3e;
}

.cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.cover-placeholder {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 48px;
  font-weight: 700;
  color: rgba(255, 255, 255, 0.2);
  background: linear-gradient(135deg, #2a2a3e 0%, #1a1a2e 100%);
}

.actions {
  position: absolute;
  bottom: 8px;
  right: 8px;
  display: flex;
  gap: 4px;
  opacity: 0;
  transition: opacity 0.2s;
}

.game-card:hover .actions {
  opacity: 1;
}

.action-btn {
  width: 32px;
  height: 32px;
  border-radius: 50%;
  border: none;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: all 0.2s;
}

.launch-btn {
  background: #6366f1;
  color: white;
}

.launch-btn:hover {
  background: #4f46e5;
}

.fav-btn {
  background: rgba(0, 0, 0, 0.6);
  color: white;
}

.fav-btn.favorited {
  color: #ef4444;
}

.playing-badge {
  position: absolute;
  top: 8px;
  right: 8px;
  padding: 4px 10px;
  border-radius: 4px;
  font-size: 11px;
  color: white;
  background: rgba(99, 102, 241, 0.9);
  display: flex;
  align-items: center;
  gap: 6px;
}

.pulse {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #4ade80;
  animation: pulse 1.5s infinite;
}

@keyframes pulse {
  0% { opacity: 1; }
  50% { opacity: 0.4; }
  100% { opacity: 1; }
}

.info {
  padding: 10px 12px;
}

.name {
  font-size: 13px;
  font-weight: 500;
  color: #e0e0e0;
  margin-bottom: 4px;
}

.meta {
  font-size: 11px;
  color: #888;
}
</style>
