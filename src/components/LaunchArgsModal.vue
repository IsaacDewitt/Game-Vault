<script setup lang="ts">
import { ref, watch } from "vue";
import { NModal, NInput, NButton, NSpace, useMessage } from "naive-ui";
import type { Game } from "../lib/tauri";
import { useGamesStore } from "../stores/games";

/**
 * 启动参数编辑弹窗（0.8.3）
 *
 * 两个入口共用：卡片右键菜单「设置启动参数」与详情面板启动参数行右侧的编辑图标。
 * 仅本地游戏可达（平台游戏的入口在调用侧就不渲染——参数会被客户端忽略，填了是误导）。
 */
const props = defineProps<{
  show: boolean;
  game: Game;
}>();

const emit = defineEmits<{
  close: [];
  saved: [];
}>();

const store = useGamesStore();
const message = useMessage();
const saving = ref(false);
const args = ref("");

watch(
  () => props.show,
  (visible) => {
    if (visible) args.value = props.game.launch_args ?? "";
  }
);

async function handleSave() {
  saving.value = true;
  const trimmed = args.value.trim();
  try {
    // 键名必须是 camelCase（Tauri 用 camelCase 查键，snake_case 会被静默丢弃）
    await store.updateGameMeta(props.game.id, { launchArgs: trimmed });
    message.success(trimmed ? "启动参数已保存" : "启动参数已清空");
    emit("saved");
    emit("close");
  } catch (e) {
    message.error("保存失败: " + (e as Error).toString());
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <n-modal
    :show="show"
    preset="card"
    title=""
    :style="{ width: '540px' }"
    :bordered="false"
    :closable="true"
    :mask-closable="true"
    @update:show="(val: boolean) => !val && emit('close')"
  >
    <template #header>
      <div class="modal-header">
        <span>启动参数 — {{ game.name }}</span>
      </div>
    </template>

    <div class="form-body">
      <n-input
        v-model:value="args"
        placeholder="例如 -savetouserdir"
        clearable
        @keyup.enter="handleSave"
      />
      <div class="form-hint">
        原样追加到游戏 exe 之后，等价于写进 bat 启动器（如《寂静岭 f》需要
        <code>-savetouserdir</code>）。未经 cmd.exe，故环境变量
        <code>%USERPROFILE%</code> 不会展开，需要路径请写绝对路径；留空即清除。
      </div>
    </div>

    <template #footer>
      <n-space justify="end">
        <n-button @click="emit('close')">取消</n-button>
        <n-button type="primary" :loading="saving" @click="handleSave">保存</n-button>
      </n-space>
    </template>
  </n-modal>
</template>

<style scoped>
.modal-header {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 16px;
  font-weight: 600;
}

.form-body {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.form-hint {
  font-size: 12px;
  line-height: 1.6;
  color: rgba(255, 255, 255, 0.45);
}

.form-hint code {
  font-family: Consolas, Monaco, monospace;
  font-size: 11px;
  padding: 1px 4px;
  border-radius: 3px;
  background: rgba(255, 255, 255, 0.08);
  color: rgba(255, 255, 255, 0.7);
}
</style>
