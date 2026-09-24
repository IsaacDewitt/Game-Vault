<script setup lang="ts">
import { ref, computed, onActivated } from "vue";
import { NSpin, NEmpty, NSelect, useMessage } from "naive-ui";
import * as api from "../lib/tauri";
import type { AchievementSummary, GlobalAchievementStatus, GameAchievementStatus } from "../lib/tauri";
import { formatPlayTime, formatDate } from "../lib/format";

const message = useMessage();
const summary = ref<AchievementSummary | null>(null);
const loading = ref(true);
const selectedGameId = ref<string | null>(null);
const activeCategory = ref<string | null>(null);

const CATEGORY_META: Record<string, { label: string; color: string }> = {
  progress: { label: "进度", color: "#4da3ff" },
  collect: { label: "收集", color: "#a78bfa" },
  fun: { label: "趣味", color: "#fbbf24" },
  challenge: { label: "挑战", color: "#f87171" },
};

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

// 全局成就按基础 ID 分组（多级成就合并展示）
interface AchievementGroup {
  name: string;
  icon: string;
  desc: string;
  category: string;
  items: GlobalAchievementStatus[];
  unlockedCount: number;
  lastUnlockedAt: string | null;
}

const globalGroups = computed<AchievementGroup[]>(() => {
  if (!summary.value) return [];
  const map = new Map<string, GlobalAchievementStatus[]>();
  for (const item of summary.value.global) {
    const arr = map.get(item.def.base_id) ?? [];
    arr.push(item);
    map.set(item.def.base_id, arr);
  }
  const groups: AchievementGroup[] = [...map.values()]
    .filter((items) => !activeCategory.value || items[0].def.category === activeCategory.value)
    .map((items) => {
      const unlockedItems = items.filter((i) => i.unlocked);
      return {
        name: items[0].def.name,
        icon: items[0].def.icon,
        desc: items[0].def.desc,
        category: items[0].def.category,
        items,
        unlockedCount: unlockedItems.length,
        lastUnlockedAt: unlockedItems.length
          ? unlockedItems[unlockedItems.length - 1].unlocked_at
          : null,
      };
    });
  // 排序：未解锁的排前面，解锁多的排前面
  return groups.sort((a, b) => {
    const aDone = a.unlockedCount === a.items.length ? 1 : 0;
    const bDone = b.unlockedCount === b.items.length ? 1 : 0;
    if (aDone !== bDone) return aDone - bDone;
    return b.unlockedCount - a.unlockedCount;
  });
});

// 单游戏成就分组
const gameGroups = computed<AchievementGroup[]>(() => {
  if (!summary.value) return [];
  const game = summary.value.per_game.find((g) => g.game_id === selectedGameId.value);
  if (!game) return [];
  const map = new Map<string, GameAchievementStatus[]>();
  for (const item of game.achievements) {
    const arr = map.get(item.def.base_id) ?? [];
    arr.push(item);
    map.set(item.def.base_id, arr);
  }
  return [...map.values()].map((items) => {
    const unlockedItems = items.filter((i) => i.unlocked);
    return {
      name: items[0].def.name,
      icon: items[0].def.icon,
      desc: items[0].def.desc,
      category: items[0].def.category,
      items,
      unlockedCount: unlockedItems.length,
      lastUnlockedAt: unlockedItems.length
        ? unlockedItems[unlockedItems.length - 1].unlocked_at
        : null,
    };
  });
});

// 概览统计
const stats = computed(() => {
  if (!summary.value) return { unlocked: 0, total: 0, percent: 0, gamesWithProgress: 0 };
  const { unlocked_count, total_count, per_game } = summary.value;
  const gamesWithProgress = per_game.filter((g) =>
    g.achievements.some((a) => a.unlocked)
  ).length;
  const percent = total_count > 0 ? Math.round((unlocked_count / total_count) * 100) : 0;
  return { unlocked: unlocked_count, total: total_count, percent, gamesWithProgress };
});

// 格式化进度文本
function formatProgress(item: { progress: number; target: number }): string {
  const { progress, target } = item;
  if (target <= 1) return progress >= 1 ? "已完成" : "未完成";
  if (target >= 3600) return `${formatPlayTime(progress)} / ${formatPlayTime(target)}`;
  if (target >= 60) {
    const pm = Math.floor(progress / 60);
    const tm = Math.floor(target / 60);
    return `${pm} 分钟 / ${tm} 分钟`;
  }
  return `${progress} / ${target}`;
}

function groupProgress(group: AchievementGroup): { progress: number; target: number } {
  // 进度取当前最高已解锁等级的下一个目标；未解锁任何等级则取第一级目标
  const unlockedTiers = group.items.filter((i) => i.unlocked).length;
  if (unlockedTiers === 0) {
    return { progress: group.items[0]?.progress ?? 0, target: group.items[0]?.target ?? 0 };
  }
  const lastUnlocked = group.items[unlockedTiers - 1];
  const next = group.items[unlockedTiers];
  if (next) return { progress: lastUnlocked.progress, target: next.target };
  return { progress: lastUnlocked.progress, target: lastUnlocked.target };
}

// 当前选中的是否为「已删除留档」条目
const selectedGameRemoved = computed(() =>
  (summary.value?.per_game ?? []).some(
    (g) => g.game_id === selectedGameId.value && g.removed
  )
);

// 游戏选择器选项
//
// 后端按 game_id（UUID）字典序返回，那是**无意义顺序**——42 款游戏的下拉里根本找不着人。
// 这里重排成人看得懂的顺序（2026-09-24 修「已删除条目在下拉里找不到」）：
//   ① 已获成就的排前面（最可能是想看的）；
//   ② 再按已解锁数量多的靠前；
//   ③ 同分按名字（用 localeCompare 让中文按拼音、英文按字母）；
//   ④ **已删除留档条目沉到最底**并带「已删除」后缀——它们不在库中，不该混在在库游戏里，
//      但要能在列表末尾找得到（成就记录按设计保留）。
const gameOptions = computed(() => {
  const list = [...(summary.value?.per_game ?? [])];
  list.sort((a, b) => {
    if (a.removed !== b.removed) return a.removed ? 1 : -1;
    const ua = a.achievements.filter((x) => x.unlocked).length;
    const ub = b.achievements.filter((x) => x.unlocked).length;
    if (ua !== ub) return ub - ua;
    return a.game_name.localeCompare(b.game_name, "zh-Hans-CN");
  });
  return list.map((g) => ({
    label: g.removed ? `${g.game_name}（已删除）` : g.game_name,
    value: g.game_id,
  }));
});

// 已删除留档条目在选项里的子集（给「沉底找不到」补一条显式入口）
const removedOptions = computed(() =>
  gameOptions.value.filter((o) => o.label.endsWith("（已删除）"))
);

// 分类徽章颜色
function categoryColor(cat: string): string {
  return CATEGORY_META[cat]?.color ?? "#888";
}
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
              <span class="side-num">{{ summary.per_game.length }}</span>
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
                  {{ formatProgress(groupProgress(group)) }}
                </span>
                <span v-if="group.lastUnlockedAt" class="unlock-time">
                  {{ formatDate(group.lastUnlockedAt) }}解锁
                </span>
              </div>
            </div>
          </div>
          <n-empty v-else description="该分类下暂无成就" style="padding: 40px 0" />
        </div>

        <!-- 单游戏成就 -->
        <div class="section">
          <div class="section-header">
            <h2>🎮 单游戏成就</h2>
            <n-select
              v-model:value="selectedGameId"
              :options="gameOptions"
              placeholder="选择一款游戏查看其成就（可输入搜索）"
              style="width: 280px"
              filterable
              clearable
            />
          </div>

          <!-- 已删除留档条目提示：它们沉在列表末尾，不点出来用户根本不知道有这几个 -->
          <div v-if="removedOptions.length" class="removed-hint">
            已删除的游戏（{{ removedOptions.length }} 款）的成就记录仍保留，列在列表末尾：
            <button
              v-for="o in removedOptions"
              :key="o.value"
              class="removed-link"
              @click="selectedGameId = o.value"
            >
              {{ o.label }}
            </button>
          </div>

          <template v-if="selectedGameId">
            <!-- 已删除条目的说明：解锁记录按设计留档，但它已不在库中 -->
            <div v-if="selectedGameRemoved" class="removed-note">
              该游戏已从库中删除，此处展示的是其历史解锁记录——成就记录不随删除消失。
            </div>
            <div v-if="gameGroups.length" class="achv-grid">
              <div
                v-for="group in gameGroups"
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
                <div v-if="group.items.length > 1" class="tier-row">
                  <span
                    v-for="item in group.items"
                    :key="item.def.id"
                    class="tier-dot"
                    :class="{ on: item.unlocked }"
                  ></span>
                </div>
                <div class="card-foot">
                  <span class="progress-text">
                    {{ formatProgress(groupProgress(group)) }}
                  </span>
                  <span v-if="group.lastUnlockedAt" class="unlock-time">
                    {{ formatDate(group.lastUnlockedAt) }}解锁
                  </span>
                </div>
              </div>
            </div>
            <n-empty v-else description="该游戏暂无成就数据" style="padding: 40px 0" />
          </template>
          <div v-else class="select-hint">
            从上方选择一款游戏，查看它独立结算的成就进度 —— 每款游戏的成就是单独计算的。
          </div>
        </div>
      </template>
    </n-spin>
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

/* 已删除留档条目的说明条 */
.removed-note {
  margin-bottom: 14px;
  padding: 10px 16px;
  border-radius: 10px;
  font-size: 13px;
  line-height: 1.6;
  color: var(--text-secondary, #9aa4b2);
  background: var(--panel, #161b24);
  border: 1px dashed var(--border, #262d3d);
}

/* 下拉前的已删除条目快捷入口（它们沉在列表末尾，这里给一条明路） */
.removed-hint {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 14px;
  padding: 8px 14px;
  border-radius: 10px;
  font-size: 13px;
  color: var(--text-secondary, #9aa4b2);
}

.removed-link {
  padding: 2px 10px;
  border-radius: 999px;
  font-size: 12px;
  font-family: inherit;
  cursor: pointer;
  color: var(--text-secondary, #9aa4b2);
  background: var(--panel, #161b24);
  border: 1px dashed var(--border, #262d3d);
  transition: color 0.15s, border-color 0.15s;
}

.removed-link:hover {
  color: var(--accent-color, #6366f1);
  border-color: var(--accent-color, #6366f1);
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

.select-hint {
  padding: 36px;
  text-align: center;
  color: var(--text-dim, #9aa4b8);
  font-size: 13px;
  border: 1px dashed var(--border, #262d3d);
  border-radius: 12px;
  background: var(--panel, #161b24);
}
</style>
