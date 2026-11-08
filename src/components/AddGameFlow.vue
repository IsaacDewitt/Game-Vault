<script setup lang="ts">
import { NButton, NInput, NModal, NSpace, useMessage } from "naive-ui";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { useGamesStore } from "../stores/games";
import { resetAddGameFlow, useAddGameFlow } from "../lib/addGameFlow";
import PlatformImportModal from "./PlatformImportModal.vue";

/**
 * 「添加游戏」弹窗宿主。
 *
 * 挂在 `App.vue`（不是游戏库视图）——入口在侧边栏左下角，属于全局动作，
 * 在任何页面都能唤起；状态在 `lib/addGameFlow.ts` 的模块单例里，
 * 游戏库空状态按钮与右键菜单通过 `openAddGame()` 唤起同一套流程。
 */
const store = useGamesStore();
const message = useMessage();
const { showPicker, showName, showImport, pendingExePath, gameNameInput, importPlatform } =
  useAddGameFlow();

/** 来源选「本地游戏」→ 走原有的选择 exe 流程 */
function handlePickLocalExe() {
  showPicker.value = false;
  pickExeFile();
}

/** 来源选 Steam / Epic → 打开平台导入弹窗 */
function handlePickPlatform(platform: string) {
  showPicker.value = false;
  importPlatform.value = platform;
  showImport.value = true;
}

/** 选择本地游戏的 exe 文件（原「添加游戏」流程） */
async function pickExeFile() {
  try {
    const selected = await openFileDialog({
      multiple: false,
      title: "选择游戏程序",
      filters: [
        {
          name: "可执行文件",
          extensions: ["exe"],
        },
      ],
    });
    if (selected) {
      const exePath = selected as string;
      // 从 exe 路径提取文件名（不含扩展名）作为默认游戏名
      const fileName = exePath.split(/[/\\]/).pop() || "";
      const defaultName = fileName.replace(/\.exe$/i, "");
      pendingExePath.value = exePath;
      gameNameInput.value = defaultName;
      showName.value = true;
    }
  } catch (e) {
    console.error(e);
    message.error("选择文件失败");
  }
}

async function handleConfirmAddGame() {
  const name = gameNameInput.value.trim();
  if (!name) {
    message.warning("请输入游戏名称");
    return;
  }
  try {
    await store.addGameManual(name, pendingExePath.value);
    message.success(`已添加游戏: ${name}`);
    showName.value = false;
    pendingExePath.value = "";
    gameNameInput.value = "";
  } catch (e) {
    message.error("添加游戏失败");
  }
}

function handleCancelAddGame() {
  showName.value = false;
  pendingExePath.value = "";
  gameNameInput.value = "";
}

/** 平台导入完成后刷新列表（导入是后端批量入库，前端需重新拉取） */
async function handlePlatformImported() {
  try {
    await store.loadGames();
  } catch (e) {
    console.error("刷新游戏列表失败:", e);
  }
}

function closePicker() {
  resetAddGameFlow();
}
</script>

<template>
  <!-- 游戏来源选择弹窗：本地 / Steam / Epic -->
  <n-modal
    :show="showPicker"
    preset="card"
    title="添加游戏"
    class="source-picker-modal"
    :closable="true"
    @close="closePicker"
  >
    <p style="margin-bottom: 14px; color: #999; font-size: 13px">请选择游戏来源：</p>
    <div class="source-list">
      <button class="source-item" @click="handlePickLocalExe()">
        <span class="source-icon">🖥️</span>
        <span class="source-text">
          <span class="source-title">本地游戏</span>
          <span class="source-desc">手动选择游戏 exe，由本应用直接启动</span>
        </span>
      </button>
      <button class="source-item" @click="handlePickPlatform('steam')">
        <span class="source-icon">🎮</span>
        <span class="source-text">
          <span class="source-title">Steam 游戏</span>
          <span class="source-desc">扫描本机 Steam 库，交由 Steam 启动并记录时长</span>
        </span>
      </button>
      <button class="source-item" @click="handlePickPlatform('epic')">
        <span class="source-icon">🕹️</span>
        <span class="source-text">
          <span class="source-title">Epic 游戏</span>
          <span class="source-desc">扫描本机 Epic 库，交由 Epic 启动、记录时长并支持截图</span>
        </span>
      </button>
    </div>
  </n-modal>

  <!-- 输入游戏名称弹窗 -->
  <n-modal
    :show="showName"
    preset="card"
    title="添加游戏"
    style="width: 450px"
    :closable="true"
    @close="handleCancelAddGame()"
  >
    <p style="margin-bottom: 12px; color: #999">请输入游戏名称：</p>
    <n-input
      v-model:value="gameNameInput"
      placeholder="游戏名称"
      @keyup.enter="handleConfirmAddGame()"
    />
    <template #footer>
      <n-space justify="end">
        <n-button @click="handleCancelAddGame()">取消</n-button>
        <n-button type="primary" @click="handleConfirmAddGame()">确认添加</n-button>
      </n-space>
    </template>
  </n-modal>

  <!-- 平台游戏导入弹窗 -->
  <PlatformImportModal
    :show="showImport"
    :platform="importPlatform"
    @close="showImport = false"
    @imported="handlePlatformImported()"
  />
</template>

<!-- 非 scoped：弹窗被 teleport 到 body，scoped 选择器无法命中内部卡片 -->
<style>
/* 「添加游戏」来源选择弹窗宽度：Modal 会把 class 透传到内部卡片 .n-modal 上
   （与 .n-modal 同属一个元素，故此处用复合选择器）。卡片默认按内容撑满全宽，须显式定宽。 */
.n-modal.source-picker-modal {
  width: 440px;
  max-width: 92vw;
}
</style>

<style scoped>
/* 游戏来源选择弹窗 */
.source-list {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.source-item {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 12px 14px;
  text-align: left;
  background: rgba(128, 128, 128, 0.08);
  border: 1px solid rgba(128, 128, 128, 0.18);
  border-radius: 8px;
  cursor: pointer;
  transition: all 0.15s;
  font-family: inherit;
  color: inherit;
}

.source-item:hover {
  border-color: #6366f1;
  background: rgba(99, 102, 241, 0.1);
}

.source-icon {
  font-size: 22px;
  line-height: 1;
  flex-shrink: 0;
}

.source-text {
  display: flex;
  flex-direction: column;
  gap: 3px;
  min-width: 0;
}

.source-title {
  font-size: 14px;
  font-weight: 600;
}

.source-desc {
  font-size: 11.5px;
  color: #888;
  line-height: 1.4;
}
</style>
