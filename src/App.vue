<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, defineAsyncComponent, h, type Component } from "vue";
import { darkTheme, NConfigProvider, NLayout, NLayoutSider, NLayoutContent, NMenu, NIcon, NMessageProvider, NDialogProvider } from "naive-ui";
import type { MenuOption } from "naive-ui";
import { HomeOutline, StatsChartOutline, SettingsOutline, GameControllerOutline } from "@vicons/ionicons5";
import HomeView from "./views/HomeView.vue";
import { useGamesStore } from "./stores/games";

// 懒加载非首屏视图，减少初始包体积（ECharts ~800KB 只在访问统计页时加载）
const StatsView = defineAsyncComponent(() => import("./views/StatsView.vue"));
const SettingsView = defineAsyncComponent(() => import("./views/SettingsView.vue"));

const gamesStore = useGamesStore();
const activeView = ref("home");
const collapsed = ref(false);

// 视图组件映射，配合 keep-alive 和 component :is 使用
const viewComponents: Record<string, Component> = {
  home: HomeView,
  stats: StatsView,
  settings: SettingsView,
};

const currentComponent = computed(() => viewComponents[activeView.value]);

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

// 初始化
onMounted(async () => {
  await gamesStore.setupEventListeners();
  await gamesStore.loadGames();
});

// 清理事件监听器
onUnmounted(() => {
  gamesStore.cleanupEventListeners();
});
</script>

<template>
  <n-message-provider>
    <n-dialog-provider>
    <n-config-provider :theme="darkTheme" :theme-overrides="{ common: { primaryColor: '#6366f1' } }">
      <n-layout has-sider style="height: 100vh">
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
            <n-icon size="28" color="#6366f1">
              <GameControllerOutline />
            </n-icon>
            <span v-if="!collapsed" class="logo-text">Game Vault</span>
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
        <n-layout-content :native-scrollbar="false" style="height: 100vh">
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

body {
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
  background-color: #1a1a2e;
  color: #e0e0e0;
}

.logo {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 20px 16px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.1);
}

.logo.collapsed {
  padding: 20px 0;
}

.logo-text {
  font-size: 18px;
  font-weight: 700;
  color: #6366f1;
  white-space: nowrap;
}

.main-content {
  padding: 24px;
  min-height: 100vh;
  background: linear-gradient(135deg, #1a1a2e 0%, #16213e 100%);
}
</style>
