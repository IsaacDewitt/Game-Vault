<script setup lang="ts">
import { ref, onMounted, onUnmounted, watch, inject } from "vue";
import {
  NCard,
  NForm,
  NFormItem,
  NInput,
  NSwitch,
  NButton,
  NSpace,
  NSelect,
  NIcon,
  NInputNumber,
  useMessage,
} from "naive-ui";
import { DownloadOutline, CloudUploadOutline } from "@vicons/ionicons5";
import { save, open } from "@tauri-apps/plugin-dialog";
import { writeTextFile, readTextFile } from "@tauri-apps/plugin-fs";
import * as api from "../lib/tauri";
import type { Settings } from "../lib/tauri";
import { DEFAULT_ACCENT_COLOR, DEBOUNCE_MS } from "../lib/constants";
import { isValidHexColor } from "../lib/color";

const message = useMessage();

// 默认值仅用于表单初始显示，实际默认值从后端 API 获取
const settings = ref<Settings>({
  theme: "dark",
  language: "zh-CN",
  steamgriddb_api_key: "",
  llm_protocol: "",
  llm_api_key: "",
  llm_base_url: "",
  llm_model: "",
  llm_enabled: false,
  accent_color: DEFAULT_ACCENT_COLOR,
  window_width: 1400,
  window_height: 900,
});

// 窗口大小预设选项
const windowSizePresets = [
  { label: "1200 × 800", width: 1200, height: 800 },
  { label: "1400 × 900 (默认)", width: 1400, height: 900 },
  { label: "1600 × 1000", width: 1600, height: 1000 },
  { label: "1920 × 1080", width: 1920, height: 1080 },
];

const protocolOptions = [
  { label: "OpenAI 格式", value: "openai" },
  { label: "Anthropic 格式", value: "anthropic" },
];

// 预设主题色
const presetColors = [
  { label: "靛蓝", value: "#6366f1" },
  { label: "蓝色", value: "#3b82f6" },
  { label: "绿色", value: "#22c55e" },
  { label: "红色", value: "#f43f5e" },
  { label: "橙色", value: "#f97316" },
  { label: "黄色", value: "#eab308" },
  { label: "粉色", value: "#ec4899" },
  { label: "青色", value: "#14b8a6" },
];

// 加载中标记，避免初始化时触发自动保存
const loading = ref(true);

// 通过 inject 获取父组件提供的主题更新函数
const updateAccentColor = inject<(color: string) => void>("updateAccentColor");
const updateTheme = inject<(dark: boolean) => void>("updateTheme");

// 主题色变化时实时预览并自动保存
watch(() => settings.value.accent_color, (color) => {
  // 非法值（输入未完成等）不应用、不保存，避免 NaN 颜色
  if (!color || !isValidHexColor(color)) return;
  if (updateAccentColor) {
    updateAccentColor(color);
  }
  if (!loading.value) {
    autoSaveThemeSettings();
  }
});

// 主题切换时实时预览并自动保存
watch(() => settings.value.theme, (theme) => {
  if (updateTheme) {
    updateTheme(theme !== "light");
  }
  if (!loading.value) {
    autoSaveThemeSettings();
  }
});

// 防抖自动保存外观设置
let autoSaveTimer: ReturnType<typeof setTimeout> | null = null;
function autoSaveThemeSettings() {
  if (autoSaveTimer) clearTimeout(autoSaveTimer);
  autoSaveTimer = setTimeout(async () => {
    try {
      await api.saveSettings(settings.value);
    } catch (e) {
      console.error("自动保存外观设置失败:", e);
    }
  }, DEBOUNCE_MS);
}

const saving = ref(false);
const exporting = ref(false);
const importing = ref(false);
const exportingSaves = ref(false);
const importingSaves = ref(false);
const autostartEnabled = ref(false);
const autostartLoading = ref(false);

async function loadSettings() {
  loading.value = true;
  try {
    settings.value = await api.getSettings();
  } catch (e) {
    console.error("加载设置失败:", e);
  } finally {
    // 等待 DOM 更新后再解除加载标记，避免 watch 误触发保存
    setTimeout(() => { loading.value = false; }, 0);
  }
}

async function applyWindowSize() {
  const w = settings.value.window_width;
  const h = settings.value.window_height;
  if (w < 900 || h < 600) {
    message.warning("窗口最小尺寸为 900 × 600");
    return;
  }
  try {
    await api.setWindowSize(w, h);
    message.success("窗口大小已调整");
  } catch (e) {
    console.error("设置窗口大小失败:", e);
    message.error("设置窗口大小失败");
  }
}

function applyPresetSize(preset: { width: number; height: number }) {
  settings.value.window_width = preset.width;
  settings.value.window_height = preset.height;
  applyWindowSize();
}

async function saveSettings() {
  saving.value = true;
  try {
    await api.saveSettings(settings.value);
    message.success("设置已保存");
  } catch (e) {
    console.error("保存设置失败:", e);
    message.error("保存失败");
  } finally {
    saving.value = false;
  }
}

async function handleExportData() {
  exporting.value = true;
  try {
    const jsonData = await api.exportGameData();

    // 弹出保存文件对话框
    const filePath = await save({
      defaultPath: "gamevault-backup.json",
      filters: [
        {
          name: "JSON 文件",
          extensions: ["json"],
        },
      ],
    });

    if (filePath) {
      await writeTextFile(filePath, jsonData);
      message.success("数据导出成功");
    }
  } catch (e) {
    console.error("导出数据失败:", e);
    message.error("导出失败: " + (e as Error).toString());
  } finally {
    exporting.value = false;
  }
}

async function handleImportData() {
  importing.value = true;
  try {
    const selected = await open({
      multiple: false,
      title: "选择备份文件",
      filters: [
        {
          name: "JSON 文件",
          extensions: ["json"],
        },
      ],
    });

    if (!selected) {
      importing.value = false;
      return;
    }

    const jsonContent = await readTextFile(selected as string);
    const result = await api.importGameData(jsonContent);

    message.success(
      `导入成功！已恢复 ${result.imported_games} 个游戏${result.settings_restored ? "和设置" : ""}`
    );

    // 刷新游戏列表
    const { useGamesStore } = await import("../stores/games");
    const store = useGamesStore();
    await store.loadGames();
  } catch (e) {
    console.error("导入数据失败:", e);
    message.error("导入失败: " + (e as Error).toString());
  } finally {
    importing.value = false;
  }
}

async function loadAutostartState() {
  try {
    autostartEnabled.value = await api.getAutostartEnabled();
  } catch (e) {
    console.error("获取自启动状态失败:", e);
  }
}

async function toggleAutostart(enabled: boolean) {
  autostartLoading.value = true;
  try {
    await api.setAutostartEnabled(enabled);
    autostartEnabled.value = enabled;
    message.success(enabled ? "已开启开机自启动" : "已关闭开机自启动");
  } catch (e) {
    console.error("设置自启动失败:", e);
    message.error("设置自启动失败");
    // 恢复原状态
    autostartEnabled.value = !enabled;
  } finally {
    autostartLoading.value = false;
  }
}

onMounted(() => {
  loadSettings();
  loadAutostartState();
});

// 组件卸载时清理定时器
onUnmounted(() => {
  if (autoSaveTimer) {
    clearTimeout(autoSaveTimer);
    autoSaveTimer = null;
  }
});

async function handleExportSaves() {
  exportingSaves.value = true;
  try {
    const filePath = await save({
      defaultPath: "gamevault-saves-backup.zip",
      filters: [
        {
          name: "ZIP 文件",
          extensions: ["zip"],
        },
      ],
    });

    if (!filePath) {
      exportingSaves.value = false;
      return;
    }

    const result = await api.exportSavesBackup(filePath);

    if (result.errors.length > 0) {
      message.warning(
        `存档导出完成，成功 ${result.exported} 个，失败 ${result.errors.length} 个`
      );
    } else {
      message.success(`存档导出成功，共 ${result.exported} 个游戏存档`);
    }
  } catch (e) {
    console.error("导出存档失败:", e);
    message.error("导出存档失败: " + (e as Error).toString());
  } finally {
    exportingSaves.value = false;
  }
}

async function handleImportSaves() {
  importingSaves.value = true;
  try {
    const selected = await open({
      multiple: false,
      title: "选择存档备份文件",
      filters: [
        {
          name: "ZIP 文件",
          extensions: ["zip"],
        },
      ],
    });

    if (!selected) {
      importingSaves.value = false;
      return;
    }

    const result = await api.importSavesBackup(selected as string);

    if (result.errors.length > 0) {
      message.warning(
        `存档恢复完成，成功 ${result.restored} 个，失败 ${result.errors.length} 个`
      );
    } else {
      message.success(`存档恢复成功，共 ${result.restored} 个游戏存档`);
    }
  } catch (e) {
    console.error("导入存档失败:", e);
    message.error("导入存档失败: " + (e as Error).toString());
  } finally {
    importingSaves.value = false;
  }
}
</script>

<template>
  <div class="settings-view">
    <h2 style="margin-bottom: 24px">设置</h2>

    <!-- 外观设置 -->
    <n-card title="外观设置" style="margin-bottom: 16px">
      <n-form label-placement="left" label-width="140">
        <n-form-item label="主题模式">
          <n-switch
            :value="settings.theme !== 'light'"
            @update:value="(val: boolean) => settings.theme = val ? 'dark' : 'light'"
          >
            <template #checked>暗色</template>
            <template #unchecked>亮色</template>
          </n-switch>
        </n-form-item>
        <n-form-item label="主题色">
          <n-space align="center">
            <div
              v-for="color in presetColors"
              :key="color.value"
              class="color-swatch"
              :class="{ active: settings.accent_color === color.value }"
              :style="{ background: color.value }"
              :title="color.label"
              @click="settings.accent_color = color.value"
            />
            <n-input
              v-model:value="settings.accent_color"
              :placeholder="DEFAULT_ACCENT_COLOR"
              style="width: 120px"
              size="small"
            />
          </n-space>
        </n-form-item>
      </n-form>
    </n-card>

    <!-- 基本设置 -->
    <n-card title="基本设置" style="margin-bottom: 16px">
      <n-form label-placement="left" label-width="140">
        <n-form-item label="开机自启动">
          <n-switch
            v-model:value="autostartEnabled"
            :loading="autostartLoading"
            @update:value="toggleAutostart"
          />
          <span style="margin-left: 12px; font-size: 12px; color: #888">
            开启后将在系统启动时自动运行 Game Vault
          </span>
        </n-form-item>
        <n-form-item label="SteamGridDB API Key">
          <n-input
            v-model:value="settings.steamgriddb_api_key"
            type="password"
            show-password-on="click"
            placeholder="可选，用于自动获取封面图"
          />
        </n-form-item>
      </n-form>
    </n-card>

    <!-- 窗口设置 -->
    <n-card title="窗口设置" style="margin-bottom: 16px">
      <n-form label-placement="left" label-width="140">
        <n-form-item label="预设尺寸">
          <n-space>
            <n-button
              v-for="preset in windowSizePresets"
              :key="preset.label"
              :type="settings.window_width === preset.width && settings.window_height === preset.height ? 'primary' : 'default'"
              size="small"
              @click="applyPresetSize(preset)"
            >
              {{ preset.label }}
            </n-button>
          </n-space>
        </n-form-item>
        <n-form-item label="自定义尺寸">
          <n-space align="center">
            <n-input-number
              v-model:value="settings.window_width"
              :min="900"
              :max="3840"
              :step="50"
              size="small"
              style="width: 120px"
              placeholder="宽度"
            />
            <span>×</span>
            <n-input-number
              v-model:value="settings.window_height"
              :min="600"
              :max="2160"
              :step="50"
              size="small"
              style="width: 120px"
              placeholder="高度"
            />
            <n-button type="primary" size="small" @click="applyWindowSize">
              应用
            </n-button>
          </n-space>
        </n-form-item>
      </n-form>
    </n-card>

    <!-- LLM 游戏信息获取 -->
    <n-card title="LLM 游戏信息获取" style="margin-bottom: 16px">
      <template #header-extra>
        <n-switch v-model:value="settings.llm_enabled" />
      </template>
      <n-form label-placement="left" label-width="140">
        <n-form-item label="协议格式">
          <n-select
            v-model:value="settings.llm_protocol"
            :options="protocolOptions"
            style="width: 200px"
          />
          <span style="margin-left: 8px; font-size: 12px; color: #888">
            请求体和响应体的格式
          </span>
        </n-form-item>
        <n-form-item label="Base URL">
          <n-input
            v-model:value="settings.llm_base_url"
            placeholder="https://api.xiaomimimo.com/v1"
          />
        </n-form-item>
        <n-form-item label="模型名称">
          <n-input
            v-model:value="settings.llm_model"
            placeholder="mimo-v2.5-pro"
          />
        </n-form-item>
        <n-form-item label="API Key">
          <n-input
            v-model:value="settings.llm_api_key"
            type="password"
            show-password-on="click"
            placeholder="输入 API Key"
          />
        </n-form-item>
      </n-form>
    </n-card>

    <!-- 保存按钮 -->
    <n-space>
      <n-button type="primary" :loading="saving" @click="saveSettings">
        保存设置
      </n-button>
    </n-space>

    <!-- 数据管理 -->
    <n-card title="数据管理" style="margin-top: 16px">
      <n-form label-placement="left" label-width="140">
        <n-form-item label="导出游戏数据">
          <n-button :loading="exporting" @click="handleExportData">
            <template #icon>
              <n-icon :component="DownloadOutline" />
            </template>
            导出备份
          </n-button>
          <span style="margin-left: 12px; font-size: 12px; color: #888">
            导出所有游戏信息和设置为 JSON 文件
          </span>
        </n-form-item>
        <n-form-item label="导入游戏数据">
          <n-button :loading="importing" @click="handleImportData">
            <template #icon>
              <n-icon :component="CloudUploadOutline" />
            </template>
            导入备份
          </n-button>
          <span style="margin-left: 12px; font-size: 12px; color: #888">
            从之前导出的 JSON 文件恢复游戏数据和设置
          </span>
        </n-form-item>
        <n-form-item label="导出存档文件">
          <n-button :loading="exportingSaves" @click="handleExportSaves">
            <template #icon>
              <n-icon :component="DownloadOutline" />
            </template>
            导出存档
          </n-button>
          <span style="margin-left: 12px; font-size: 12px; color: #888">
            将所有游戏存档文件导出为 ZIP 压缩包
          </span>
        </n-form-item>
        <n-form-item label="导入存档文件">
          <n-button :loading="importingSaves" @click="handleImportSaves">
            <template #icon>
              <n-icon :component="CloudUploadOutline" />
            </template>
            导入存档
          </n-button>
          <span style="margin-left: 12px; font-size: 12px; color: #888">
            从 ZIP 备份文件恢复游戏存档到对应位置
          </span>
        </n-form-item>
      </n-form>
    </n-card>
  </div>
</template>

<style scoped>
.settings-view {
  max-width: 800px;
}

.color-swatch {
  width: 28px;
  height: 28px;
  border-radius: 6px;
  cursor: pointer;
  border: 2px solid transparent;
  transition: border-color 0.2s, transform 0.2s;
}

.color-swatch:hover {
  transform: scale(1.1);
}

.color-swatch.active {
  border-color: white;
  box-shadow: 0 0 0 2px rgba(255, 255, 255, 0.3);
}
</style>
