<script setup lang="ts">
import { ref, onMounted, onActivated, onUnmounted, computed, watch } from "vue";
import {
  NCard,
  NGrid,
  NGi,
  NStatistic,
  NIcon,
  NTabs,
  NTabPane,
  NSpin,
  NSelect,
  NEmpty,
  NSpace,
  NButton,
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
  LineChart,
  PieChart,
  HeatmapChart,
  BarChart,
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
  PlaySessionDetail,
} from "../lib/tauri";
import { useGamesStore } from "../stores/games";
import { formatPlayTime, formatDate } from "../lib/format";

const gamesStore = useGamesStore();

use([
  CanvasRenderer,
  LineChart,
  PieChart,
  HeatmapChart,
  BarChart,
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

// 游玩会话历史
const sessionHistory = ref<PlaySessionDetail[]>([]);
const sessionGameFilter = ref<string>("");
const sessionLoading = ref(false);
const sessionOffset = ref(0);
const sessionHasMore = ref(true);
const SESSION_PAGE_SIZE = 50;

// 游戏筛选选项（用于会话历史）
const gameFilterOptions = computed(() => [
  { label: "全部游戏", value: "" },
  ...gamesStore.games.map((g) => ({ label: g.name, value: g.id })),
]);

// 游戏主色调缓存 (game_id -> hex color)
const gameColors = ref<Record<string, string>>({});

// 当前活跃的 tab
const activeTab = ref("overview");

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
        radius: ["40%", "65%"],
        center: ["30%", "50%"],
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

// 游戏类型 - 按游戏数量饼图
const genreCountPieOption = computed(() => {
  const sorted = [...genreStats.value].sort((a, b) => b.game_count - a.game_count);
  const top = sorted.slice(0, 12);
  const otherCount = sorted.slice(12).reduce((sum, g) => sum + g.game_count, 0);
  const data = top.map((g) => ({
    name: g.genre,
    value: g.game_count,
  }));
  if (otherCount > 0) {
    data.push({ name: "其他", value: otherCount });
  }

  const colors = [
    "#6366f1", "#8b5cf6", "#ec4899", "#f43f5e", "#f97316",
    "#eab308", "#22c55e", "#14b8a6", "#06b6d4", "#3b82f6",
    "#a78bfa", "#fb923c", "#6b7280",
  ];

  return {
    tooltip: {
      trigger: "item",
      formatter: (params: any) => {
        return `${params.name}<br/>游戏数: ${params.value} 款<br/>占比: ${params.percent}%`;
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
        radius: ["35%", "65%"],
        center: ["32%", "50%"],
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
            fontSize: 15,
            fontWeight: "bold",
            color: "#fff",
          },
        },
        labelLine: { show: false },
        data: data.map((d, i) => ({
          ...d,
          itemStyle: { color: colors[i % colors.length] },
        })),
      },
    ],
  };
});

// 游戏类型 - 按游戏数量条形图
const genreBarOption = computed(() => {
  const sorted = [...genreStats.value].sort((a, b) => a.game_count - b.game_count);
  const top = sorted.slice(-15); // top 15 by game count

  const colors = [
    "#3b82f6", "#06b6d4", "#14b8a6", "#22c55e", "#eab308",
    "#f97316", "#f43f5e", "#ec4899", "#8b5cf6", "#6366f1",
    "#a78bfa", "#c084fc", "#e879f9", "#f472b6", "#fb923c",
  ];

  return {
    tooltip: {
      trigger: "axis",
      axisPointer: { type: "shadow" },
      formatter: (params: any) => {
        const d = params[0];
        const genre = top[d.dataIndex];
        return `${genre.genre}<br/>游戏数: ${genre.game_count} 款`;
      },
    },
    grid: { left: "3%", right: "8%", bottom: "3%", top: 20, containLabel: true },
    xAxis: {
      type: "value",
      name: "款",
      axisLabel: { color: "#aaa" },
      splitLine: { lineStyle: { color: "rgba(255,255,255,0.05)" } },
    },
    yAxis: {
      type: "category",
      data: top.map((g) => g.genre),
      axisLabel: {
        color: "#aaa",
        fontSize: 12,
        width: 80,
        overflow: "truncate",
      },
    },
    series: [
      {
        type: "bar",
        data: top.map((g, i) => ({
          value: g.game_count,
          itemStyle: {
            color: colors[i % colors.length],
            borderRadius: [0, 4, 4, 0],
          },
        })),
        barWidth: "60%",
        label: {
          show: true,
          position: "right",
          formatter: "{c} 款",
          color: "#aaa",
          fontSize: 11,
        },
      },
    ],
  };
});

// 游戏时长排行数据（前 10，正序排列用于展示）
const top10 = computed(() => {
  return playStats.value.slice(0, 10).map((s) => ({
    ...s,
    hours: Number((s.total_seconds / 3600).toFixed(1)),
  }));
});

// bar 宽度百分比（相对最大值）
function barWidth(hours: number): string {
  const maxHours = top10.value.length > 0 ? top10.value[0].hours : 1;
  return Math.max((hours / maxHours) * 100, 2) + "%";
}

// 获取游戏主色调（带 fallback）
function getGameColor(gameId: string): string {
  return gameColors.value[gameId] || "#6366f1";
}

// 生成 bar 渐变样式
function getBarGradient(gameId: string): string {
  const base = getGameColor(gameId);
  const lighter = lightenColor(base, 30);
  return `linear-gradient(90deg, ${base}, ${lighter})`;
}

// 颜色工具：提亮
function lightenColor(hex: string, percent: number): string {
  const num = parseInt(hex.replace("#", ""), 16);
  const r = Math.min(255, ((num >> 16) & 0xff) + Math.round(255 * percent / 100));
  const g = Math.min(255, ((num >> 8) & 0xff) + Math.round(255 * percent / 100));
  const b = Math.min(255, (num & 0xff) + Math.round(255 * percent / 100));
  return `#${((r << 16) | (g << 8) | b).toString(16).padStart(6, "0")}`;
}

// 从封面图片提取主色调
function extractDominantColor(imgSrc: string): Promise<string> {
  return new Promise((resolve) => {
    const img = new Image();
    img.crossOrigin = "anonymous";
    img.onload = () => {
      const canvas = document.createElement("canvas");
      const size = 32; // 缩小取样，提高性能
      canvas.width = size;
      canvas.height = size;
      const ctx = canvas.getContext("2d");
      if (!ctx) { resolve("#6366f1"); return; }
      ctx.drawImage(img, 0, 0, size, size);
      const data = ctx.getImageData(0, 0, size, size).data;

      // 统计颜色频率（量化为 32 级）
      const colorMap = new Map<string, { r: number; g: number; b: number; count: number }>();
      for (let i = 0; i < data.length; i += 4) {
        const r = data[i], g = data[i + 1], b = data[i + 2];
        // 跳过过暗和过亮的像素
        const brightness = (r + g + b) / 3;
        if (brightness < 30 || brightness > 230) continue;
        // 量化
        const qr = (r >> 5) << 5;
        const qg = (g >> 5) << 5;
        const qb = (b >> 5) << 5;
        const key = `${qr},${qg},${qb}`;
        const existing = colorMap.get(key);
        if (existing) {
          existing.count++;
        } else {
          colorMap.set(key, { r: qr, g: qg, b: qb, count: 1 });
        }
      }

      // 找最高频颜色
      let best = { r: 99, g: 102, b: 241, count: 0 }; // fallback: #6366f1
      for (const val of colorMap.values()) {
        if (val.count > best.count) best = val;
      }

      const hex = `#${((best.r << 16) | (best.g << 8) | best.b).toString(16).padStart(6, "0")}`;
      resolve(hex);
    };
    img.onerror = () => resolve("#6366f1");
    img.src = imgSrc;
  });
}

// 加载封面并提取颜色
async function loadGameColors(stats: PlayStats[]) {
  const coverCache = gamesStore.coverBase64Cache;
  const tasks: Promise<void>[] = [];

  for (const s of stats) {
    if (gameColors.value[s.game_id]) continue; // 已有缓存
    const cover = coverCache[s.game_id];
    if (cover) {
      tasks.push(
        extractDominantColor(cover).then((color) => {
          gameColors.value[s.game_id] = color;
        })
      );
    }
  }

  await Promise.all(tasks);
}

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

// 加载游玩会话历史
async function loadSessionHistory(reset = false) {
  if (reset) {
    sessionOffset.value = 0;
    sessionHistory.value = [];
    sessionHasMore.value = true;
  }

  if (!sessionHasMore.value) return;

  sessionLoading.value = true;
  try {
    const gameId = sessionGameFilter.value || undefined;
    const sessions = await api.getPlaySessions(gameId, SESSION_PAGE_SIZE, sessionOffset.value);
    if (sessions.length < SESSION_PAGE_SIZE) {
      sessionHasMore.value = false;
    }
    sessionHistory.value.push(...sessions);
    sessionOffset.value += sessions.length;
  } catch (e) {
    console.error("加载会话历史失败:", e);
  } finally {
    sessionLoading.value = false;
  }
}

// 筛选游戏变化时重新加载
watch(sessionGameFilter, () => {
  loadSessionHistory(true);
});

// 切换到游玩记录 tab 时加载数据
watch(activeTab, (tab) => {
  if (tab === "sessions" && sessionHistory.value.length === 0) {
    loadSessionHistory(true);
  }
});

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
    // 加载封面颜色（封面可能已缓存在 store 中）
    loadGameColors(ps);
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

// 当 store 中的封面缓存更新时，补充提取颜色
watch(() => gamesStore.coverBase64Cache, () => {
  if (playStats.value.length > 0) {
    loadGameColors(playStats.value);
  }
}, { deep: true });

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
      <n-tabs v-model:value="activeTab" type="line">
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
                <div class="custom-bar-chart">
                  <div
                    v-for="(game, index) in top10"
                    :key="game.game_id"
                    class="bar-row"
                  >
                    <div class="bar-rank">{{ index + 1 }}</div>
                    <div class="bar-icon">
                      <img
                        v-if="gamesStore.coverBase64Cache[game.game_id]"
                        :src="gamesStore.coverBase64Cache[game.game_id]"
                        :alt="game.game_name"
                        class="bar-icon-img"
                      />
                      <div v-else class="bar-icon-fallback">🎮</div>
                    </div>
                    <div class="bar-info">
                      <div class="bar-name" :title="game.game_name">{{ game.game_name }}</div>
                      <div class="bar-track">
                        <div
                          class="bar-fill"
                          :style="{
                            width: barWidth(game.hours),
                            background: getBarGradient(game.game_id),
                          }"
                        >
                          <span class="bar-label">{{ game.hours }}h</span>
                        </div>
                      </div>
                    </div>
                  </div>
                </div>
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

        <!-- 游戏类型 Tab -->
        <n-tab-pane name="genres" tab="游戏类型">
          <n-grid :cols="chartGridCols" :x-gap="16" :y-gap="16" responsive="screen">
            <n-gi>
              <n-card title="类型分布（按游戏数·饼图）">
                <v-chart :option="genreCountPieOption" style="height: 400px" autoresize />
              </n-card>
            </n-gi>
            <n-gi>
              <n-card title="类型分布（按游戏数·条形图）">
                <v-chart :option="genreBarOption" style="height: 400px" autoresize />
              </n-card>
            </n-gi>
          </n-grid>
          <!-- 类型详细列表 -->
          <n-card title="类型详情" style="margin-top: 16px">
            <div class="genre-detail-grid">
              <div
                v-for="(genre, index) in genreStats"
                :key="genre.genre"
                class="genre-detail-item"
              >
                <div class="genre-detail-header">
                  <span
                    class="genre-dot"
                    :style="{ background: [
                      '#6366f1', '#8b5cf6', '#ec4899', '#f43f5e', '#f97316',
                      '#eab308', '#22c55e', '#14b8a6', '#06b6d4', '#3b82f6',
                      '#a78bfa', '#fb923c',
                    ][index % 12] }"
                  />
                  <span class="genre-detail-name">{{ genre.genre }}</span>
                </div>
                <div class="genre-detail-stats">
                  <span>{{ genre.game_count }} 款</span>
                </div>
              </div>
            </div>
          </n-card>
        </n-tab-pane>

        <!-- 游玩记录 Tab -->
        <n-tab-pane name="sessions" tab="游玩记录">
          <n-card>
            <template #header>
              <n-space align="center" justify="space-between">
                <span>游玩会话历史</span>
                <n-select
                  v-model:value="sessionGameFilter"
                  :options="gameFilterOptions"
                  placeholder="筛选游戏"
                  clearable
                  style="width: 200px"
                  size="small"
                />
              </n-space>
            </template>

            <div v-if="sessionHistory.length === 0 && !sessionLoading" style="padding: 40px 0">
              <n-empty description="暂无游玩记录" />
            </div>

            <div v-else class="session-list">
              <div
                v-for="session in sessionHistory"
                :key="session.id"
                class="session-item"
              >
                <div class="session-game-name">{{ session.game_name }}</div>
                <div class="session-details">
                  <span class="session-time">{{ formatDate(session.start_time) }}</span>
                  <span class="session-duration">{{ formatPlayTime(session.duration_seconds) }}</span>
                </div>
              </div>
            </div>

            <div v-if="sessionHasMore && sessionHistory.length > 0" style="text-align: center; margin-top: 16px">
              <n-button
                :loading="sessionLoading"
                @click="loadSessionHistory(false)"
                size="small"
              >
                加载更多
              </n-button>
            </div>

            <div v-if="sessionLoading && sessionHistory.length === 0" style="text-align: center; padding: 40px 0">
              <n-spin size="medium" />
            </div>
          </n-card>
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

/* 自定义条形图 */
.custom-bar-chart {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 4px 0;
}

.bar-row {
  display: flex;
  align-items: center;
  gap: 10px;
  height: 40px;
}

.bar-rank {
  width: 20px;
  text-align: center;
  font-size: 12px;
  font-weight: 600;
  color: #666;
  flex-shrink: 0;
}

.bar-icon {
  width: 36px;
  height: 36px;
  border-radius: 6px;
  overflow: hidden;
  flex-shrink: 0;
  background: rgba(255, 255, 255, 0.05);
}

.bar-icon-img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.bar-icon-fallback {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 18px;
}

.bar-info {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.bar-name {
  font-size: 12px;
  color: #ccc;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  line-height: 1.2;
}

.bar-track {
  width: 100%;
  height: 18px;
  background: rgba(255, 255, 255, 0.04);
  border-radius: 4px;
  overflow: hidden;
}

.bar-fill {
  height: 100%;
  border-radius: 4px;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  padding-right: 8px;
  min-width: 40px;
  transition: width 0.6s cubic-bezier(0.22, 1, 0.36, 1);
}

.bar-label {
  font-size: 11px;
  font-weight: 500;
  color: rgba(255, 255, 255, 0.9);
  white-space: nowrap;
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.3);
}

/* 游玩会话列表 */
.session-list {
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.session-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 12px;
  background: rgba(255, 255, 255, 0.02);
  border-radius: 4px;
  transition: background 0.2s;
}

.session-item:hover {
  background: rgba(255, 255, 255, 0.05);
}

.session-game-name {
  font-size: 13px;
  font-weight: 500;
  color: #ddd;
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.session-details {
  display: flex;
  gap: 16px;
  align-items: center;
  flex-shrink: 0;
}

.session-time {
  font-size: 12px;
  color: #888;
}

.session-duration {
  font-size: 12px;
  font-weight: 500;
  color: #aaa;
  min-width: 60px;
  text-align: right;
}

/* 类型详情网格 */
.genre-detail-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
  gap: 10px;
}

.genre-detail-item {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 10px 12px;
  background: rgba(255, 255, 255, 0.03);
  border-radius: 8px;
  border: 1px solid rgba(255, 255, 255, 0.06);
  transition: background 0.2s;
}

.genre-detail-item:hover {
  background: rgba(255, 255, 255, 0.06);
}

.genre-detail-header {
  display: flex;
  align-items: center;
  gap: 6px;
}

.genre-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}

.genre-detail-name {
  font-size: 13px;
  color: #ddd;
  font-weight: 500;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.genre-detail-stats {
  font-size: 12px;
  color: #888;
  padding-left: 14px;
}
</style>
