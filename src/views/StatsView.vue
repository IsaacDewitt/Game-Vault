<script setup lang="ts">
import { ref, onMounted, onActivated, onUnmounted, computed } from "vue";
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
import {
  BarChart,
  LineChart,
  PieChart,
  HeatmapChart,
} from "echarts/charts";
import {
  TitleComponent,
  TooltipComponent,
  GridComponent,
  LegendComponent,
  VisualMapComponent,
  CalendarComponent,
} from "echarts/components";
import VChart from "vue-echarts";
import * as api from "../lib/tauri";
import type {
  PlayStats,
  DailyStats,
  GenreStats,
  HeatmapDay,
  HourlyStats,
  StatusStats,
} from "../lib/tauri";

use([
  CanvasRenderer,
  BarChart,
  LineChart,
  PieChart,
  HeatmapChart,
  TitleComponent,
  TooltipComponent,
  GridComponent,
  LegendComponent,
  VisualMapComponent,
  CalendarComponent,
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
const genreStats = ref<GenreStats[]>([]);
const heatmapStats = ref<HeatmapDay[]>([]);
const hourlyStats = ref<HourlyStats[]>([]);
const statusStats = ref<StatusStats>({
  unplayed: 0,
  playing: 0,
  completed: 0,
  abandoned: 0,
});

// 响应式网格列数
const gridCols = ref(4);
const chartGridCols = ref(2);

// 监听窗口大小变化
function updateGridCols() {
  const width = window.innerWidth;
  if (width < 900) {
    gridCols.value = 2;
    chartGridCols.value = 1;
  } else if (width < 1200) {
    gridCols.value = 4;
    chartGridCols.value = 1;
  } else {
    gridCols.value = 4;
    chartGridCols.value = 2;
  }
}

// 概览卡片
const overviewCards = computed(() => [
  {
    label: "游戏总数",
    value: overview.value.game_count,
    icon: GameControllerOutline,
    suffix: "款",
  },
  {
    label: "总游玩时长",
    value: formatHours(overview.value.total_play_time),
    icon: TimeOutline,
    suffix: "h",
  },
  {
    label: "本月时长",
    value: formatHours(overview.value.monthly_play_time),
    icon: TrendingUpOutline,
    suffix: "h",
  },
  {
    label: "今日时长",
    value: formatHours(overview.value.today_play_time),
    icon: CalendarOutline,
    suffix: "h",
  },
]);

// 游戏状态分布饼图
const statusPieOption = computed(() => {
  const data = [
    { name: "未游玩", value: statusStats.value.unplayed, itemStyle: { color: "#8b5cf6" } },
    { name: "游玩中", value: statusStats.value.playing, itemStyle: { color: "#3b82f6" } },
    { name: "已通关", value: statusStats.value.completed, itemStyle: { color: "#22c55e" } },
    { name: "已弃坑", value: statusStats.value.abandoned, itemStyle: { color: "#6b7280" } },
  ].filter((item) => item.value > 0);

  return {
    tooltip: {
      trigger: "item",
      formatter: "{b}: {c} 款 ({d}%)",
    },
    legend: {
      orient: "vertical",
      right: "2%",
      top: "center",
      textStyle: { color: "#aaa", fontSize: 12 },
      itemWidth: 12,
      itemHeight: 12,
      itemGap: 16,
    },
    series: [
      {
        type: "pie",
        radius: ["40%", "65%"],
        center: ["35%", "50%"],
        avoidLabelOverlap: false,
        itemStyle: {
          borderRadius: 6,
          borderColor: "#1a1a2e",
          borderWidth: 2,
        },
        label: {
          show: false,
          position: "center",
        },
        emphasis: {
          label: {
            show: true,
            fontSize: 16,
            fontWeight: "bold",
            color: "#fff",
          },
        },
        labelLine: {
          show: false,
        },
        data,
      },
    ],
  };
});

// 游戏类型分布环形图
const genrePieOption = computed(() => {
  const topGenres = genreStats.value.slice(0, 10);
  const colors = [
    "#6366f1", "#8b5cf6", "#ec4899", "#f43f5e", "#f97316",
    "#eab308", "#22c55e", "#14b8a6", "#06b6d4", "#3b82f6",
  ];

  return {
    tooltip: {
      trigger: "item",
      formatter: (params: any) => {
        const hours = (params.data.value / 3600).toFixed(1);
        return `${params.name}<br/>时长: ${hours}h<br/>游戏数: ${params.data.gameCount} 款`;
      },
    },
    legend: {
      type: "scroll",
      orient: "vertical",
      right: "2%",
      top: "center",
      bottom: 20,
      textStyle: { color: "#aaa", fontSize: 11 },
      itemWidth: 12,
      itemHeight: 12,
      itemGap: 12,
      pageTextStyle: { color: "#aaa" },
      pageIconColor: "#aaa",
      pageIconInactiveColor: "#555",
    },
    series: [
      {
        type: "pie",
        radius: ["30%", "60%"],
        center: ["30%", "50%"],
        roseType: "area",
        itemStyle: {
          borderRadius: 6,
          borderColor: "#1a1a2e",
          borderWidth: 2,
        },
        label: {
          show: false,
        },
        emphasis: {
          label: {
            show: true,
            fontSize: 14,
            fontWeight: "bold",
            color: "#fff",
          },
        },
        data: topGenres.map((g, i) => ({
          name: g.genre,
          value: g.total_seconds,
          gameCount: g.game_count,
          itemStyle: { color: colors[i % colors.length] },
        })),
      },
    ],
  };
});

// 游戏时长排行图（水平柱状图）
const barOption = computed(() => {
  const top10 = playStats.value.slice(0, 10).reverse();

  // 智能截断游戏名称：保留前面的有意义部分
  const truncateName = (name: string, maxLen: number) => {
    if (name.length <= maxLen) return name;
    // 尝试在空格处截断
    const truncated = name.slice(0, maxLen);
    const lastSpace = truncated.lastIndexOf(" ");
    if (lastSpace > maxLen * 0.6) {
      return truncated.slice(0, lastSpace) + "...";
    }
    return truncated + "...";
  };

  return {
    tooltip: {
      trigger: "axis",
      axisPointer: { type: "shadow" },
      formatter: (params: any) => {
        const data = params[0];
        const hours = Number(data.value).toFixed(1);
        return `${data.name}<br/>时长: ${hours}h`;
      },
    },
    grid: { left: "18%", right: "10%", bottom: "3%", top: "3%", containLabel: true },
    xAxis: {
      type: "value",
      name: "小时",
      axisLabel: { color: "#aaa" },
      splitLine: { lineStyle: { color: "rgba(255,255,255,0.05)" } },
    },
    yAxis: {
      type: "category",
      data: top10.map((s) => truncateName(s.game_name, 18)),
      axisLabel: {
        color: "#aaa",
        width: 140,
        overflow: "break",
        fontSize: 12,
      },
    },
    series: [
      {
        type: "bar",
        data: top10.map((s) => ({
          value: Number((s.total_seconds / 3600).toFixed(1)),
          name: s.game_name,
        })),
        itemStyle: {
          color: {
            type: "linear",
            x: 0, y: 0, x2: 1, y2: 0,
            colorStops: [
              { offset: 0, color: "#6366f1" },
              { offset: 1, color: "#8b5cf6" },
            ],
          },
          borderRadius: [0, 4, 4, 0],
        },
        emphasis: {
          itemStyle: {
            color: {
              type: "linear",
              x: 0, y: 0, x2: 1, y2: 0,
              colorStops: [
                { offset: 0, color: "#818cf8" },
                { offset: 1, color: "#a78bfa" },
              ],
            },
          },
        },
        label: {
          show: true,
          position: "right",
          formatter: (params: any) => params.value + "h",
          color: "#aaa",
          fontSize: 11,
        },
      },
    ],
  };
});

// 每日游玩时长趋势图
const lineOption = computed(() => ({
  tooltip: {
    trigger: "axis",
    formatter: (params: any) => {
      const data = params[0];
      const hours = Number(data.value).toFixed(1);
      return `${data.name}<br/>时长: ${hours}h`;
    },
  },
  grid: { left: "3%", right: "4%", bottom: "3%", containLabel: true },
  xAxis: {
    type: "category",
    data: dailyStats.value
      .slice()
      .reverse()
      .map((s) => s.date.slice(5)),
    axisLabel: { color: "#aaa" },
    boundaryGap: false,
  },
  yAxis: {
    type: "value",
    name: "小时",
    axisLabel: { color: "#aaa" },
    splitLine: { lineStyle: { color: "rgba(255,255,255,0.05)" } },
  },
  series: [
    {
      type: "line",
      data: dailyStats.value
        .slice()
        .reverse()
        .map((s) => Number((s.total_seconds / 3600).toFixed(1))),
      smooth: true,
      symbol: "circle",
      symbolSize: 6,
      areaStyle: {
        color: {
          type: "linear",
          x: 0, y: 0, x2: 0, y2: 1,
          colorStops: [
            { offset: 0, color: "rgba(99,102,241,0.3)" },
            { offset: 1, color: "rgba(99,102,241,0.05)" },
          ],
        },
      },
      lineStyle: { color: "#6366f1", width: 2 },
      itemStyle: { color: "#6366f1" },
    },
  ],
}));

// 色阶条最大值
const heatmapMaxHours = computed(() => {
  const maxS = Math.max(...heatmapStats.value.map((d) => d.total_seconds), 0);
  return Math.round(maxS / 3600);
});

const hourlyMaxHours = computed(() => {
  const dataMap = new Map<string, number>();
  hourlyStats.value.forEach((s) => {
    const key = `${s.weekday}-${s.hour}`;
    dataMap.set(key, s.total_seconds);
  });
  const maxS = Math.max(...dataMap.values(), 0);
  return Math.round(maxS / 3600);
});

// GitHub 风格热力图
const heatmapOption = computed(() => {
  const data = heatmapStats.value.map((d) => [d.date, d.total_seconds]);
  const maxSeconds = Math.max(...heatmapStats.value.map((d) => d.total_seconds), 1);

  return {
    tooltip: {
      formatter: (params: any) => {
        const date = params.data[0];
        const hours = (params.data[1] / 3600).toFixed(1);
        return `${date}<br/>时长: ${hours}h`;
      },
    },
    visualMap: {
      min: 0,
      max: maxSeconds,
      show: false,
      inRange: {
        color: ["#1a1a2e", "#2d1b69", "#4c1d95", "#6d28d9", "#8b5cf6"],
      },
    },
    calendar: {
      top: 50,
      left: 60,
      right: 40,
      bottom: 20,
      cellSize: ["auto", 13],
      range: getHeatmapRange(),
      itemStyle: {
        borderWidth: 2,
        borderColor: "#1a1a2e",
      },
      splitLine: { show: false },
      yearLabel: { show: false },
      monthLabel: { color: "#aaa", fontSize: 11 },
      dayLabel: {
        color: "#aaa",
        fontSize: 11,
        nameMap: ["日", "一", "二", "三", "四", "五", "六"],
      },
    },
    series: [
      {
        type: "heatmap",
        coordinateSystem: "calendar",
        data,
      },
    ],
  };
});

// 游玩时段热力图 (24小时 x 7天)
const hourlyHeatmapOption = computed(() => {
  const hours = Array.from({ length: 24 }, (_, i) => i);
  const weekdays = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];

  // 构建数据矩阵
  const dataMap = new Map<string, number>();
  hourlyStats.value.forEach((s) => {
    const key = `${s.weekday}-${s.hour}`;
    dataMap.set(key, s.total_seconds);
  });

  const data: [number, number, number][] = [];
  weekdays.forEach((_, dayIdx) => {
    hours.forEach((hour) => {
      const weekday = dayIdx + 1;
      const key = `${weekday}-${hour}`;
      const seconds = dataMap.get(key) || 0;
      data.push([hour, dayIdx, seconds]);
    });
  });

  const maxSeconds = Math.max(...data.map((d) => d[2]), 1);

  return {
    tooltip: {
      formatter: (params: any) => {
        const hour = params.data[0];
        const day = weekdays[params.data[1]];
        const hours = (params.data[2] / 3600).toFixed(1);
        return `${day} ${hour}:00<br/>时长: ${hours}h`;
      },
    },
    grid: {
      top: 35,
      bottom: 40,
      left: 70,
      right: 60,
    },
    xAxis: {
      type: "category",
      data: hours.map((h) => h + ":00"),
      splitArea: { show: true },
      axisLabel: {
        color: "#aaa",
        fontSize: 10,
        interval: 2,
        rotate: 0,
      },
    },
    yAxis: {
      type: "category",
      data: weekdays,
      splitArea: { show: true },
      axisLabel: { color: "#aaa", fontSize: 12 },
    },
    visualMap: {
      min: 0,
      max: maxSeconds,
      show: false,
      inRange: {
        color: ["#1a1a2e", "#2d1b69", "#4c1d95", "#6d28d9", "#8b5cf6"],
      },
    },
    series: [
      {
        type: "heatmap",
        data,
        label: {
          show: false,
        },
        emphasis: {
          itemStyle: {
            shadowBlur: 10,
            shadowColor: "rgba(0, 0, 0, 0.5)",
          },
        },
      },
    ],
  };
});

function formatHours(seconds: number): string {
  return (seconds / 3600).toFixed(1);
}

function getHeatmapRange(): string[] {
  const end = new Date();
  const start = new Date();
  start.setFullYear(start.getFullYear() - 1);
  return [start.toISOString().slice(0, 10), end.toISOString().slice(0, 10)];
}

async function loadStats() {
  loading.value = true;
  try {
    const [ov, ps, ds, gs, hs, hls, ss] = await Promise.all([
      api.getOverviewStats(),
      api.getPlayStats(20),
      api.getDailyStats(30),
      api.getGenreStats(),
      api.getHeatmapStats(365),
      api.getHourlyStats(),
      api.getStatusStats(),
    ]);
    overview.value = ov;
    playStats.value = ps;
    dailyStats.value = ds;
    genreStats.value = gs;
    heatmapStats.value = hs;
    hourlyStats.value = hls;
    statusStats.value = ss;
  } catch (e) {
    console.error("加载统计数据失败:", e);
  } finally {
    loading.value = false;
  }
}

onMounted(() => {
  loadStats();
  updateGridCols();
  window.addEventListener('resize', updateGridCols);
});

// keep-alive 缓存的组件再次激活时刷新数据
onActivated(() => {
  loadStats();
  updateGridCols();
});

// 组件卸载时清理事件监听器
onUnmounted(() => {
  window.removeEventListener('resize', updateGridCols);
});
</script>

<template>
  <div class="stats-view">
    <n-spin :show="loading">
      <!-- 概览卡片 -->
      <n-grid :cols="gridCols" :x-gap="16" :y-gap="16" responsive="screen" style="margin-bottom: 24px">
        <n-gi v-for="card in overviewCards" :key="card.label">
          <n-card size="small">
            <n-statistic :label="card.label">
              <template #prefix>
                <n-icon :component="card.icon" />
              </template>
              {{ card.value }}{{ card.suffix }}
            </n-statistic>
          </n-card>
        </n-gi>
      </n-grid>

      <!-- 图表 -->
      <n-tabs type="line">
        <!-- 概览 Tab -->
        <n-tab-pane name="overview" tab="概览">
          <n-grid :cols="chartGridCols" :x-gap="16" :y-gap="16" responsive="screen">
            <n-gi>
              <n-card title="游戏状态分布">
                <v-chart :option="statusPieOption" style="height: 350px" autoresize />
              </n-card>
            </n-gi>
            <n-gi>
              <n-card title="游戏类型分布">
                <v-chart :option="genrePieOption" style="height: 350px" autoresize />
              </n-card>
            </n-gi>
          </n-grid>
        </n-tab-pane>

        <!-- 时长 Tab -->
        <n-tab-pane name="duration" tab="时长分析">
          <n-grid :cols="1" :y-gap="16">
            <n-gi>
              <n-card title="时长排行">
                <v-chart :option="barOption" style="height: 400px" autoresize />
              </n-card>
            </n-gi>
            <n-gi>
              <n-card title="每日趋势">
                <v-chart :option="lineOption" style="height: 350px" autoresize />
              </n-card>
            </n-gi>
          </n-grid>
        </n-tab-pane>

        <!-- 热力图 Tab -->
        <n-tab-pane name="heatmap" tab="热力图">
          <n-grid :cols="1" :y-gap="16">
            <n-gi>
              <n-card title="年度游玩热力图">
                <div class="chart-wrapper">
                  <v-chart :option="heatmapOption" style="height: 250px" autoresize />
                  <div class="color-legend">
                    <span class="legend-label">0h</span>
                    <div class="legend-bar" />
                    <span class="legend-label">{{ heatmapMaxHours }}h</span>
                  </div>
                </div>
              </n-card>
            </n-gi>
            <n-gi>
              <n-card title="游玩时段分布">
                <div class="chart-wrapper">
                  <v-chart :option="hourlyHeatmapOption" style="height: 380px" autoresize />
                  <div class="color-legend">
                    <span class="legend-label">0h</span>
                    <div class="legend-bar" />
                    <span class="legend-label">{{ hourlyMaxHours }}h</span>
                  </div>
                </div>
              </n-card>
            </n-gi>
          </n-grid>
        </n-tab-pane>
      </n-tabs>
    </n-spin>
  </div>
</template>

<style scoped>
.stats-view {
  width: 100%;
}

.chart-wrapper {
  position: relative;
}

.color-legend {
  position: absolute;
  top: 0;
  right: 8px;
  display: flex;
  align-items: center;
  gap: 4px;
  z-index: 1;
}

.legend-bar {
  width: 80px;
  height: 8px;
  border-radius: 2px;
  background: linear-gradient(to right, #1a1a2e, #2d1b69, #4c1d95, #6d28d9, #8b5cf6);
}

.legend-label {
  font-size: 10px;
  color: #aaa;
  white-space: nowrap;
}
</style>
