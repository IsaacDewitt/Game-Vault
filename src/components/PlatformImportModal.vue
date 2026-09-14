<template>
  <n-modal
    :show="show"
    preset="card"
    :title="`从 ${title} 导入游戏`"
    class="platform-import-modal"
    :closable="true"
    @close="handleClose"
  >
    <!-- 工具栏：搜索 + 仅看未添加 + 重新扫描 -->
    <div class="import-toolbar">
      <n-input
        v-model:value="search"
        placeholder="搜索游戏名称或安装路径"
        clearable
        size="small"
        style="flex: 1"
      />
      <n-checkbox v-model:checked="hideAdded" size="small">
        仅看未添加
      </n-checkbox>
      <n-button size="small" :loading="loading" @click="scan">
        重新扫描
      </n-button>
    </div>

    <!-- 扫描中 -->
    <div v-if="loading" class="import-center">
      <n-spin size="medium" />
      <p>正在扫描本机 {{ title }} 游戏库…</p>
    </div>

    <!-- 空状态 -->
    <n-empty
      v-else-if="visibleGames.length === 0"
      :description="emptyText"
      style="padding: 40px 0"
    />

    <!-- 候选列表 -->
    <div v-else class="import-list">
      <div
        v-for="g in visibleGames"
        :key="g.platform_id"
        class="import-item"
        :class="{ 'is-added': g.already_added, 'is-selected': isSelected(g) }"
        @click="toggle(g)"
      >
        <n-checkbox
          :checked="isSelected(g)"
          :disabled="g.already_added"
          @click.stop="toggle(g)"
        />
        <div class="import-info">
          <div class="import-name">
            <span class="import-name-text" :title="g.name">{{ g.name }}</span>
            <n-tag v-if="g.already_added" size="tiny" type="success">已在库</n-tag>
            <n-tag v-else-if="g.same_name_in_library" size="tiny" type="warning">
              库中有同名
            </n-tag>
          </div>
          <div class="import-path" :title="g.install_path">{{ g.install_path }}</div>
        </div>
        <div class="import-size">{{ formatSize(g.size_bytes) }}</div>
      </div>
    </div>

    <template #footer>
      <n-space justify="space-between" align="center" style="width: 100%">
        <span class="import-hint">
          共 {{ games.length }} 个 · 已选 {{ selected.length }} 个
        </span>
        <n-space>
          <n-button @click="handleClose">取消</n-button>
          <n-button
            type="primary"
            :disabled="selected.length === 0"
            :loading="importing"
            @click="doImport"
          >
            导入所选（{{ selected.length }}）
          </n-button>
        </n-space>
      </n-space>
    </template>
  </n-modal>
</template>

<script setup lang="ts">
import { ref, computed, watch } from "vue";
import {
  NModal,
  NInput,
  NCheckbox,
  NButton,
  NSpace,
  NSpin,
  NEmpty,
  NTag,
  useMessage,
} from "naive-ui";
import * as api from "../lib/tauri";

/**
 * 平台游戏导入弹窗（Steam / Epic）
 *
 * 扫描本机既有清单（Epic manifest / Steam acf），列出候选供勾选入库。
 * 入库后这些游戏与本地游戏共享同一套库与统计，仅在 platform 字段上区分。
 */
const props = defineProps<{
  show: boolean;
  /** "steam" | "epic" */
  platform: string;
}>();

const emit = defineEmits<{
  (e: "close"): void;
  (e: "imported"): void;
}>();

const message = useMessage();

const games = ref<api.PlatformGame[]>([]);
const loading = ref(false);
const importing = ref(false);
const search = ref("");
/** 默认只看未添加：老爷 Steam 有 59 个游戏，一上来就过滤掉已在库的更实用 */
const hideAdded = ref(true);
const selectedIds = ref<Set<string>>(new Set());

const title = computed(() => (props.platform === "steam" ? "Steam" : "Epic"));

const visibleGames = computed(() => {
  const kw = search.value.trim().toLowerCase();
  return games.value.filter((g) => {
    if (hideAdded.value && g.already_added) return false;
    if (!kw) return true;
    return (
      g.name.toLowerCase().includes(kw) ||
      g.install_path.toLowerCase().includes(kw)
    );
  });
});

const selected = computed(() =>
  games.value.filter((g) => selectedIds.value.has(g.platform_id))
);

const emptyText = computed(() =>
  games.value.length === 0
    ? `未在本机找到已安装的 ${title.value} 游戏`
    : "没有符合条件的游戏"
);

function isSelected(g: api.PlatformGame) {
  return selectedIds.value.has(g.platform_id);
}

function toggle(g: api.PlatformGame) {
  if (g.already_added) return;
  const next = new Set(selectedIds.value);
  if (next.has(g.platform_id)) next.delete(g.platform_id);
  else next.add(g.platform_id);
  selectedIds.value = next;
}

async function scan() {
  loading.value = true;
  try {
    games.value = await api.scanPlatformGames(props.platform);
    selectedIds.value = new Set();
  } catch (e) {
    message.error(`扫描失败：${e instanceof Error ? e.message : String(e)}`);
    games.value = [];
  } finally {
    loading.value = false;
  }
}

async function doImport() {
  const items = selected.value;
  if (items.length === 0) return;

  importing.value = true;
  try {
    const r = await api.importPlatformGames(items);
    const parts = [`成功导入 ${r.imported} 个`];
    if (r.reclaimed > 0) parts.push(`其中 ${r.reclaimed} 个续接了历史统计`);
    if (r.skipped > 0) parts.push(`跳过 ${r.skipped} 个（已在库）`);
    if (r.failed > 0) parts.push(`失败 ${r.failed} 个`);
    message.success(parts.join("，"));

    if (r.errors.length > 0) {
      console.warn("平台导入部分失败：", r.errors);
    }

    emit("imported");
    // 重新扫描以刷新「已在库」标记
    await scan();
  } catch (e) {
    message.error(`导入失败：${e instanceof Error ? e.message : String(e)}`);
  } finally {
    importing.value = false;
  }
}

function formatSize(bytes: number | null) {
  if (!bytes || bytes <= 0) return "";
  const gb = bytes / 1024 / 1024 / 1024;
  return gb >= 1 ? `${gb.toFixed(1)} GB` : `${(bytes / 1024 / 1024).toFixed(0)} MB`;
}

function handleClose() {
  emit("close");
}

// 每次打开时重置状态并自动扫描
watch(
  () => props.show,
  (visible) => {
    if (visible) {
      search.value = "";
      selectedIds.value = new Set();
      scan();
    }
  }
);
</script>

<style scoped>
.import-toolbar {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 12px;
}

.import-center {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 48px 0;
  gap: 12px;
  color: #888;
  font-size: 13px;
}

.import-list {
  max-height: 420px;
  overflow-y: auto;
  border: 1px solid rgba(128, 128, 128, 0.2);
  border-radius: 6px;
}

.import-item {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 9px 12px;
  cursor: pointer;
  border-bottom: 1px solid rgba(128, 128, 128, 0.12);
  transition: background-color 0.15s;
}

.import-item:last-child {
  border-bottom: none;
}

.import-item:hover {
  background-color: rgba(128, 128, 128, 0.08);
}

.import-item.is-selected {
  background-color: rgba(99, 102, 241, 0.12);
}

.import-item.is-added {
  cursor: default;
  opacity: 0.55;
}

.import-info {
  flex: 1;
  min-width: 0;
}

.import-name {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  font-weight: 500;
  margin-bottom: 2px;
}

.import-name-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.import-path {
  font-size: 11px;
  color: #888;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.import-size {
  font-size: 11px;
  color: #999;
  flex-shrink: 0;
  min-width: 60px;
  text-align: right;
}

.import-hint {
  font-size: 12px;
  color: #888;
}
</style>

<!-- 非 scoped：弹窗被 teleport 到 body，scoped 选择器无法命中内部卡片 -->
<style>
/*
 * 弹窗宽度：Modal 会把 class 透传到内部卡片 .n-modal 上（实测 class 与 .n-modal 同属一个元素，
 * 故必须用复合选择器而非后代选择器），在此处显式定宽——卡片默认按内容撑满全宽。
 * 刻意不用 `style="width"`：该属性在 naive-ui 里语义属于遮罩容器（overlayStyle 的替代），
 * 用来当"弹窗宽度"用会与遮罩层样式混淆，命名误导。
 */
.n-modal.platform-import-modal {
  width: 760px;
  max-width: 92vw;
}
</style>
