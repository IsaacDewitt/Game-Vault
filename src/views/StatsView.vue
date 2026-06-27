<script setup lang="ts">
import { ref, inject, onMounted, onActivated, onUnmounted, computed, watch, nextTick, type Ref } from "vue";
import { lightenColor } from "../lib/color";
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
import { DEFAULT_ACCENT_COLOR, DEFAULT_ACCENT_RGB, COLOR_DARK_BG, COVER_SAMPLE_SIZE, COVER_BRIGHTNESS_MIN, COVER_BRIGHTNESS_MAX } from "../lib/constants";

const gamesStore = useGamesStore();

// 从 App.vue 注入响应式主题色，用于图表动态配色
const accentColor = inject<Ref<string>>("accentColor", ref(DEFAULT_ACCENT_COLOR));

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
const hourlyChartHeight = ref(280);

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

/** 根据容器宽度计算时段热力图高度，使单元格接近正方形 */
function updateHourlyHeight() {
  nextTick(() => {
    const wrapper = document.querySelector('.hourly-heatmap-wrapper');
    if (!wrapper) return;
    const containerWidth = wrapper.clientWidth;
    // 减去 y 轴标签宽度(48px)、grid 左右 padding(48+10px)、以及容器 padding(8px)
    const chartWidth = containerWidth - 48 - 48 - 10 - 8;
    // 24 列，23 个 gap(各 2px)
    const cellWidth = (chartWidth - 23 * 2) / 24;
    // 7 行，6 个 gap(各 2px)，加上 grid top(10) + bottom(50)
    const idealHeight = cellWidth * 7 + 6 * 2 + 10 + 50;
    hourlyChartHeight.value = Math.max(200, Math.min(500, Math.round(idealHeight)));
  });
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
          borderColor: COLOR_DARK_BG,
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

// 游戏类型分布环形图（仅显示有游玩时长的类型）
const genrePieOption = computed(() => {
  const playedGenres = genreStats.value.filter((g) => g.total_seconds > 0);
  const topGenres = playedGenres.slice(0, 10);
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
          borderColor: COLOR_DARK_BG,
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
          borderColor: COLOR_DARK_BG,
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
  return gameColors.value[gameId] || DEFAULT_ACCENT_COLOR;
}

// 生成 bar 渐变样式
function getBarGradient(gameId: string): string {
  const base = getGameColor(gameId);
  const lighter = lightenColor(base, 30);
  return `linear-gradient(90deg, ${base}, ${lighter})`;
}

// 从封面图片提取主色调
function extractDominantColor(imgSrc: string): Promise<string> {
  return new Promise((resolve) => {
    const img = new Image();
    img.crossOrigin = "anonymous";
    img.onload = () => {
      const canvas = document.createElement("canvas");
      canvas.width = COVER_SAMPLE_SIZE;
      canvas.height = COVER_SAMPLE_SIZE;
      const ctx = canvas.getContext("2d");
      if (!ctx) { resolve(DEFAULT_ACCENT_COLOR); return; }
      ctx.drawImage(img, 0, 0, COVER_SAMPLE_SIZE, COVER_SAMPLE_SIZE);
      const data = ctx.getImageData(0, 0, COVER_SAMPLE_SIZE, COVER_SAMPLE_SIZE).data;

      // 统计颜色频率（量化为 32 级）
      const colorMap = new Map<string, { r: number; g: number; b: number; count: number }>();
      for (let i = 0; i < data.length; i += 4) {
        const r = data[i], g = data[i + 1], b = data[i + 2];
        // 跳过过暗和过亮的像素
        const brightness = (r + g + b) / 3;
        if (brightness < COVER_BRIGHTNESS_MIN || brightness > COVER_BRIGHTNESS_MAX) continue;
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
      let best = { ...DEFAULT_ACCENT_RGB, count: 0 };
      for (const val of colorMap.values()) {
        if (val.count > best.count) best = val;
      }

      const hex = `#${((best.r << 16) | (best.g << 8) | best.b).toString(16).padStart(6, "0")}`;
      resolve(hex);
    };
    img.onerror = () => resolve(DEFAULT_ACCENT_COLOR);
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
            { offset: 0, color: accentColor.value + "4d" },
            { offset: 1, color: accentColor.value + "0d" },
          ],
        },
      },
      lineStyle: { color: accentColor.value, width: 2 },
      itemStyle: { color: accentColor.value },
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

// GitHub 风格热力图 — 自定义 HTML/CSS 实现
const HEATMAP_COLORS = ["#161b22", "#0e4429", "#006d32", "#26a641", "#39d353"] as const;
const HEATMAP_EMPTY = "#161b22";

/** 本地日期字符串 (YYYY-MM-DD)，避免 toISOString 的 UTC 偏移 */
function toLocalDateStr(d: Date): string {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

/** 计算年度日历网格数据 */
const heatmapGrid = computed(() => {
  const statsMap = new Map<string, number>();
  let maxSeconds = 0;
  heatmapStats.value.forEach((d) => {
    statsMap.set(d.date, d.total_seconds);
    if (d.total_seconds > maxSeconds) maxSeconds = d.total_seconds;
  });
  if (maxSeconds === 0) maxSeconds = 1;

  const today = new Date();
  const endDate = new Date(today);
  const startDate = new Date(today);
  startDate.setFullYear(startDate.getFullYear() - 1);

  // 找到 endDate 所在周的周六（本周结束，周六=6）
  const endSaturday = new Date(endDate);
  endSaturday.setDate(endSaturday.getDate() + (6 - endSaturday.getDay()));

  // 找到 startDate 所在周的周日（本周开始，周日=0）
  const startSunday = new Date(startDate);
  startSunday.setDate(startSunday.getDate() - startSunday.getDay());

  // 生成周列表（从周日开始，与 GitHub 一致）
  const weeks: Array<Array<{ date: string; seconds: number; level: number; inRange: boolean }>> = [];
  const cursor = new Date(startSunday);

  while (cursor <= endSaturday) {
    const week: Array<{ date: string; seconds: number; level: number; inRange: boolean }> = [];
    for (let day = 0; day < 7; day++) {
      const dateStr = toLocalDateStr(cursor);
      const inRange = cursor >= startDate && cursor <= endDate;
      const seconds = statsMap.get(dateStr) || 0;

      // 计算颜色等级 (0-4)
      let level = 0;
      if (seconds > 0) {
        const ratio = seconds / maxSeconds;
        if (ratio <= 0.25) level = 1;
        else if (ratio <= 0.5) level = 2;
        else if (ratio <= 0.75) level = 3;
        else level = 4;
      }

      week.push({ date: dateStr, seconds, level, inRange });
      cursor.setDate(cursor.getDate() + 1);
    }
    weeks.push(week);
  }

  // 生成月份标签
  const monthLabels: Array<{ label: string; weekIndex: number }> = [];
  let lastMonth = -1;
  weeks.forEach((week, weekIdx) => {
    // 用周日的日期判断月份
    const sunDate = new Date(week[0].date + "T00:00:00");
    const m = sunDate.getMonth();
    if (m !== lastMonth) {
      const monthNames = ["1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月"];
      monthLabels.push({ label: monthNames[m], weekIndex: weekIdx });
      lastMonth = m;
    }
  });

  return { weeks, monthLabels, maxSeconds };
});

const heatmapTooltip = ref<{ show: boolean; x: number; y: number; date: string; hours: string; seconds: number }>({
  show: false, x: 0, y: 0, date: "", hours: "0", seconds: 0,
});

function onHeatmapCellHover(e: MouseEvent, cell: { date: string; seconds: number }) {
  const hours = (cell.seconds / 3600).toFixed(1);
  heatmapTooltip.value = {
    show: true,
    x: e.clientX,
    y: e.clientY,
    date: cell.date,
    hours,
    seconds: cell.seconds,
  };
}

function onHeatmapCellLeave() {
  heatmapTooltip.value.show = false;
}

function getHeatmapCellColor(level: number): string {
  return HEATMAP_COLORS[level] || HEATMAP_EMPTY;
}

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
      backgroundColor: "rgba(22, 27, 34, 0.95)",
      borderColor: "rgba(48, 54, 61, 0.8)",
      borderWidth: 1,
      textStyle: { color: "#e6edf3", fontSize: 12 },
      formatter: (params: any) => {
        const hour = params.data[0];
        const day = weekdays[params.data[1]];
        const hrs = (params.data[2] / 3600).toFixed(1);
        return `<div style="font-weight:600;margin-bottom:2px">${day} ${hour}:00</div><div style="color:#8b949e">${hrs}h 游玩时长</div>`;
      },
    },
    grid: {
      top: 10,
      bottom: 50,
      left: 48,
      right: 10,
      containLabel: false,
    },
    xAxis: {
      type: "category",
      data: hours.map((h) => `${h}`),
      splitArea: { show: false },
      axisLabel: {
        color: "#8b949e",
        fontSize: 10,
        interval: (idx: number) => idx % 3 === 0,
        formatter: (val: string) => `${val}时`,
      },
      axisLine: { show: false },
      axisTick: { show: false },
      splitLine: { show: false },
    },
    yAxis: {
      type: "category",
      data: weekdays,
      splitArea: { show: false },
      axisLabel: { color: "#8b949e", fontSize: 11 },
      axisLine: { show: false },
      axisTick: { show: false },
      splitLine: { show: false },
    },
    visualMap: {
      min: 0,
      max: maxSeconds,
      show: false,
      inRange: {
        color: HEATMAP_COLORS,
      },
    },
    series: [
      {
        type: "heatmap",
        data,
        label: { show: false },
        itemStyle: {
          borderColor: "rgba(22, 27, 34, 0.6)",
          borderWidth: 2,
          borderRadius: 3,
        },
        emphasis: {
          itemStyle: {
            borderColor: "#e6edf3",
            borderWidth: 1,
            shadowBlur: 0,
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

// 切换到游玩记录 tab 时始终刷新数据
watch(activeTab, (tab) => {
  if (tab === "sessions") {
    loadSessionHistory(true);
  }
});

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
    loadGameColors(ps).catch(() => {});
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

function onWindowResize() {
  updateGridCols();
  updateHourlyHeight();
}

onMounted(() => {
  loadStats();
  updateGridCols();
  updateHourlyHeight();
  window.addEventListener('resize', onWindowResize);
});

// 当 store 中的封面缓存更新时，补充提取颜色（使用 shallow watch 避免深层遍历）
watch(() => Object.keys(gamesStore.coverBase64Cache).length, (newLen, oldLen) => {
  if (newLen > (oldLen ?? 0) && playStats.value.length > 0) {
    loadGameColors(playStats.value).catch(() => {});
  }
});

// keep-alive 缓存的组件再次激活时刷新数据
onActivated(() => {
  loadStats();
  // 如果当前正在查看游玩记录，也刷新会话历史
  if (activeTab.value === "sessions") {
    loadSessionHistory(true);
  }
  updateGridCols();
  nextTick(() => updateHourlyHeight());
});

// 组件卸载时清理事件监听器
onUnmounted(() => {
  window.removeEventListener('resize', onWindowResize);
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
                <div class="gh-heatmap-wrapper">
                  <!-- 月份标签 -->
                  <div
                    class="gh-month-labels"
                    :style="{ '--gh-weeks': heatmapGrid.weeks.length }"
                  >
                    <span
                      v-for="m in heatmapGrid.monthLabels"
                      :key="m.weekIndex"
                      class="gh-month-label"
                      :style="{ gridColumn: m.weekIndex + 1 }"
                    >{{ m.label }}</span>
                  </div>
                  <div class="gh-heatmap-body">
                    <!-- 星期标签（与 7 行对齐，只在 Mon/Wed/Fri 显示） -->
                    <div class="gh-day-labels">
                      <span class="gh-day-label gh-day-label-empty">.</span>
                      <span class="gh-day-label">一</span>
                      <span class="gh-day-label gh-day-label-empty">.</span>
                      <span class="gh-day-label">三</span>
                      <span class="gh-day-label gh-day-label-empty">.</span>
                      <span class="gh-day-label">五</span>
                      <span class="gh-day-label gh-day-label-empty">.</span>
                    </div>
                    <!-- 网格 -->
                    <div class="gh-grid">
                      <div
                        v-for="(week, wIdx) in heatmapGrid.weeks"
                        :key="wIdx"
                        class="gh-week"
                      >
                        <div
                          v-for="(cell, dIdx) in week"
                          :key="dIdx"
                          class="gh-cell"
                          :class="{ 'gh-cell-empty': !cell.inRange }"
                          :style="{ backgroundColor: cell.inRange ? getHeatmapCellColor(cell.level) : 'transparent' }"
                          @mouseenter="onHeatmapCellHover($event, cell)"
                          @mouseleave="onHeatmapCellLeave"
                        />
                      </div>
                    </div>
                  </div>
                  <!-- 图例 -->
                  <div class="gh-legend">
                    <span class="gh-legend-text">{{ formatHours(0) }}h</span>
                    <div
                      v-for="i in 5"
                      :key="i"
                      class="gh-legend-cell"
                      :style="{ backgroundColor: HEATMAP_COLORS[i - 1] }"
                    />
                    <span class="gh-legend-text">{{ heatmapMaxHours }}h</span>
                  </div>
                </div>
                <!-- 自定义 tooltip -->
                <Teleport to="body">
                  <div
                    v-if="heatmapTooltip.show"
                    class="gh-tooltip"
                    :style="{ left: heatmapTooltip.x + 12 + 'px', top: heatmapTooltip.y - 40 + 'px' }"
                  >
                    <div class="gh-tooltip-time">{{ heatmapTooltip.hours }}h 游玩时长</div>
                    <div class="gh-tooltip-date">{{ heatmapTooltip.date }}</div>
                  </div>
                </Teleport>
              </n-card>
            </n-gi>
            <n-gi>
              <n-card title="游玩时段分布">
                <div class="hourly-heatmap-wrapper">
                  <v-chart :option="hourlyHeatmapOption" :style="{ height: hourlyChartHeight + 'px' }" autoresize />
                  <div class="gh-legend gh-legend-hourly">
                    <span class="gh-legend-text">{{ formatHours(0) }}h</span>
                    <div
                      v-for="i in 5"
                      :key="i"
                      class="gh-legend-cell"
                      :style="{ backgroundColor: HEATMAP_COLORS[i - 1] }"
                    />
                    <span class="gh-legend-text">{{ hourlyMaxHours }}h</span>
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

/* ==================== GitHub 风格年度热力图 ==================== */
.gh-heatmap-wrapper {
  padding: 8px 0;
  overflow-x: auto;
}

.gh-month-labels {
  display: grid;
  grid-template-columns: repeat(var(--gh-weeks, 53), 1fr);
  gap: 3px;
  margin-left: 36px;
  margin-bottom: 4px;
  height: 18px;
}

.gh-month-label {
  font-size: 11px;
  color: #8b949e;
  white-space: nowrap;
  position: relative;
}

.gh-heatmap-body {
  display: flex;
  gap: 0;
}

.gh-day-labels {
  display: grid;
  grid-template-rows: repeat(7, 1fr);
  gap: 3px;
  width: 28px;
  flex-shrink: 0;
}

.gh-day-label {
  font-size: 10px;
  color: #8b949e;
  line-height: 13px;
  display: flex;
  align-items: center;
}

/* 只在 Mon(1), Wed(3), Fri(5) 行显示标签 */
.gh-day-label-empty {
  visibility: hidden;
}

.gh-grid {
  display: flex;
  gap: 3px;
  width: 100%;
}

.gh-week {
  display: flex;
  flex-direction: column;
  gap: 3px;
  flex: 1;
  min-width: 0;
}

.gh-cell {
  width: 100%;
  aspect-ratio: 1;
  border-radius: 2px;
  outline: 1px solid rgba(27, 31, 35, 0.06);
  outline-offset: -1px;
  cursor: pointer;
  transition: outline-color 0.1s;
}

.gh-cell:hover {
  outline: 2px solid rgba(255, 255, 255, 0.3);
  outline-offset: -2px;
}

.gh-cell-empty {
  outline: none;
  cursor: default;
}

/* 图例 */
.gh-legend {
  display: flex;
  align-items: center;
  gap: 3px;
  margin-top: 12px;
  justify-content: flex-end;
  padding-right: 4px;
}

.gh-legend-text {
  font-size: 10px;
  color: #8b949e;
  margin: 0 4px;
}

.gh-legend-cell {
  width: 13px;
  height: 13px;
  border-radius: 2px;
  outline: 1px solid rgba(27, 31, 35, 0.06);
  outline-offset: -1px;
}

/* 自定义 tooltip */
.gh-tooltip {
  position: fixed;
  z-index: 9999;
  background: rgba(22, 27, 34, 0.95);
  border: 1px solid rgba(48, 54, 61, 0.8);
  border-radius: 6px;
  padding: 8px 12px;
  pointer-events: none;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
}

.gh-tooltip-time {
  font-size: 13px;
  font-weight: 600;
  color: #e6edf3;
  white-space: nowrap;
}

.gh-tooltip-date {
  font-size: 11px;
  color: #8b949e;
  margin-top: 2px;
  white-space: nowrap;
}

/* 时段热力图 */
.hourly-heatmap-wrapper {
  padding: 0 4px;
}

.gh-legend-hourly {
  margin-top: 8px;
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
