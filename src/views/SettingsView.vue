<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch, inject } from "vue";
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
import { DownloadOutline, CloudUploadOutline, FolderOpenOutline, SaveOutline } from "@vicons/ionicons5";
import { save, open } from "@tauri-apps/plugin-dialog";
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
  screenshot_dir: "",
  screenshot_hotkey: "F12",
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

// ==================== 未保存改动标记 ====================
// 只有下面这些字段需要手动点「保存设置」；主题色/主题/截图目录/快捷键都是改动后即自动保存，
// 不计入脏标记，否则会出现「刚改完主题就提示有未保存修改」的假警报。
const manualSaveFields = computed(() => ({
  steamgriddb_api_key: settings.value.steamgriddb_api_key,
  llm_enabled: settings.value.llm_enabled,
  llm_protocol: settings.value.llm_protocol,
  llm_base_url: settings.value.llm_base_url,
  llm_model: settings.value.llm_model,
  llm_api_key: settings.value.llm_api_key,
  window_width: settings.value.window_width,
  window_height: settings.value.window_height,
}));

const dirty = ref(false);
let baseline = "";

function markSaved() {
  baseline = JSON.stringify(manualSaveFields.value);
  dirty.value = false;
}

watch(
  manualSaveFields,
  (v) => {
    if (loading.value) return;
    dirty.value = JSON.stringify(v) !== baseline;
  },
  { deep: true }
);

// 通过 inject 获取父组件提供的主题更新函数
const updateAccentColor = inject<(color: string) => void>("updateAccentColor");
const updateTheme = inject<(dark: boolean) => void>("updateTheme");

// 主题色变化时实时预览并自动保存（只存主题色，不带手动字段）
watch(() => settings.value.accent_color, (color) => {
  // 非法值（输入未完成等）不应用、不保存，避免 NaN 颜色
  if (!color || !isValidHexColor(color)) return;
  if (updateAccentColor) {
    updateAccentColor(color);
  }
  if (!loading.value) {
    autoSaveThemeSettings({ accent_color: color });
  }
});

// 主题切换时实时预览并自动保存（只存主题，不带手动字段）
watch(() => settings.value.theme, (theme) => {
  if (updateTheme) {
    updateTheme(theme !== "light");
  }
  if (!loading.value) {
    autoSaveThemeSettings({ theme });
  }
});

// 防抖自动保存外观设置（partial：只更新 patch 内白名单字段）
//
// 主题与主题色两个 watch 共用这一个防抖器，故**必须累积 patch 而不是替换**
// （2026-09-12 修正）：此前每调用一次就覆盖整个 patch，300ms 内先切主题再改主题色
// （或反过来）会取消前一次、只落盘后者，重启后前一项改动丢失。
let autoSaveTimer: ReturnType<typeof setTimeout> | null = null;
let pendingThemePatch: api.AutoSavePatch = {};
function autoSaveThemeSettings(patch: api.AutoSavePatch) {
  pendingThemePatch = { ...pendingThemePatch, ...patch };
  if (autoSaveTimer) clearTimeout(autoSaveTimer);
  autoSaveTimer = setTimeout(async () => {
    const toSave = pendingThemePatch;
    pendingThemePatch = {};
    try {
      await api.saveSettingsPartial(toSave);
    } catch (e) {
      console.error("自动保存外观设置失败:", e);
      // 落盘失败则把改动并回待保存队列，下次触发时一并重试，避免静默丢失
      pendingThemePatch = { ...toSave, ...pendingThemePatch };
    }
  }, DEBOUNCE_MS);
}

// 截图目录修改后防抖自动保存（立即生效，无需手动点保存；同样只存目录本身）
watch(() => settings.value.screenshot_dir, () => {
  if (!loading.value) {
    autoSaveScreenshotDir();
  }
});

let screenshotDirSaveTimer: ReturnType<typeof setTimeout> | null = null;
function autoSaveScreenshotDir() {
  if (screenshotDirSaveTimer) clearTimeout(screenshotDirSaveTimer);
  screenshotDirSaveTimer = setTimeout(async () => {
    try {
      await api.saveSettingsPartial({ screenshot_dir: settings.value.screenshot_dir });
      message.success("截图目录已保存");
    } catch (e) {
      console.error("保存截图目录失败:", e);
      message.error("保存截图目录失败");
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
    // 以加载到的服务端数据为基线，之后的表单改动才判定为「有未保存修改」
    markSaved();
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
    markSaved();
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
      // 后端负责写入：主备份（脱敏密钥）+ 独立的 xxx.keys.json 密钥文件
      await api.exportGameData(filePath);
      message.success("数据导出成功（已同时生成独立密钥文件）");
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

    // 后端负责读取：主备份 + 自动识别同目录下的 xxx.keys.json 密钥文件
    const result = await api.importGameData(selected as string);

    message.success(
      `导入成功！已恢复 ${result.imported_games} 个游戏${result.settings_restored ? "和设置" : ""}${
        result.keys_restored > 0 ? `、${result.keys_restored} 个密钥` : ""
      }`
    );

    // 刷新游戏列表
    const { useGamesStore } = await import("../stores/games");
    const store = useGamesStore();
    await store.loadGames();

    // 备份里含设置时，必须把设置重新读进表单（2026-09-12 修正）：
    // 否则表单仍是导入前的旧值，用户下一次点「保存设置」会把旧值回写，等于抹掉刚恢复的设置。
    if (result.settings_restored) {
      await loadSettings();
    }
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

async function handleChooseScreenshotDir() {
  try {
    const selected = await open({
      multiple: false,
      directory: true,
      title: "选择截图保存目录",
    });
    if (selected) {
      settings.value.screenshot_dir = selected as string;
    }
  } catch (e) {
    console.error("选择截图目录失败:", e);
  }
}

/**
 * 在文件管理器中打开截图根目录。
 * 先落盘一次设置：目录是防抖自动保存的，用户刚改完路径就点打开时
 * 可能还没保存到后端，不先保存会打开到旧目录。
 */
async function handleOpenScreenshotDir() {
  try {
    // 先落盘截图目录：它是防抖自动保存的，用户刚改完路径就点打开时可能还没保存，
    // 不先保存会打开到旧目录。partial 只写目录，不会带走未确认的手动字段。
    await api.saveSettingsPartial({ screenshot_dir: settings.value.screenshot_dir });
    await api.openScreenshotDir();
  } catch (e) {
    console.error("打开截图目录失败:", e);
    message.error("打开截图目录失败: " + (e as Error).toString());
  }
}

// ==================== 截图快捷键录制 ====================
const recordingHotkey = ref(false);

function startRecordingHotkey() {
  recordingHotkey.value = true;
}

function cancelRecordingHotkey() {
  recordingHotkey.value = false;
}

// 把 KeyboardEvent.key 转成 global-hotkey 能解析的键名（与后端 parse_key 约定一致）
function normalizeHotkeyKey(key: string): string | null {
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(key)) return key; // F1-F24
  if (/^[a-zA-Z]$/.test(key)) return key.toUpperCase();
  if (/^[0-9]$/.test(key)) return key;
  const arrowMap: Record<string, string> = {
    ArrowUp: "Up",
    ArrowDown: "Down",
    ArrowLeft: "Left",
    ArrowRight: "Right",
  };
  if (arrowMap[key]) return arrowMap[key];
  if (key === " ") return "Space";
  const specialMap: Record<string, string> = {
    Escape: "Esc",
    Enter: "Enter",
    Backspace: "Backspace",
    Delete: "Delete",
    Tab: "Tab",
    Home: "Home",
    End: "End",
    PageUp: "PageUp",
    PageDown: "PageDown",
    Insert: "Insert",
    PrintScreen: "PrintScreen",
    ScrollLock: "ScrollLock",
    Pause: "Pause",
    CapsLock: "CapsLock",
    NumLock: "NumLock",
    "-": "Minus",
    "=": "Equal",
    "[": "BracketLeft",
    "]": "BracketRight",
    "\\": "Backslash",
    ";": "Semicolon",
    "'": "Quote",
    ",": "Comma",
    ".": "Period",
    "/": "Slash",
    "`": "Backquote",
  };
  if (specialMap[key]) return specialMap[key];
  return null;
}

function onGlobalHotkeyKeydown(e: KeyboardEvent) {
  if (!recordingHotkey.value) return;
  e.preventDefault();
  e.stopPropagation();

  // Esc 取消录制
  if (e.key === "Escape") {
    recordingHotkey.value = false;
    return;
  }
  // Backspace / Delete 清空快捷键
  if (e.key === "Backspace" || e.key === "Delete") {
    settings.value.screenshot_hotkey = "";
    recordingHotkey.value = false;
    void applyHotkeyChange();
    return;
  }
  // 纯修饰键：等待主键
  if (e.key === "Control" || e.key === "Shift" || e.key === "Alt" || e.key === "Meta") {
    return;
  }

  const key = normalizeHotkeyKey(e.key);
  if (!key) {
    recordingHotkey.value = false;
    return;
  }

  const mods: string[] = [];
  if (e.ctrlKey) mods.push("Ctrl");
  if (e.shiftKey) mods.push("Shift");
  if (e.altKey) mods.push("Alt");
  if (e.metaKey) mods.push("Super"); // Windows 上 metaKey = Win 键

  settings.value.screenshot_hotkey = [...mods, key].join("+");
  recordingHotkey.value = false;
  void applyHotkeyChange();
}

// 快捷键变更后立即保存并重新注册全局热键，同时检测冲突。
// 走 partial（只存快捷键字段），后端据此触发重注册，不带走手动字段。
async function applyHotkeyChange() {
  try {
    await api.saveSettingsPartial({ screenshot_hotkey: settings.value.screenshot_hotkey });
    message.success(
      settings.value.screenshot_hotkey
        ? `截图快捷键已设为 ${settings.value.screenshot_hotkey}`
        : "已清空截图快捷键"
    );
    const err = await api.getScreenshotHotkeyStatus();
    if (err) {
      message.error(
        `该快捷键注册失败：${err}。可能被其他程序（如 Steam）占用，请换一个。`,
        { duration: 8000 }
      );
    }
  } catch (e) {
    console.error("保存快捷键失败:", e);
    message.error("保存快捷键失败");
  }
}

onMounted(() => {
  window.addEventListener("keydown", onGlobalHotkeyKeydown);
  loadSettings();
  loadAutostartState();
});

// 组件卸载时清理定时器与按键监听
onUnmounted(() => {
  window.removeEventListener("keydown", onGlobalHotkeyKeydown);
  if (autoSaveTimer) {
    clearTimeout(autoSaveTimer);
    autoSaveTimer = null;
  }
  if (screenshotDirSaveTimer) {
    clearTimeout(screenshotDirSaveTimer);
    screenshotDirSaveTimer = null;
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
    <!-- 吸顶操作栏：保存按钮常驻顶部，不用滚到数据管理上方才能找到 -->
    <div class="settings-header" :class="{ 'is-dirty': dirty }">
      <div class="header-title-row">
        <h2 class="header-title">设置</h2>
        <span v-if="dirty" class="dirty-badge">有未保存的修改</span>
      </div>
      <n-button type="primary" size="large" :loading="saving" @click="saveSettings">
        <template #icon>
          <n-icon :component="SaveOutline" />
        </template>
        保存设置
      </n-button>
    </div>

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

    <!-- 截图设置 -->
    <n-card title="截图设置" style="margin-bottom: 16px">
      <n-form label-placement="left" label-width="140">
        <n-form-item label="保存目录">
          <n-space align="center" style="width: 100%">
            <n-input
              v-model:value="settings.screenshot_dir"
              placeholder="%USERPROFILE%\Videos"
              style="flex: 1; min-width: 320px"
            />
            <n-button size="small" @click="handleChooseScreenshotDir">
              浏览
            </n-button>
            <n-button size="small" @click="handleOpenScreenshotDir">
              <template #icon>
                <n-icon :component="FolderOpenOutline" />
              </template>
              打开
            </n-button>
          </n-space>
        </n-form-item>
        <n-form-item label="截图快捷键">
          <n-space align="center">
            <n-input
              :value="settings.screenshot_hotkey"
              readonly
              :placeholder="recordingHotkey ? '请按下快捷键…' : 'F12'"
              :status="recordingHotkey ? 'warning' : undefined"
              style="width: 220px"
              @click="startRecordingHotkey"
              @blur="cancelRecordingHotkey"
            />
            <span
              :style="{
                fontSize: '12px',
                color: recordingHotkey ? '#f0a020' : '#888',
                lineHeight: '1.6',
              }"
            >
              {{
                recordingHotkey
                  ? '按下要设置的键（Esc 取消，Backspace 清空）'
                  : '点击输入框后直接按键即可录制'
              }}
            </span>
          </n-space>
        </n-form-item>
        <n-form-item label=" ">
          <span style="font-size: 12px; color: #888; line-height: 1.6">
            保存目录与快捷键修改后立即生效。截图仅对「从本库启动且正在前台运行」的游戏生效；
            文件按「进程名\进程名_日期_时间.png」归档到保存目录下对应游戏文件夹
          </span>
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

    <!-- 数据管理（保存按钮已上移至顶部吸顶栏） -->
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
  max-width: 960px;
  margin: 0 auto;
}

/* 吸顶操作栏：随内容区居中（不再通栏铺满），与下方卡片同宽对齐 */
.settings-header {
  position: sticky;
  top: 0;
  z-index: 20;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  margin-bottom: 20px;
  padding: 16px 20px;
  border-radius: 10px;
  /* 半透明 + 毛玻璃：滚动时内容从下方穿过，仍保持标题与保存按钮可读 */
  background: rgba(22, 33, 62, 0.85);
  backdrop-filter: blur(12px);
  border: 1px solid rgba(255, 255, 255, 0.08);
  transition: border-color 0.2s, box-shadow 0.2s;
}

/* 有未保存改动时给顶栏一点视觉重量，配合右侧徽标提醒 */
.settings-header.is-dirty {
  border-color: var(--accent-color, #6366f1);
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.25);
}

.header-title-row {
  display: flex;
  align-items: baseline;
  gap: 12px;
  min-width: 0;
}

.header-title {
  margin: 0;
  font-size: 20px;
  font-weight: 600;
}

.dirty-badge {
  flex-shrink: 0;
  padding: 2px 10px;
  border-radius: 10px;
  font-size: 12px;
  color: var(--accent-color, #6366f1);
  /* 前两行是 color-mix 不可用时的降级（旧版 WebView2） */
  background: rgba(99, 102, 241, 0.18);
  border: 1px solid rgba(99, 102, 241, 0.4);
  background: color-mix(in srgb, var(--accent-color, #6366f1) 18%, transparent);
  border: 1px solid color-mix(in srgb, var(--accent-color, #6366f1) 40%, transparent);
  white-space: nowrap;
}

/* 亮色主题适配 */
.light-theme .settings-header {
  background: rgba(240, 240, 240, 0.88);
  border-color: rgba(0, 0, 0, 0.1);
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
