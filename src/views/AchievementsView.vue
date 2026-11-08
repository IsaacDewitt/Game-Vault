<script setup lang="ts">
import { ref, computed, onActivated, onDeactivated } from "vue";
import {
  NSpin,
  NEmpty,
  NSelect,
  NInput,
  NButton,
  NButtonGroup,
  NIcon,
  useMessage,
} from "naive-ui";
import { SearchOutline } from "@vicons/ionicons5";
import * as api from "../lib/tauri";
import type { AchievementSummary, Game, GameAchievements } from "../lib/tauri";
import { displayName, useGamesStore } from "../stores/games";
import {
  CATEGORY_META,
  categoryColor,
  computeGameAchievementProgress,
  groupAchievements,
  groupStatusText,
  type AchievementGroup,
  type GameAchievementProgress,
} from "../lib/achievements";
import AchievementGameCard from "../components/AchievementGameCard.vue";
import GameAchievementsDrawer from "../components/GameAchievementsDrawer.vue";
import { formatDate } from "../lib/format";

const message = useMessage();
const store = useGamesStore();
const summary = ref<AchievementSummary | null>(null);
const loading = ref(true);
const activeCategory = ref<string | null>(null);

// 单游戏成就区：搜索 / 筛选 / 排序 / 抽屉
const searchQuery = ref("");
type StatusFilter = "all" | "in_progress" | "completed" | "not_started";
const statusFilter = ref<StatusFilter>("all");
type SortKey = "progress" | "unlocked" | "name";
const sortKey = ref<SortKey>("progress");
const selectedGameId = ref<string | null>(null);

const statusOptions: { value: StatusFilter; label: string }[] = [
  { value: "all", label: "全部" },
  { value: "in_progress", label: "进行中" },
  { value: "completed", label: "已全成就" },
  { value: "not_started", label: "未解锁" },
];

const sortOptions: { value: SortKey; label: string }[] = [
  { value: "progress", label: "完成度（高→低）" },
  { value: "unlocked", label: "已解锁数（多→少）" },
  { value: "name", label: "名称" },
];

// 分类筛选（仅作用于全局成就区）
const categoryTabs = [
  { key: null, label: "全部" },
  { key: "progress", label: "进度" },
  { key: "collect", label: "收集" },
  { key: "fun", label: "趣味" },
  { key: "challenge", label: "挑战" },
];

async function load() {
  loading.value = true;
  try {
    summary.value = await api.getAchievements();
  } catch (e) {
    message.error(`加载成就失败: ${e}`);
  } finally {
    loading.value = false;
  }
}

// 用 onActivated 替代 onMounted：组件被 keep-alive 缓存后，每次切回成就页都会重新拉取数据
onActivated(load);

// 离开页面时收起抽屉：keep-alive 会保留组件状态，否则切回来时抽屉还开着
onDeactivated(() => {
  selectedGameId.value = null;
});

/** 全局成就分组（多级成就合并），未解锁的排前面、解锁多的靠前 */
const globalGroups = computed<AchievementGroup[]>(() => {
  if (!summary.value) return [];
  const groups = groupAchievements(
    activeCategory.value
      ? summary.value.global.filter((i) => i.def.category === activeCategory.value)
      : summary.value.global
  );
  return groups.sort((a, b) => {
    const aDone = a.unlockedCount === a.items.length ? 1 : 0;
    const bDone = b.unlockedCount === b.items.length ? 1 : 0;
    if (aDone !== bDone) return aDone - bDone;
    return b.unlockedCount - a.unlockedCount;
  });
});

// 概览统计
const stats = computed(() => {
  if (!summary.value) {
    return { unlocked: 0, total: 0, percent: 0, gamesWithProgress: 0, gamesPlayable: 0 };
  }
  const { unlocked_count, total_count, per_game } = summary.value;
  // 与相邻的「可解锁游戏」保持同一分母：已删除的留档条目不再结算，不进这两个数
  const gamesWithProgress = per_game.filter(
    (g) => !g.removed && g.achievements.some((a) => a.unlocked)
  ).length;
  // 「可解锁游戏」只数在库条目：已删除的留档条目不再参与结算，算进去会虚高
  const gamesPlayable = per_game.filter((g) => !g.removed).length;
  const percent = total_count > 0 ? Math.round((unlocked_count / total_count) * 100) : 0;
  return { unlocked: unlocked_count, total: total_count, percent, gamesWithProgress, gamesPlayable };
});

const gameMap = computed(() => {
  const map = new Map<string, Game>();
  for (const g of store.games) map.set(g.id, g);
  return map;
});

interface GameCardVM {
  entry: GameAchievements;
  name: string;
  progress: GameAchievementProgress;
}

/** 卡片展示名与游戏库同口径：库内条目英文名优先，已删除条目退回汇总里的名字 */
function entryName(entry: GameAchievements): string {
  const game = gameMap.value.get(entry.game_id);
  return game ? displayName(game) : entry.game_name;
}

const allCards = computed<GameCardVM[]>(() => {
  if (!summary.value) return [];
  return summary.value.per_game
    .filter((entry) => entry.achievements.length > 0)
    .map((entry) => ({
      entry,
      name: entryName(entry),
      progress: computeGameAchievementProgress(entry.achievements),
    }));
});

function matchesSearch(card: GameCardVM): boolean {
  const query = searchQuery.value.trim().toLowerCase();
  if (!query) return true;
  const game = gameMap.value.get(card.entry.game_id);
  const haystack = [card.name, card.entry.game_name, game?.name, game?.name_en]
    .filter(Boolean)
    .join(" ")
    .toLowerCase();
  return haystack.includes(query);
}

function matchesStatus(card: GameCardVM): boolean {
  const { unlocked, completed } = card.progress;
  switch (statusFilter.value) {
    case "in_progress":
      return unlocked > 0 && !completed;
    case "completed":
      return completed;
    case "not_started":
      return unlocked === 0;
    default:
      return true;
  }
}

function sortCards(list: GameCardVM[]): GameCardVM[] {
  const sorted = [...list];
  sorted.sort((a, b) => {
    if (sortKey.value === "name") {
      return a.name.localeCompare(b.name, "zh-Hans-CN");
    }
    if (sortKey.value === "unlocked") {
      if (b.progress.unlocked !== a.progress.unlocked) {
        return b.progress.unlocked - a.progress.unlocked;
      }
      return a.name.localeCompare(b.name, "zh-Hans-CN");
    }
    if (b.progress.percent !== a.progress.percent) {
      return b.progress.percent - a.progress.percent;
    }
    if (b.progress.unlocked !== a.progress.unlocked) {
      return b.progress.unlocked - a.progress.unlocked;
    }
    return a.name.localeCompare(b.name, "zh-Hans-CN");
  });
  return sorted;
}

const visibleCards = computed(() =>
  allCards.value.filter((card) => matchesSearch(card) && matchesStatus(card))
);

// 在库游戏与已删除留档分开展示：后者沉在页面最底，但一直可见，不靠下拉里翻找
const liveCards = computed(() =>
  sortCards(visibleCards.value.filter((card) => !card.entry.removed))
);
const removedCards = computed(() =>
  sortCards(visibleCards.value.filter((card) => card.entry.removed))
);

const selectedEntry = computed(
  () => summary.value?.per_game.find((g) => g.game_id === selectedGameId.value) ?? null
);
</script>

<template>
  <div class="achievements-view">
    <div class="page-header">
      <h1>🏆 成就</h1>
      <p class="subtitle">全局成就 + 每款游戏独立成就 · 数据自动结算，无需手动打卡</p>
    </div>

    <n-spin :show="loading">
      <template v-if="summary">
        <!-- 概览卡片 -->
        <div class="overview-card">
          <div class="overview-main">
            <div class="overview-numbers">
              <span class="big-number">{{ stats.unlocked }}</span>
              <span class="total">/ {{ stats.total }}</span>
              <span class="label">已解锁成就</span>
            </div>
            <div class="overview-progress">
              <div class="progress-track">
                <div class="progress-fill" :style="{ width: stats.percent + '%' }"></div>
              </div>
              <span class="percent">{{ stats.percent }}%</span>
            </div>
          </div>
          <div class="overview-side">
            <div class="side-item">
              <span class="side-num">{{ summary.global.length }}</span>
              <span class="side-label">全局成就</span>
            </div>
            <div class="side-item">
              <span class="side-num">{{ stats.gamesPlayable }}</span>
              <span class="side-label">可解锁游戏</span>
            </div>
            <div class="side-item">
              <span class="side-num">{{ stats.gamesWithProgress }}</span>
              <span class="side-label">已获成就的游戏</span>
            </div>
          </div>
        </div>

        <!-- 全局成就 -->
        <div class="section">
          <div class="section-header">
            <h2>🌐 全局成就</h2>
            <div class="cat-tabs">
              <button
                v-for="tab in categoryTabs"
                :key="tab.key ?? 'all'"
                class="cat-tab"
                :class="{ active: activeCategory === tab.key }"
                @click="activeCategory = tab.key"
              >
                {{ tab.label }}
              </button>
            </div>
          </div>

          <div v-if="globalGroups.length" class="achv-grid">
            <div
              v-for="group in globalGroups"
              :key="group.name"
              class="achv-card"
              :class="{ unlocked: group.unlockedCount === group.items.length }"
            >
              <div class="card-top">
                <span class="achv-icon" :style="{ background: categoryColor(group.category) + '22' }">
                  {{ group.icon }}
                </span>
                <div class="card-title">
                  <div class="name-row">
                    <span class="achv-name">{{ group.name }}</span>
                    <span v-if="group.items.length > 1" class="tier-badge">
                      {{ group.unlockedCount }}/{{ group.items.length }}
                    </span>
                  </div>
                  <span class="cat-badge" :style="{ color: categoryColor(group.category), background: categoryColor(group.category) + '1a' }">
                    {{ CATEGORY_META[group.category]?.label ?? group.category }}
                  </span>
                </div>
              </div>
              <p class="achv-desc">{{ group.desc }}</p>

              <!-- 多级成就等级条 -->
              <div v-if="group.items.length > 1" class="tier-row">
                <span
                  v-for="(item, idx) in group.items"
                  :key="item.def.id"
                  class="tier-dot"
                  :class="{ on: item.unlocked }"
                  :title="`第 ${idx + 1} 级`"
                ></span>
              </div>

              <div class="card-foot">
                <span class="progress-text">
                  {{ groupStatusText(group) }}
                </span>
                <span v-if="group.lastUnlockedAt" class="unlock-time">
                  {{ formatDate(group.lastUnlockedAt) }}解锁
                </span>
              </div>
            </div>
          </div>
          <n-empty v-else description="该分类下暂无成就" style="padding: 40px 0" />
        </div>

        <!-- 单游戏成就：卡片网格 + 点击进抽屉（与游戏库同一种浏览方式） -->
        <div class="section">
          <div class="section-header">
            <h2>🎮 游戏成就</h2>
            <div class="game-toolbar">
              <n-input
                v-model:value="searchQuery"
                placeholder="搜索游戏..."
                clearable
                size="small"
                style="width: 220px"
              >
                <template #prefix>
                  <n-icon :component="SearchOutline" />
                </template>
              </n-input>
              <n-select
                v-model:value="sortKey"
                :options="sortOptions"
                size="small"
                style="width: 165px"
              />
            </div>
          </div>

          <div class="filter-row">
            <n-button-group>
              <n-button
                v-for="opt in statusOptions"
                :key="opt.value"
                :type="statusFilter === opt.value ? 'primary' : 'default'"
                size="small"
                @click="statusFilter = opt.value"
              >
                {{ opt.label }}
              </n-button>
            </n-button-group>
            <span class="result-count">
              共 {{ liveCards.length }} 款{{ removedCards.length ? ` + ${removedCards.length} 款留档` : "" }}
            </span>
          </div>

          <div v-if="liveCards.length" class="game-grid">
            <AchievementGameCard
              v-for="card in liveCards"
              :key="card.entry.game_id"
              :entry="card.entry"
              @click="selectedGameId = card.entry.game_id"
            />
          </div>
          <n-empty
            v-else
            :description="
              allCards.length
                ? removedCards.length
                  ? '在库游戏中没有符合条件的条目，已删除的见下方'
                  : '没有符合条件的游戏'
                : '库中还没有可结算成就的游戏'
            "
            style="padding: 40px 0"
          />

          <!-- 已删除留档：独立分区，灰度卡片，始终可见 -->
          <div v-if="removedCards.length" class="removed-block">
            <div class="removed-title">
              🗑️ 已删除的游戏
              <span class="removed-count">{{ removedCards.length }}</span>
            </div>
            <p class="removed-desc">
              这些游戏已从库中删除，成就解锁记录按设计留档，点击仍可查看。
            </p>
            <div class="game-grid">
              <AchievementGameCard
                v-for="card in removedCards"
                :key="card.entry.game_id"
                :entry="card.entry"
                @click="selectedGameId = card.entry.game_id"
              />
            </div>
          </div>
        </div>
      </template>
    </n-spin>

    <!-- 单游戏成就详情抽屉 -->
    <GameAchievementsDrawer
      v-if="selectedEntry"
      :entry="selectedEntry"
      @close="selectedGameId = null"
    />
  </div>
</template>

<style scoped>
.achievements-view {
  max-width: 1100px;
  margin: 0 auto;
}

.page-header h1 {
  font-size: 24px;
  font-weight: 700;
  margin-bottom: 4px;
}

.subtitle {
  color: var(--text-dim, #9aa4b8);
  font-size: 13.5px;
  margin-bottom: 20px;
}

/* 概览卡片 */
.overview-card {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-wrap: wrap;
  gap: 20px;
  background: var(--panel, #161b24);
  border: 1px solid var(--border, #262d3d);
  border-radius: 14px;
  padding: 22px 28px;
  margin-bottom: 28px;
}

.overview-main {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.overview-numbers {
  display: flex;
  align-items: baseline;
  gap: 6px;
}

.big-number {
  font-size: 42px;
  font-weight: 800;
  color: var(--accent-color, #6366f1);
  line-height: 1;
}

.total {
  font-size: 18px;
  color: var(--text-dim, #9aa4b8);
}

.label {
  margin-left: 10px;
  font-size: 14px;
  color: var(--text-dim, #9aa4b8);
}

.overview-progress {
  display: flex;
  align-items: center;
  gap: 12px;
}

.progress-track {
  width: 260px;
  height: 8px;
  background: var(--panel-2, #1b2130);
  border-radius: 999px;
  overflow: hidden;
}

.progress-fill {
  height: 100%;
  background: linear-gradient(90deg, var(--accent-color, #6366f1), #fbbf24);
  border-radius: 999px;
  transition: width 0.6s ease;
}

.percent {
  font-size: 13px;
  color: var(--text-dim, #9aa4b8);
  font-weight: 600;
}

.overview-side {
  display: flex;
  gap: 28px;
}

.side-item {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 2px;
}

.side-num {
  font-size: 20px;
  font-weight: 700;
}

.side-label {
  font-size: 12px;
  color: var(--text-dim, #9aa4b8);
}

/* 分区 */
.section {
  margin-bottom: 32px;
}

.section-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-wrap: wrap;
  gap: 12px;
  margin-bottom: 16px;
}

.section-header h2 {
  font-size: 17px;
  font-weight: 700;
}

.cat-tabs {
  display: flex;
  gap: 6px;
}

.cat-tab {
  padding: 4px 14px;
  border-radius: 999px;
  font-size: 12.5px;
  border: 1px solid var(--border, #262d3d);
  background: transparent;
  color: var(--text-dim, #9aa4b8);
  cursor: pointer;
  transition: all 0.15s;
}

.cat-tab:hover {
  color: var(--text);
}

.cat-tab.active {
  color: var(--accent-color, #6366f1);
  border-color: var(--accent-color, #6366f1);
  background: color-mix(in srgb, var(--accent-color, #6366f1) 12%, transparent);
}

/* 成就网格 */
.achv-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
  gap: 12px;
}

/* 单游戏区工具条 */
.game-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
}

.filter-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-wrap: wrap;
  gap: 12px;
  margin-bottom: 14px;
}

.result-count {
  font-size: 12.5px;
  color: var(--text-dim, #9aa4b8);
}

/* 游戏卡片网格：列数算法与游戏库一致（HomeView .game-grid），
   卡片宽度上限在 AchievementGameCard 的 max-width（220px）上 */
.game-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
  gap: 16px;
}

/* 已删除留档分区 */
.removed-block {
  margin-top: 26px;
  padding-top: 18px;
  border-top: 1px dashed var(--border, #262d3d);
}

.removed-title {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 14px;
  font-weight: 700;
  color: var(--text-secondary, #9aa4b2);
}

.removed-count {
  font-size: 11px;
  font-weight: 700;
  padding: 1px 8px;
  border-radius: 999px;
  color: var(--text-dim, #9aa4b8);
  background: var(--panel-2, #1b2130);
}

.removed-desc {
  margin: 6px 0 14px;
  font-size: 12.5px;
  color: var(--text-dim, #9aa4b8);
}

.achv-card {
  background: var(--panel, #161b24);
  border: 1px solid var(--border, #262d3d);
  border-radius: 12px;
  padding: 16px;
  display: flex;
  flex-direction: column;
  gap: 10px;
  transition: all 0.2s;
  filter: grayscale(0.35);
  opacity: 0.72;
}

.achv-card:hover {
  transform: translateY(-2px);
  border-color: var(--border, #3a4458);
}

.achv-card.unlocked {
  filter: none;
  opacity: 1;
  border-color: color-mix(in srgb, #fbbf24 45%, var(--border, #262d3d));
  background: linear-gradient(160deg, color-mix(in srgb, #fbbf24 6%, var(--panel, #161b24)), var(--panel, #161b24));
}

.card-top {
  display: flex;
  align-items: center;
  gap: 12px;
}

.achv-icon {
  width: 44px;
  height: 44px;
  border-radius: 10px;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 22px;
  flex-shrink: 0;
}

.card-title {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}

.name-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.achv-name {
  font-size: 15px;
  font-weight: 700;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.tier-badge {
  font-size: 11px;
  font-weight: 700;
  color: #fbbf24;
  background: color-mix(in srgb, #fbbf24 14%, transparent);
  padding: 1px 8px;
  border-radius: 999px;
  flex-shrink: 0;
}

.cat-badge {
  font-size: 11px;
  font-weight: 600;
  padding: 1px 8px;
  border-radius: 6px;
  width: fit-content;
}

.achv-desc {
  font-size: 12.5px;
  color: var(--text-dim, #9aa4b8);
  line-height: 1.5;
  min-height: 34px;
}

.tier-row {
  display: flex;
  gap: 6px;
}

.tier-dot {
  width: 22px;
  height: 6px;
  border-radius: 999px;
  background: var(--panel-2, #1b2130);
  border: 1px solid var(--border, #262d3d);
  transition: all 0.3s;
}

.tier-dot.on {
  background: linear-gradient(90deg, var(--accent-color, #6366f1), #fbbf24);
  border-color: transparent;
}

.card-foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-top: auto;
}

.progress-text {
  font-size: 12px;
  font-weight: 600;
  color: var(--text-dim, #9aa4b8);
}

.unlock-time {
  font-size: 11px;
  color: #fbbf24;
  flex-shrink: 0;
}
</style>
