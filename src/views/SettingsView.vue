<script setup lang="ts">
import { ref, onMounted, watch } from "vue";
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
  useMessage,
} from "naive-ui";
import { DownloadOutline } from "@vicons/ionicons5";
import { save } from "@tauri-apps/plugin-dialog";
import { writeTextFile } from "@tauri-apps/plugin-fs";
import * as api from "../lib/tauri";
import type { Settings } from "../lib/tauri";

const message = useMessage();

// 默认值仅用于表单初始显示，实际默认值从后端 API 获取
const settings = ref<Settings>({
  theme: "dark",
  language: "zh-CN",
  steamgriddb_api_key: "",
  llm_provider: "",
  llm_protocol: "",
  llm_api_key: "",
  llm_base_url: "",
  llm_model: "",
  llm_enabled: false,
});

// LLM 提供商预设
const providerOptions = [
  { label: "小米 MiMo", value: "xiaomi" },
  { label: "DeepSeek", value: "deepseek" },
];

const protocolOptions = [
  { label: "OpenAI 格式", value: "openai" },
  { label: "Anthropic 格式", value: "anthropic" },
];

// 切换提供商时自动填充默认值
watch(() => settings.value.llm_provider, (provider) => {
  if (provider === "xiaomi") {
    if (!settings.value.llm_base_url || settings.value.llm_base_url === "https://api.deepseek.com/v1") {
      settings.value.llm_base_url = "https://api.xiaomimimo.com/v1";
    }
    if (!settings.value.llm_model || settings.value.llm_model === "deepseek-chat") {
      settings.value.llm_model = "mimo-v2.5-pro";
    }
    settings.value.llm_protocol = "openai";
  } else if (provider === "deepseek") {
    if (!settings.value.llm_base_url || settings.value.llm_base_url === "https://api.xiaomimimo.com/v1") {
      settings.value.llm_base_url = "https://api.deepseek.com/v1";
    }
    if (!settings.value.llm_model || settings.value.llm_model === "mimo-v2.5-pro") {
      settings.value.llm_model = "deepseek-chat";
    }
  }
});

const saving = ref(false);
const exporting = ref(false);

async function loadSettings() {
  try {
    settings.value = await api.getSettings();
  } catch (e) {
    console.error("加载设置失败:", e);
  }
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

onMounted(loadSettings);
</script>

<template>
  <div class="settings-view">
    <h2 style="margin-bottom: 24px">设置</h2>

    <!-- 基本设置 -->
    <n-card title="基本设置" style="margin-bottom: 16px">
      <n-form label-placement="left" label-width="140">
        <n-form-item label="SteamGridDB API Key">
          <n-input
            v-model:value="settings.steamgriddb_api_key"
            placeholder="可选，用于自动获取封面图"
            type="password"
          />
        </n-form-item>
      </n-form>
    </n-card>

    <!-- LLM 游戏信息获取 -->
    <n-card title="LLM 游戏信息获取" style="margin-bottom: 16px">
      <template #header-extra>
        <n-switch v-model:value="settings.llm_enabled" />
      </template>
      <n-form label-placement="left" label-width="140">
        <n-form-item label="提供商">
          <n-select
            v-model:value="settings.llm_provider"
            :options="providerOptions"
            style="width: 200px"
          />
        </n-form-item>
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
            placeholder="输入 API Key"
            type="password"
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
      </n-form>
    </n-card>
  </div>
</template>

<style scoped>
.settings-view {
  max-width: 800px;
}
</style>
