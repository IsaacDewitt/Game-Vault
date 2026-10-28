<script setup lang="ts">
import { computed, ref } from "vue";
import { NDrawer, NDrawerContent } from "naive-ui";
import type { GameAchievements } from "../lib/tauri";
import { displayName, useGamesStore } from "../stores/games";
import {
  CATEGORY_META,
  categoryColor,
  computeGameAchievementProgress,
  formatAchievementProgress,
  groupAchievements,
  groupProgress,
  type AchievementGroup,
} from "../lib/achievements";
import { formatDate } from "../lib/format";

const props = defineProps<{
  entry: GameAchievements;
}>();

const emit = defineEmits<{
  close: [];
}>();

const store = useGamesStore();

const game = computed(() => store.games.find((g) => g.id === props.entry.game_id) ?? null);
const name = computed(() => (game.value ? displayName(game.value) : props.entry.game_name));
const coverSrc = computed(() => store.coverSrc(props.entry.game_id, false));
const imgFailed = ref(false);

const progress = computed(() => computeGameAchievementProgress(props.entry.achievements));
const groups = computed(() => groupAchievements(props.entry.achievements));

/** 组状态文案：全解锁就不必再报「进度到哪一级」 */
function groupText(group: AchievementGroup): string {
  if (group.unlockedCount === group.items.length) return "已全部解锁";
  return formatAchievementProgress(groupProgress(group));
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
    <n-drawer-content
      title="游戏成就"
      closable
      :native-scrollbar="false"
      @close="emit('close')"
      @contextmenu.prevent
    >
      <!-- 游戏头部 -->
      <div class="hero">
        <div class="hero-cover">
          <img
            v-if="coverSrc && !imgFailed"
            :src="coverSrc"
            :alt="name"
            @error="imgFailed = true"
          />
          <div v-else class="hero-cover-placeholder">{{ name.charAt(0) }}</div>
        </div>
        <div class="hero-main">
          <div class="hero-name" :title="name">{{ name }}</div>
          <div class="hero-count">
            <span class="hero-num">{{ progress.unlocked }}</span>
            <span class="hero-total"> / {{ progress.attainable }} 已解锁</span>
            <span class="hero-percent" :class="{ done: progress.completed }">
              {{ progress.percent }}%
            </span>
          </div>
          <div class="hero-track">
            <div class="hero-fill" :style="{ width: progress.percent + '%' }"></div>
          </div>
          <div v-if="progress.completed" class="hero-done">🏆 全成就达成</div>
        </div>
      </div>

      <!-- 已删除留档说明：解锁记录按设计保留，但条目已不在库中 -->
      <div v-if="entry.removed" class="note removed-note">
        该游戏已从库中删除，此处展示的是其历史解锁记录——成就记录不随删除消失。
      </div>

      <!-- 完成度口径说明：缺 HLTB 参考时长的条目不计入分母 -->
      <div v-if="progress.skipped > 0" class="note skipped-note">
        另有 {{ progress.skipped }} 条成就因缺少 HLTB 参考时长，无法达成，不计入完成度。
      </div>

      <!-- 成就列表 -->
      <div class="achv-list">
        <div
          v-for="group in groups"
          :key="group.name"
          class="achv-row"
          :class="{ unlocked: group.unlockedCount === group.items.length }"
        >
          <div
            class="row-icon"
            :style="{ background: categoryColor(group.category) + '22' }"
          >
            {{ group.icon }}
          </div>
          <div class="row-main">
            <div class="row-title">
              <span class="row-name">{{ group.name }}</span>
              <span v-if="group.items.length > 1" class="tier-badge">
                {{ group.unlockedCount }}/{{ group.items.length }}
              </span>
              <span
                class="cat-badge"
                :style="{
                  color: categoryColor(group.category),
                  background: categoryColor(group.category) + '1a',
                }"
              >
                {{ CATEGORY_META[group.category]?.label ?? group.category }}
              </span>
            </div>
            <div class="row-desc">{{ group.desc }}</div>
            <div class="row-foot">
              <div v-if="group.items.length > 1" class="tier-row">
                <span
                  v-for="(item, idx) in group.items"
                  :key="item.def.id"
                  class="tier-dot"
                  :class="{ on: item.unlocked }"
                  :title="`第 ${idx + 1} 级`"
                ></span>
              </div>
              <span class="row-progress">{{ groupText(group) }}</span>
              <span v-if="group.lastUnlockedAt" class="row-time">
                {{ formatDate(group.lastUnlockedAt) }}解锁
              </span>
            </div>
          </div>
        </div>
      </div>
    </n-drawer-content>
  </n-drawer>
</template>

<style scoped>
.hero {
  display: flex;
  gap: 14px;
  padding: 4px 0 16px;
  border-bottom: 1px solid var(--border, #262d3d);
  margin-bottom: 14px;
}

.hero-cover {
  width: 72px;
  height: 96px;
  border-radius: 8px;
  overflow: hidden;
  background: #2a2a3e;
  flex-shrink: 0;
}

.hero-cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.hero-cover-placeholder {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 28px;
  font-weight: 700;
  color: rgba(255, 255, 255, 0.25);
  background: linear-gradient(135deg, #2a2a3e 0%, #1a1a2e 100%);
}

.hero-main {
  min-width: 0;
  flex: 1;
  display: flex;
  flex-direction: column;
  justify-content: center;
  gap: 7px;
}

.hero-name {
  font-size: 16px;
  font-weight: 700;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.hero-count {
  display: flex;
  align-items: baseline;
  gap: 2px;
  font-size: 12.5px;
  color: var(--text-dim, #9aa4b8);
}

.hero-num {
  font-size: 20px;
  font-weight: 800;
  color: var(--accent-color, #6366f1);
  line-height: 1;
}

.hero-percent {
  margin-left: auto;
  font-weight: 700;
  color: var(--text-dim, #9aa4b8);
}

.hero-percent.done {
  color: #fbbf24;
}

.hero-track {
  height: 7px;
  border-radius: 999px;
  background: var(--panel-2, #1b2130);
  overflow: hidden;
}

.hero-fill {
  height: 100%;
  border-radius: 999px;
  background: linear-gradient(90deg, var(--accent-color, #6366f1), #fbbf24);
  transition: width 0.5s ease;
}

.hero-done {
  font-size: 12px;
  font-weight: 700;
  color: #fbbf24;
}

.note {
  padding: 9px 12px;
  border-radius: 8px;
  font-size: 12px;
  line-height: 1.6;
  margin-bottom: 12px;
}

.removed-note {
  color: var(--text-secondary, #9aa4b2);
  background: var(--panel, #161b24);
  border: 1px dashed var(--border, #262d3d);
}

.skipped-note {
  color: var(--text-dim, #9aa4b8);
  background: color-mix(in srgb, var(--accent-color, #6366f1) 8%, transparent);
  border: 1px solid color-mix(in srgb, var(--accent-color, #6366f1) 22%, transparent);
}

.achv-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding-bottom: 16px;
}

.achv-row {
  display: flex;
  gap: 12px;
  padding: 12px;
  border-radius: 10px;
  background: var(--panel, #161b24);
  border: 1px solid var(--border, #262d3d);
  opacity: 0.78;
  transition: opacity 0.2s, border-color 0.2s;
}

.achv-row.unlocked {
  opacity: 1;
  border-color: color-mix(in srgb, #fbbf24 40%, var(--border, #262d3d));
  background: linear-gradient(
    160deg,
    color-mix(in srgb, #fbbf24 5%, var(--panel, #161b24)),
    var(--panel, #161b24)
  );
}

.row-icon {
  width: 38px;
  height: 38px;
  border-radius: 9px;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 19px;
  flex-shrink: 0;
}

.row-main {
  min-width: 0;
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 5px;
}

.row-title {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.row-name {
  font-size: 13.5px;
  font-weight: 700;
}

.tier-badge {
  font-size: 10.5px;
  font-weight: 700;
  color: #fbbf24;
  background: color-mix(in srgb, #fbbf24 14%, transparent);
  padding: 1px 7px;
  border-radius: 999px;
}

.cat-badge {
  font-size: 10.5px;
  font-weight: 600;
  padding: 1px 7px;
  border-radius: 5px;
}

.row-desc {
  font-size: 12px;
  color: var(--text-dim, #9aa4b8);
  line-height: 1.5;
}

.row-foot {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  margin-top: 1px;
}

.tier-row {
  display: flex;
  gap: 4px;
}

.tier-dot {
  width: 18px;
  height: 5px;
  border-radius: 999px;
  background: var(--panel-2, #1b2130);
  border: 1px solid var(--border, #262d3d);
}

.tier-dot.on {
  background: linear-gradient(90deg, var(--accent-color, #6366f1), #fbbf24);
  border-color: transparent;
}

.row-progress {
  font-size: 11.5px;
  font-weight: 600;
  color: var(--text-dim, #9aa4b8);
}

.row-time {
  margin-left: auto;
  font-size: 11px;
  color: #fbbf24;
  flex-shrink: 0;
}
</style>
