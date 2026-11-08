<script setup lang="ts">
import { h } from "vue";
import { NButton, NDropdown, NIcon, NTooltip, type DropdownOption } from "naive-ui";
import {
  AddOutline,
  CloudDownloadOutline,
  DocumentTextOutline,
  FolderOutline,
  RefreshOutline,
} from "@vicons/ionicons5";
import { openAddGame } from "../lib/addGameFlow";
import { useRefreshActions } from "../lib/refreshActions";

/**
 * 侧边栏左下角的全局动作：刷新（三种）+ 添加游戏。
 *
 * 2026-11-08 从主页顶部工具栏搬来：这两个动作与「当前在哪个视图」无关，
 * 放在侧边栏才是全局入口；顶部只留搜索与筛选，视觉上也清静。
 * 功能原样保留——刷新仍是合并下拉（封面 / 信息 / 存档），添加仍是三来源弹窗。
 */
defineProps<{ collapsed: boolean }>();

const { busy, refreshCovers, refreshAllInfo, checkSaves } = useRefreshActions();

const refreshMenuOptions: DropdownOption[] = [
  {
    label: "刷新封面",
    key: "covers",
    icon: () => h(NIcon, null, { default: () => h(CloudDownloadOutline) }),
  },
  {
    label: "刷新游戏信息",
    key: "info",
    icon: () => h(NIcon, null, { default: () => h(DocumentTextOutline) }),
  },
  {
    label: "检查存档",
    key: "saves",
    icon: () => h(NIcon, null, { default: () => h(FolderOutline) }),
  },
];

// 下拉项只有四个字，各自干什么并不显然——悬停补一句完整说明
const refreshHint = [
  "刷新封面 — 从 SteamGridDB 补齐缺失的封面图",
  "刷新游戏信息 — 调用 AI 补全简介、类型、发售日期",
  "检查存档 — 检测各游戏存档目录是否配置正确",
].join("\n");

function handleRefreshSelect(key: string | number) {
  if (key === "covers") {
    refreshCovers();
  } else if (key === "info") {
    refreshAllInfo();
  } else if (key === "saves") {
    checkSaves();
  }
}
</script>

<template>
  <div class="sider-actions" :class="{ 'is-collapsed': collapsed }">
    <!-- 提示挂在包裹层 div 上，不用组件套组件：dropdown 与 tooltip 各自绑定
         一个真实 DOM 节点，避免两套弹层共用同一 trigger 时互相抢事件 -->
    <n-tooltip placement="right" :delay="600" :style="{ maxWidth: '300px', whiteSpace: 'pre-line' }">
      <template #trigger>
        <div class="act-slot">
          <n-dropdown
            trigger="click"
            placement="right-start"
            :options="refreshMenuOptions"
            @select="handleRefreshSelect"
          >
            <n-button
              secondary
              class="act-btn"
              :circle="collapsed"
              :block="!collapsed"
              :loading="busy"
            >
              <template #icon>
                <n-icon :component="RefreshOutline" />
              </template>
              <template v-if="!collapsed">刷新</template>
            </n-button>
          </n-dropdown>
        </div>
      </template>
      {{ refreshHint }}
    </n-tooltip>

    <n-tooltip placement="right" :delay="400" :disabled="!collapsed">
      <template #trigger>
        <div class="act-slot">
          <n-button
            type="primary"
            class="act-btn"
            :circle="collapsed"
            :block="!collapsed"
            @click="openAddGame()"
          >
            <template #icon>
              <n-icon :component="AddOutline" />
            </template>
            <template v-if="!collapsed">添加游戏</template>
          </n-button>
        </div>
      </template>
      添加游戏
    </n-tooltip>
  </div>
</template>

<style scoped>
.sider-actions {
  /* 父级 .sider-inner 是 flex 列且撑满侧栏高度，auto 把这一块压到底部 */
  margin-top: auto;
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 12px 10px 10px;
  border-top: 1px solid rgba(255, 255, 255, 0.07);
}

.sider-actions.is-collapsed {
  align-items: center;
  padding: 12px 0 10px;
}

.act-slot {
  width: 100%;
}

.is-collapsed .act-slot {
  width: auto;
  display: flex;
  justify-content: center;
}

.act-btn {
  /* 折叠态是圆形图标键，展开态铺满侧栏 */
  font-size: 13px;
}

.light-theme .sider-actions {
  border-top-color: rgba(0, 0, 0, 0.08);
}
</style>
