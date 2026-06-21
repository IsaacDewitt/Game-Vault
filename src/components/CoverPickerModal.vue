<script setup lang="ts">
import { ref, computed, watch } from "vue";
import {
  NModal,
  NButton,
  NButtonGroup,
  NIcon,
  NSpin,
  NEmpty,
  NTooltip,
  useMessage,
} from "naive-ui";
import {
  ImageOutline,
  PhonePortraitOutline,
  PhoneLandscapeOutline,
  GridOutline,
} from "@vicons/ionicons5";
import type { CoverOption } from "../lib/tauri";
import { useGamesStore } from "../stores/games";

const props = defineProps<{
  show: boolean;
  gameId: string;
  gameName: string;
}>();

const emit = defineEmits<{
  close: [];
  "cover-changed": [];
}>();

const store = useGamesStore();
const message = useMessage();

const loading = ref(false);
const selecting = ref(false);
const covers = ref<CoverOption[]>([]);
const filter = ref<"portrait" | "landscape" | "all">("portrait");
const displayCount = ref(20);

// 筛选后的封面列表
const filteredCovers = computed(() => {
  let result = covers.value;
  if (filter.value === "portrait") {
    result = result.filter((c) => c.height > c.width);
  } else if (filter.value === "landscape") {
    result = result.filter((c) => c.width > c.height);
  }
  return result;
});

// 当前展示的封面（分页）
const displayedCovers = computed(() => {
  return filteredCovers.value.slice(0, displayCount.value);
});

const hasMore = computed(() => {
  return displayCount.value < filteredCovers.value.length;
});

// 监听弹窗打开，获取封面列表
watch(
  () => props.show,
  async (val) => {
    if (val && props.gameId) {
      await loadCovers();
    } else {
      // 关闭时重置状态
      covers.value = [];
      displayCount.value = 20;
      filter.value = "portrait";
    }
  }
);

async function loadCovers() {
  loading.value = true;
  covers.value = [];
  try {
    covers.value = await store.fetchCoverOptions(props.gameId);
  } catch (e: any) {
    message.error(e?.toString() || "获取封面列表失败");
  } finally {
    loading.value = false;
  }
}

async function selectCover(cover: CoverOption) {
  selecting.value = true;
  try {
    await store.setCoverFromUrl(props.gameId, cover.url);
    message.success("封面已更换");
    emit("cover-changed");
    emit("close");
  } catch (e: any) {
    message.error(e?.toString() || "设置封面失败");
  } finally {
    selecting.value = false;
  }
}

function loadMore() {
  displayCount.value += 20;
}

function setFilter(f: "portrait" | "landscape" | "all") {
  filter.value = f;
  displayCount.value = 20;
}
</script>

<template>
  <n-modal
    :show="show"
    preset="card"
    title=""
    :style="{ width: '720px' }"
    :bordered="false"
    :closable="true"
    :mask-closable="true"
    @update:show="(val: boolean) => !val && emit('close')"
  >
    <template #header>
      <div class="picker-header">
        <n-icon :component="ImageOutline" :size="20" />
        <span>选择封面 - {{ gameName }}</span>
      </div>
    </template>

    <div class="picker-body">
      <!-- 筛选按钮 -->
      <div class="filter-bar">
        <n-button-group size="small">
          <n-button
            :type="filter === 'portrait' ? 'primary' : 'default'"
            @click="setFilter('portrait')"
          >
            <template #icon>
              <n-icon :component="PhonePortraitOutline" />
            </template>
            竖图
          </n-button>
          <n-button
            :type="filter === 'landscape' ? 'primary' : 'default'"
            @click="setFilter('landscape')"
          >
            <template #icon>
              <n-icon :component="PhoneLandscapeOutline" />
            </template>
            横图
          </n-button>
          <n-button
            :type="filter === 'all' ? 'primary' : 'default'"
            @click="setFilter('all')"
          >
            <template #icon>
              <n-icon :component="GridOutline" />
            </template>
            全部
          </n-button>
        </n-button-group>
        <span class="filter-count">
          共 {{ filteredCovers.length }} 张
        </span>
      </div>

      <!-- 加载中 -->
      <div v-if="loading" class="loading-state">
        <n-spin size="large" />
        <p>正在从 SteamGridDB 获取封面列表...</p>
      </div>

      <!-- 无结果 -->
      <n-empty
        v-else-if="!loading && filteredCovers.length === 0"
        description="未找到可用的封面图片"
        style="padding: 60px 0"
      />

      <!-- 封面网格 -->
      <div v-else class="cover-grid">
        <div
          v-for="(cover, index) in displayedCovers"
          :key="index"
          class="cover-item"
          :class="{ selecting: selecting }"
          @click="!selecting && selectCover(cover)"
        >
          <n-tooltip trigger="hover" :delay="500">
            <template #trigger>
              <div class="cover-thumb-wrapper">
                <img
                  :src="cover.thumb_url"
                  :alt="`${cover.width}×${cover.height}`"
                  class="cover-thumb"
                  loading="lazy"
                />
                <div class="cover-overlay">
                  <n-icon :component="ImageOutline" :size="24" />
                </div>
              </div>
            </template>
            {{ cover.width }}×{{ cover.height }} · {{ cover.style }}
          </n-tooltip>
          <div class="cover-info">
            {{ cover.width }}×{{ cover.height }}
          </div>
        </div>
      </div>

      <!-- 加载更多 -->
      <div v-if="hasMore && !loading" class="load-more">
        <n-button @click="loadMore" quaternary>
          加载更多 ({{ filteredCovers.length - displayCount }} 张剩余)
        </n-button>
      </div>
    </div>
  </n-modal>
</template>

<style scoped>
.picker-header {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 16px;
  font-weight: 600;
}

.picker-body {
  min-height: 300px;
}

.filter-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 16px;
}

.filter-count {
  font-size: 13px;
  color: rgba(255, 255, 255, 0.5);
}

.loading-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 80px 0;
  gap: 16px;
}

.loading-state p {
  color: rgba(255, 255, 255, 0.6);
  font-size: 14px;
  margin: 0;
}

.cover-grid {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: 12px;
}

.cover-item {
  cursor: pointer;
  border-radius: 8px;
  overflow: hidden;
  background: rgba(255, 255, 255, 0.05);
  transition: transform 0.2s, box-shadow 0.2s;
}

.cover-item:hover {
  transform: translateY(-2px);
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
}

.cover-item.selecting {
  pointer-events: none;
  opacity: 0.6;
}

.cover-thumb-wrapper {
  position: relative;
  width: 100%;
  aspect-ratio: 3 / 4;
  overflow: hidden;
}

.cover-thumb {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}

.cover-overlay {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.5);
  opacity: 0;
  transition: opacity 0.2s;
}

.cover-item:hover .cover-overlay {
  opacity: 1;
}

.cover-overlay .n-icon {
  color: white;
}

.cover-info {
  padding: 4px 8px;
  font-size: 11px;
  color: rgba(255, 255, 255, 0.5);
  text-align: center;
}

.load-more {
  display: flex;
  justify-content: center;
  padding: 16px 0;
}
</style>
