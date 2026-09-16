<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, defineAsyncComponent, h, type Component, watch, provide } from "vue";
import { darkTheme, lightTheme, NConfigProvider, NLayout, NLayoutSider, NLayoutContent, NMenu, NIcon, NMessageProvider, NDialogProvider, createDiscreteApi } from "naive-ui";
import type { MenuOption } from "naive-ui";
import { HomeOutline, StatsChartOutline, SettingsOutline, GameControllerOutline, EllipsisVertical, SquareOutline, CopyOutline, RemoveOutline, CloseOutline, TrophyOutline, StarOutline } from "@vicons/ionicons5";
import HomeView from "./views/HomeView.vue";
import AboutModal from "./components/AboutModal.vue";
import AchievementToast from "./components/AchievementToast.vue";
import { useGamesStore } from "./stores/games";
import * as api from "./lib/tauri";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { PhysicalSize } from "@tauri-apps/api/dpi";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { DEFAULT_ACCENT_COLOR } from "./lib/constants";
import { lightenColor, darkenColor, isValidHexColor } from "./lib/color";

// 懒加载非首屏视图，减少初始包体积（ECharts ~800KB 只在访问统计页时加载）
const StatsView = defineAsyncComponent(() => import("./views/StatsView.vue"));
const SettingsView = defineAsyncComponent(() => import("./views/SettingsView.vue"));
const AchievementsView = defineAsyncComponent(() => import("./views/AchievementsView.vue"));
const ReviewsView = defineAsyncComponent(() => import("./views/ReviewsView.vue"));

const gamesStore = useGamesStore();
const activeView = ref("home");
const collapsed = ref(false);

// 主题状态
const accentColor = ref(DEFAULT_ACCENT_COLOR);
const isDark = ref(true);
const isMaximized = ref(false);
const appVersion = ref("0.0.0");
const showAbout = ref(false);

// 视图组件映射，配合 keep-alive 和 component :is 使用
const viewComponents: Record<string, Component> = {
  home: HomeView,
  stats: StatsView,
  achievements: AchievementsView,
  reviews: ReviewsView,
  settings: SettingsView,
};

const currentComponent = computed(() => viewComponents[activeView.value]);

// 动态主题覆盖
const themeOverrides = computed(() => ({
  common: {
    primaryColor: accentColor.value,
    primaryColorHover: lightenColor(accentColor.value, 15),
    primaryColorPressed: darkenColor(accentColor.value, 15),
    primaryColorSuppl: accentColor.value,
  }
}));

// 从设置加载主题
async function loadThemeSettings() {
  try {
    const settings = await api.getSettings();
    accentColor.value = settings.accent_color || DEFAULT_ACCENT_COLOR;
    isDark.value = settings.theme !== "light";
  } catch (e) {
    console.error("加载主题设置失败:", e);
  }
}

// 暴露给子组件调用
function updateAccentColor(color: string) {
  // 校验合法性，避免非法值（如输入一半的 "#63"）导致 NaN 颜色与 Naive UI 报错
  if (!isValidHexColor(color)) return;
  accentColor.value = color;
}

function updateTheme(dark: boolean) {
  isDark.value = dark;
  document.documentElement.classList.toggle("light-theme", !dark);
}

// 通过 provide/inject 提供给子组件（替代全局 window 属性）
provide("accentColor", accentColor);
provide("updateAccentColor", updateAccentColor);
provide("updateTheme", updateTheme);

const menuOptions: MenuOption[] = [
  {
    label: "游戏库",
    key: "home",
    icon: () => h(NIcon, null, { default: () => h(HomeOutline) }),
  },
  {
    label: "游戏统计",
    key: "stats",
    icon: () => h(NIcon, null, { default: () => h(StatsChartOutline) }),
  },
  {
    label: "成就",
    key: "achievements",
    icon: () => h(NIcon, null, { default: () => h(TrophyOutline) }),
  },
  {
    label: "游戏手账",
    key: "reviews",
    icon: () => h(NIcon, null, { default: () => h(StarOutline) }),
  },
  {
    label: "设置",
    key: "settings",
    icon: () => h(NIcon, null, { default: () => h(SettingsOutline) }),
  },
];

function handleMenuUpdate(key: string) {
  activeView.value = key;
}

// 标题栏拖拽与双击最大化
let lastMouseUpTime = 0;

async function handleTitleBarDrag(e: MouseEvent) {
  // 只响应左键
  if (e.button !== 0) return;

  const now = Date.now();
  const timeSinceLastUp = now - lastMouseUpTime;

  // 如果距离上次鼠标释放 < 300ms，视为双击 -> 最大化
  if (timeSinceLastUp < 300) {
    await handleToggleMaximize();
    return;
  }

  // 单击拖拽：立即启动
  await getCurrentWindow().startDragging();
  // startDragging() 返回说明鼠标已释放，记录释放时间
  lastMouseUpTime = Date.now();
}

// 窗口控制
async function handleMinimize() {
  await getCurrentWindow().minimize();
}

async function handleToggleMaximize() {
  await getCurrentWindow().toggleMaximize();
  isMaximized.value = await getCurrentWindow().isMaximized();
}

async function handleClose() {
  await getCurrentWindow().close();
}

// 监听主题变化，更新 CSS 变量
watch([accentColor, isDark], () => {
  document.documentElement.style.setProperty("--accent-color", accentColor.value);
  document.documentElement.classList.toggle("light-theme", !isDark.value);
});

// 监听窗口关闭事件，弹出自定义确认对话框
let unlistenClose: (() => void) | null = null;

// ---- 视口看门狗：自愈 WebView2 渲染进程偶发丢失 resize 的竞态 ----
//
// 症状（2026-09-16 实拍）：点最大化后窗口本体已铺满，但页面布局停在旧视口
// （DOM 卡在左上角，其余区域露出网页背景色）。实测 14 种窗口路径无法在开发
// 环境复现：窗口 HWND 全部正确拉伸（wry WM_SIZE → SetBounds 链路完好），
// 唯 Chromium 渲染进程没收到 resize —— 属 WebView2 内部竞态，应用层无根因
// 可修，只能检测 + 重放 resize 自愈，并把证据落日志（log_frontend_diag）。
let vpToken = 0; // 防抖令牌：resize 风暴中只保留最新一次检测
let vpRepairCount = 0; // 同一段 resize 内的自愈次数（上限 2，防循环折腾）

async function viewportGuardCheck(expectW: number, expectH: number) {
  const token = ++vpToken;
  // 给正常链路留出完成时间（拖动边缘的连续 resize 期间，令牌失效的旧检查直接让位）
  await new Promise((r) => setTimeout(r, 160));
  if (token !== vpToken) return;

  const dw = Math.abs(window.innerWidth - expectW);
  const dh = Math.abs(window.innerHeight - expectH);
  if (dw <= 2 && dh <= 2) {
    vpRepairCount = 0; // 视口健康（或自愈成功），清零计数
    return;
  }
  if (vpRepairCount >= 2) return; // 两轮自愈仍卡死：放弃，避免无限循环
  vpRepairCount += 1;

  const win = getCurrentWindow();
  const detail = `视口未跟随窗口尺寸: 页面=${window.innerWidth}x${window.innerHeight} 期望=${expectW}x${expectH}（自愈第 ${vpRepairCount}/2 次）`;
  console.error("[viewport-guard]", detail);
  api.logDiag(detail).catch(() => {});

  if (await win.isMaximized()) {
    // 最大化态：重放一次「还原 → 最大化」，整条 resize 链路重走一遍
    await win.unmaximize();
    await new Promise((r) => setTimeout(r, 250));
    await win.maximize();
  } else {
    // 普通态：物理尺寸抖动 1px，强制 wry 重新 SetBounds
    const size = await win.outerSize();
    await win.setSize(new PhysicalSize(size.width - 1, size.height - 1));
    await new Promise((r) => setTimeout(r, 120));
    await win.setSize(new PhysicalSize(size.width, size.height));
  }
  // 自愈动作本身会再触发 onResized → 由下一轮回调复查；同步标题栏图标状态
  isMaximized.value = await win.isMaximized();
}

function expectViewportFrom(physical: { width: number; height: number }) {
  const dpr = window.devicePixelRatio || 1;
  return viewportGuardCheck(Math.round(physical.width / dpr), Math.round(physical.height / dpr));
}

onMounted(async () => {
  await gamesStore.setupEventListeners();
  await gamesStore.loadGames();
  await loadThemeSettings();
  document.documentElement.style.setProperty("--accent-color", accentColor.value);

  // 后台检查存档路径状态（不阻塞 UI）
  gamesStore.checkSavePaths().catch((e) => {
    console.error("启动时检查存档路径失败:", e);
  });

  // 获取应用版本号
  try {
    appVersion.value = await getVersion();
  } catch (e) {
    console.error("获取版本号失败:", e);
  }

  // 监听最大化状态变化
  const win = getCurrentWindow();
  isMaximized.value = await win.isMaximized();

  // 视口看门狗：注册 resize 监听 + 启动时主动查一次（竞态也可能发生在启动路径）
  unlistenResize = await win.onResized(({ payload }) => {
    void expectViewportFrom(payload);
  });
  void expectViewportFrom(await win.innerSize());

  // 每次弹出时使用当前主题创建 discrete API，确保主题实时同步
  unlistenClose = await listen("close-requested", () => {
    const { dialog } = createDiscreteApi(["dialog"], {
      configProviderProps: {
        theme: isDark.value ? darkTheme : lightTheme,
        themeOverrides: themeOverrides.value,
      },
    });
    dialog.warning({
      title: "关闭确认",
      content: "您想要最小化到系统托盘，还是直接退出程序？",
      positiveText: "最小化到托盘",
      negativeText: "直接退出",
      closable: true,
      onPositiveClick: () => {
        getCurrentWindow().hide();
      },
      onNegativeClick: () => {
        api.quitApp();
      },
    });
  });
});

// 清理事件监听器
let unlistenResize: (() => void) | null = null;
onUnmounted(() => {
  gamesStore.cleanupEventListeners();
  unlistenClose?.();
  unlistenResize?.();
});
</script>

<template>
  <n-message-provider>
    <n-dialog-provider>
    <n-config-provider :theme="isDark ? darkTheme : lightTheme" :theme-overrides="themeOverrides">
      <div class="app-container">
        <!-- 自定义标题栏 -->
        <div class="title-bar" @mousedown="handleTitleBarDrag">
          <div class="title-bar-left">
            <n-icon size="16" :color="accentColor">
              <GameControllerOutline />
            </n-icon>
            <span class="title-bar-text">Game Vault</span>
          </div>
          <div class="title-bar-controls">
            <button class="about-btn" @mousedown.stop @click="showAbout = true" title="关于">
              <n-icon size="16"><EllipsisVertical /></n-icon>
            </button>
            <button class="title-btn" @mousedown.stop @click="handleMinimize" title="最小化">
              <n-icon size="16"><RemoveOutline /></n-icon>
            </button>
            <button class="title-btn" @mousedown.stop @click="handleToggleMaximize" title="最大化">
              <n-icon size="14">
                <CopyOutline v-if="isMaximized" />
                <SquareOutline v-else />
              </n-icon>
            </button>
            <button class="title-btn title-btn-close" @mousedown.stop @click="handleClose" title="关闭">
              <n-icon size="16"><CloseOutline /></n-icon>
            </button>
          </div>
        </div>

        <!-- 主布局 -->
        <n-layout has-sider style="flex: 1; overflow: hidden" @contextmenu.prevent>
          <!-- 侧边栏 -->
          <n-layout-sider
            bordered
            :collapsed="collapsed"
            :collapsed-width="64"
            :width="200"
            collapse-mode="width"
            show-trigger
            @collapse="collapsed = true"
            @expand="collapsed = false"
            :native-scrollbar="false"
          >
            <div class="logo" :class="{ collapsed }">
              <n-icon size="28" :color="accentColor">
                <GameControllerOutline />
              </n-icon>
              <span v-if="!collapsed" class="logo-text" :style="{ color: accentColor }">Game Vault</span>
            </div>
            <n-menu
              :collapsed="collapsed"
              :collapsed-width="64"
              :collapsed-icon-size="22"
              :options="menuOptions"
              :value="activeView"
              @update:value="handleMenuUpdate"
            />
          </n-layout-sider>

          <!-- 主内容区 -->
          <n-layout-content :native-scrollbar="false" class="main-layout-content">
            <div class="main-content">
              <keep-alive>
                <component :is="currentComponent" />
              </keep-alive>
            </div>
          </n-layout-content>
        </n-layout>
      </div>

      <!-- 关于对话框 -->
      <AboutModal :show="showAbout" :version="appVersion" @close="showAbout = false" />
      <!-- 成就解锁通知 -->
      <AchievementToast />
    </n-config-provider>
    </n-dialog-provider>
  </n-message-provider>
</template>

<style>
* {
  margin: 0;
  padding: 0;
  box-sizing: border-box;
}

/* 从 html 层就开始设置背景，杜绝任何露黑的可能 */
html {
  background-color: #16213e;
}

body {
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
  background: linear-gradient(135deg, #1a1a2e 0%, #16213e 100%);
  color: #e0e0e0;
  transition: background 0.3s, color 0.3s;
}

/* n-layout 本身有 Naive UI 主题背景色，需要覆盖 */
.n-layout {
  background: linear-gradient(135deg, #1a1a2e 0%, #16213e 100%) !important;
}

.main-content {
  padding: 24px;
  min-height: 100%;
  background: linear-gradient(135deg, #1a1a2e 0%, #16213e 100%);
  transition: background 0.3s;
}

/* 亮色主题 */
.light-theme html {
  background-color: #e8e8e8;
}

.light-theme body {
  background: linear-gradient(135deg, #f5f5f5 0%, #e8e8e8 100%);
  color: #333;
}

.light-theme .n-layout {
  background: linear-gradient(135deg, #f5f5f5 0%, #e8e8e8 100%) !important;
}

.light-theme .n-layout-sider {
  background: #fff !important;
}

.light-theme .main-content {
  background: linear-gradient(135deg, #f5f5f5 0%, #e8e8e8 100%) !important;
}

.app-container {
  display: flex;
  flex-direction: column;
  height: 100vh;
  overflow: hidden;
}

.title-bar {
  height: 32px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 8px 0 12px;
  background: #16213e;
  border-bottom: 1px solid rgba(255, 255, 255, 0.06);
  flex-shrink: 0;
  user-select: none;
  cursor: default;
}

.light-theme .title-bar {
  background: #f0f0f0;
  border-bottom: 1px solid rgba(0, 0, 0, 0.08);
}

.title-bar-left {
  display: flex;
  align-items: center;
  gap: 8px;
}

.title-bar-text {
  font-size: 12px;
  font-weight: 600;
  color: rgba(255, 255, 255, 0.7);
}

.light-theme .title-bar-text {
  color: rgba(0, 0, 0, 0.7);
}

.title-bar-controls {
  display: flex;
  align-items: center;
  gap: 2px;
}

.about-btn {
  width: 32px;
  height: 28px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  background: transparent;
  color: rgba(255, 255, 255, 0.6);
  cursor: pointer;
  border-radius: 4px;
  transition: background 0.15s, color 0.15s;
}

.about-btn:hover {
  background: rgba(255, 255, 255, 0.1);
  color: rgba(255, 255, 255, 0.9);
}

.light-theme .about-btn {
  color: rgba(0, 0, 0, 0.6);
}

.light-theme .about-btn:hover {
  background: rgba(0, 0, 0, 0.06);
  color: rgba(0, 0, 0, 0.9);
}

.title-btn {
  width: 36px;
  height: 28px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  background: transparent;
  color: rgba(255, 255, 255, 0.7);
  cursor: pointer;
  border-radius: 4px;
  transition: background 0.15s, color 0.15s;
}

.title-btn:hover {
  background: rgba(255, 255, 255, 0.1);
  color: #fff;
}

.light-theme .title-btn {
  color: rgba(0, 0, 0, 0.7);
}

.light-theme .title-btn:hover {
  background: rgba(0, 0, 0, 0.06);
  color: #000;
}

.title-btn-close:hover {
  background: #e81123;
  color: #fff;
}

.light-theme .title-btn-close:hover {
  background: #e81123;
  color: #fff;
}

.logo {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 20px 16px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.1);
}

.light-theme .logo {
  border-bottom: 1px solid rgba(0, 0, 0, 0.1);
}

.logo.collapsed {
  padding: 20px 0;
}

.logo-text {
  font-size: 18px;
  font-weight: 700;
  white-space: nowrap;
}
</style>
