<script setup lang="ts">
import { ref, onMounted, onActivated, computed } from "vue";
import {
  NCard,
  NGrid,
  NGi,
  NStatistic,
  NIcon,
  NTabs,
  NTabPane,
  NSpin,
} from "naive-ui";
import {
  GameControllerOutline,
  TimeOutline,
  TrendingUpOutline,
  CalendarOutline,
} from "@vicons/ionicons5";
import { use } from "echarts/core";
import { CanvasRenderer } from "echarts/renderers";
import { BarChart, LineChart } from "echarts/charts";
import {
  TitleComponent,
  TooltipComponent,
  GridComponent,
} from "echarts/components";
import VChart from "vue-echarts";
import * as api from "../lib/tauri";
import type { PlayStats, DailyStats } from "../lib/tauri";

use([
  CanvasRenderer,
  BarChart,
  LineChart,
  TitleComponent,
  TooltipComponent,
  GridComponent,
]);

const loading = ref(true);
const overview = ref<{
  game_count: number;
  total_play_time: number;
  monthly_play_time: number;
  today_play_time: number;
}>({
  game_count: 0,
  total_play_time: 0,
  monthly_play_time: 0,
  today_play_time: 0,
});
const playStats = ref<PlayStats[]>([]);
const dailyStats = ref<DailyStats[]>([]);

// 游戏时长排行图
const barOption = computed(() => ({
  tooltip: { trigger: "axis" },
  grid: { left: "3%", right: "4%", bottom: "3%", containLabel: true },
  xAxis: {
    type: "category",
    data: playStats.value.slice(0, 10).map((s) =>
      s.game_name.length > 8 ? s.game_name.slice(0, 8) + "..." : s.game_name
    ),
    axisLabel: { color: "#aaa", rotate: 30 },
  },
  yAxis: {
    type: "value",
    name: "小时",
    axisLabel: { color: "#aaa" },
  },
  series: [
    {
      type: "bar",
      data: playStats.value
        .slice(0, 10)
        .map((s) => (s.total_seconds / 3600).toFixed(1)),
      itemStyle: {
        color: "#6366f1",
        borderRadius: [4, 4, 0, 0],
      },
    },
  ],
}));

// 每日游玩时长趋势图
const lineOption = computed(() => ({
  tooltip: { trigger: "axis" },
  grid: { left: "3%", right: "4%", bottom: "3%", containLabel: true },
  xAxis: {
    type: "category",
    data: dailyStats.value
      .slice()
      .reverse()
      .map((s) => s.date.slice(5)),
    axisLabel: { color: "#aaa" },
  },
  yAxis: {
    type: "value",
    name: "小时",
    axisLabel: { color: "#aaa" },
  },
  series: [
    {
      type: "line",
      data: dailyStats.value
        .slice()
        .reverse()
        .map((s) => (s.total_seconds / 3600).toFixed(1)),
      smooth: true,
      areaStyle: {
        color: {
          type: "linear",
          x: 0,
          y: 0,
          x2: 0,
          y2: 1,
          colorStops: [
            { offset: 0, color: "rgba(99,102,241,0.3)" },
            { offset: 1, color: "rgba(99,102,241,0.05)" },
          ],
        },
      },
      lineStyle: { color: "#6366f1" },
      itemStyle: { color: "#6366f1" },
    },
  ],
}));

function formatHours(seconds: number): string {
  return (seconds / 3600).toFixed(1);
}

async function loadStats() {
  loading.value = true;
  try {
    const [ov, ps, ds] = await Promise.all([
      api.getOverviewStats(),
      api.getPlayStats(10),
      api.getDailyStats(30),
    ]);
    overview.value = ov;
    playStats.value = ps;
    dailyStats.value = ds;
  } catch (e) {
    console.error("加载统计数据失败:", e);
  } finally {
    loading.value = false;
  }
}

onMounted(loadStats);

// keep-alive 缓存的组件再次激活时刷新数据
onActivated(loadStats);
</script>

<template>
  <div class="stats-view">
    <n-spin :show="loading">
      <!-- 概览卡片 -->
      <n-grid :cols="4" :x-gap="16" :y-gap="16" style="margin-bottom: 24px">
        <n-gi>
          <n-card size="small">
            <n-statistic label="游戏总数">
              <template #prefix>
                <n-icon :component="GameControllerOutline" />
              </template>
              {{ overview.game_count }}
            </n-statistic>
          </n-card>
        </n-gi>
        <n-gi>
          <n-card size="small">
            <n-statistic label="总游玩时长">
              <template #prefix>
                <n-icon :component="TimeOutline" />
              </template>
              {{ formatHours(overview.total_play_time) }}h
            </n-statistic>
          </n-card>
        </n-gi>
        <n-gi>
          <n-card size="small">
            <n-statistic label="本月时长">
              <template #prefix>
                <n-icon :component="TrendingUpOutline" />
              </template>
              {{ formatHours(overview.monthly_play_time) }}h
            </n-statistic>
          </n-card>
        </n-gi>
        <n-gi>
          <n-card size="small">
            <n-statistic label="今日时长">
              <template #prefix>
                <n-icon :component="CalendarOutline" />
              </template>
              {{ formatHours(overview.today_play_time) }}h
            </n-statistic>
          </n-card>
        </n-gi>
      </n-grid>

      <!-- 图表 -->
      <n-tabs type="line">
        <n-tab-pane name="ranking" tab="时长排行">
          <n-card>
            <v-chart :option="barOption" style="height: 400px" autoresize />
          </n-card>
        </n-tab-pane>
        <n-tab-pane name="trend" tab="每日趋势">
          <n-card>
            <v-chart :option="lineOption" style="height: 400px" autoresize />
          </n-card>
        </n-tab-pane>
      </n-tabs>
    </n-spin>
  </div>
</template>

<style scoped>
.stats-view {
  max-width: 1200px;
}
</style>
