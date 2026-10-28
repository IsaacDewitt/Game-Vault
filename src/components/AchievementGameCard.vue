<script setup lang="ts">
import { computed, ref } from "vue";
import type { GameAchievements } from "../lib/tauri";
import { displayName, useGamesStore } from "../stores/games";
import { computeGameAchievementProgress } from "../lib/achievements";

const props = defineProps<{
  entry: GameAchievements;
}>();

const emit = defineEmits<{
  click: [];
}>();

const store = useGamesStore();

/** 库内条目走 store（可拿到英文名、封面）；已删除条目只有汇总里的中文名 */
const game = computed(() => store.games.find((g) => g.id === props.entry.game_id) ?? null);
const name = computed(() => (game.value ? displayName(game.value) : props.entry.game_name));
const coverSrc = computed(() => store.coverSrc(props.entry.game_id, true));
const imgFailed = ref(false);

const progress = computed(() => computeGameAchievementProgress(props.entry.achievements));
</script>

<template>
  <div
    class="achievement-card"
    :class="{ completed: progress.completed, removed: entry.removed }"
    role="button"
    tabindex="0"
    @click="emit('click')"
    @keydown.enter="emit('click')"
  >
    <div class="cover">
      <img
        v-if="coverSrc && !imgFailed"
        :src="coverSrc"
        :alt="name"
        loading="lazy"
        @error="imgFailed = true"
      />
      <div v-else class="cover-placeholder">{{ name.charAt(0) }}</div>

      <div v-if="progress.completed" class="full-badge">🏆 全成就</div>
      <div v-if="entry.removed" class="removed-badge">已删除</div>
    </div>

    <div class="info">
      <div class="name" :title="name">{{ name }}</div>
      <div
        class="progress-row"
        :title="
          progress.skipped
            ? `另有 ${progress.skipped} 条成就因缺少 HLTB 参考时长，不计入完成度`
            : undefined
        "
      >
        <span class="progress-count">{{ progress.unlocked }} / {{ progress.attainable }}</span>
        <span class="progress-percent">{{ progress.percent }}%</span>
      </div>
      <div class="progress-track">
        <div class="progress-fill" :style="{ width: progress.percent + '%' }"></div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 尺寸与观感对齐游戏库卡片（GameCard.vue）：
   圆角 8px、3:4 封面、宽度上限 220px（与后端缩略图 512 预算配套，别单独调） */
.achievement-card {
  cursor: pointer;
  border-radius: 8px;
  overflow: hidden;
  background: rgba(255, 255, 255, 0.05);
  transition: transform 0.2s ease, box-shadow 0.2s ease, border-color 0.2s ease;
  position: relative;
  max-width: 220px;
  border: 1px solid transparent;
  outline: none;
}

.achievement-card:hover,
.achievement-card:focus-visible {
  transform: translateY(-4px);
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.3);
}

.achievement-card.completed {
  border-color: color-mix(in srgb, #fbbf24 55%, transparent);
}

.achievement-card.removed {
  opacity: 0.72;
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

.full-badge {
  position: absolute;
  top: 8px;
  left: 8px;
  padding: 3px 8px;
  border-radius: 4px;
  font-size: 11px;
  color: #1a1a1a;
  background: linear-gradient(135deg, #fcd34d, #fbbf24);
  font-weight: 700;
}

.removed-badge {
  position: absolute;
  top: 8px;
  right: 8px;
  padding: 3px 8px;
  border-radius: 4px;
  font-size: 11px;
  color: #e5e7eb;
  background: rgba(0, 0, 0, 0.66);
}

.info {
  padding: 10px 12px 12px;
}

.name {
  font-size: 13px;
  font-weight: 500;
  color: #e0e0e0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.progress-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin: 6px 0 5px;
  font-size: 11px;
  color: #888;
}

.progress-count {
  font-variant-numeric: tabular-nums;
}

.progress-percent {
  font-weight: 600;
  color: var(--text-dim, #9aa4b8);
  font-variant-numeric: tabular-nums;
}

.progress-track {
  height: 6px;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.08);
  overflow: hidden;
}

.progress-fill {
  height: 100%;
  border-radius: 999px;
  background: linear-gradient(90deg, var(--accent-color, #6366f1), #fbbf24);
  transition: width 0.5s ease;
}

.achievement-card.completed .progress-fill {
  background: linear-gradient(90deg, #fbbf24, #f59e0b);
}

.achievement-card.completed .progress-percent {
  color: #fbbf24;
}
</style>
