<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, defineAsyncComponent, h, type Component, watch } from "vue";
import { darkTheme, lightTheme, NConfigProvider, NLayout, NLayoutSider, NLayoutContent, NMenu, NIcon, NMessageProvider, NDialogProvider, createDiscreteApi } from "naive-ui";
import type { MenuOption } from "naive-ui";
import { HomeOutline, StatsChartOutline, SettingsOutline, GameControllerOutline } from "@vicons/ionicons5";
import HomeView from "./views/HomeView.vue";
import { useGamesStore } from "./stores/games";
import * as api from "./lib/tauri";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";

// 懒加载非首屏视图，减少初始包体积（ECharts ~800KB 只在访问统计页时加载）
const StatsView = defineAsyncComponent(() => import("./views/StatsView.vue"));
const SettingsView = defineAsyncComponent(() => import("./views/SettingsView.vue"));

const gamesStore = useGamesStore();
const activeView = ref("home");
const collapsed = ref(false);

// 主题状态
const accentColor = ref("#6366f1");
const isDark = ref(true);

// 视图组件映射，配合 keep-alive 和 component :is 使用
const viewComponents: Record<string, Component> = {
  home: HomeView,
  stats: StatsView,
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

// 颜色工具函数
function lightenColor(hex: string, percent: number): string {
  const num = parseInt(hex.replace("#", ""), 16);
  const r = Math.min(255, ((num >> 16) & 0xff) + Math.round(255 * percent / 100));
  const g = Math.min(255, ((num >> 8) & 0xff) + Math.round(255 * percent / 100));
  const b = Math.min(255, (num & 0xff) + Math.round(255 * percent / 100));
  return `#${((r << 16) | (g << 8) | b).toString(16).padStart(6, "0")}`;
}

function darkenColor(hex: string, percent: number): string {
  const num = parseInt(hex.replace("#", ""), 16);
  const r = Math.max(0, ((num >> 16) & 0xff) - Math.round(255 * percent / 100));
  const g = Math.max(0, ((num >> 8) & 0xff) - Math.round(255 * percent / 100));
  const b = Math.max(0, (num & 0xff) - Math.round(255 * percent / 100));
  return `#${((r << 16) | (g << 8) | b).toString(16).padStart(6, "0")}`;
}

// 从设置加载主题
async function loadThemeSettings() {
  try {
    const settings = await api.getSettings();
    accentColor.value = settings.accent_color || "#6366f1";
    isDark.value = settings.theme !== "light";
  } catch (e) {
    console.error("加载主题设置失败:", e);
  }
}

// 暴露给子组件调用
function updateAccentColor(color: string) {
  accentColor.value = color;
}

function updateTheme(dark: boolean) {
  isDark.value = dark;
}

// 提供给子组件
(window as any).__updateAccentColor = updateAccentColor;
(window as any).__updateTheme = updateTheme;

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
    label: "设置",
    key: "settings",
    icon: () => h(NIcon, null, { default: () => h(SettingsOutline) }),
  },
];

function handleMenuUpdate(key: string) {
  activeView.value = key;
}

// 监听主题变化，更新 CSS 变量
watch([accentColor, isDark], () => {
  document.documentElement.style.setProperty("--accent-color", accentColor.value);
  document.documentElement.classList.toggle("light-theme", !isDark.value);
});

// 监听窗口关闭事件，弹出自定义确认对话框
let unlistenClose: (() => void) | null = null;

onMounted(async () => {
  await gamesStore.setupEventListeners();
  await gamesStore.loadGames();
  await loadThemeSettings();
  document.documentElement.style.setProperty("--accent-color", accentColor.value);

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
onUnmounted(() => {
  gamesStore.cleanupEventListeners();
  unlistenClose?.();
});
</script>

<template>
  <n-message-provider>
    <n-dialog-provider>
    <n-config-provider :theme="isDark ? darkTheme : lightTheme" :theme-overrides="themeOverrides">
      <n-layout has-sider style="height: 100vh" @contextmenu.prevent>
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
          style="height: 100vh"
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
        <n-layout-content :native-scrollbar="false" class="main-layout-content" style="height: 100vh">
          <div class="main-content">
            <keep-alive>
              <component :is="currentComponent" />
            </keep-alive>
          </div>
        </n-layout-content>
      </n-layout>
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
  min-height: 100vh;
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
