<script setup lang="ts">
import { computed, ref, watch } from "vue";
import {
  NModal,
  NButton,
  NIcon,
  NInput,
  NInputNumber,
  NDatePicker,
  NSelect,
  NSpace,
  useMessage,
} from "naive-ui";
import { CreateOutline, AddOutline, TrashOutline } from "@vicons/ionicons5";
import type { Game } from "../lib/tauri";
import type { SelectOption } from "naive-ui";
import { useGamesStore } from "../stores/games";
import { GENRE_GROUPS } from "../lib/constants";

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

// 类型候选：从规范表里选，避免手打又冒出「Action」这类新写法。
// 仍保留自由输入（tag），输入的值会由后端归一化（命中别名即转规范名）。
const genreOptions: SelectOption[] = GENRE_GROUPS.map((g) => ({
  type: "group",
  label: g.group,
  key: g.group,
  children: g.genres.map((name) => ({ label: name, value: name })),
}));

// 表单字段
const description = ref("");
const developer = ref("");
const publisher = ref("");
const releaseDate = ref<number | null>(null);
const genres = ref<string[]>([]);
const hltbMainStory = ref<number | null>(null);
const hltbMainExtra = ref<number | null>(null);
const hltbCompletionist = ref<number | null>(null);
const savePaths = ref<string[]>([]);
const launchArgs = ref("");

// 启动参数仅对本地游戏有意义：Steam/Epic 由客户端自己拉起并套用它自身的启动选项，
// 我们塞参数过去也不会被转交（见 core/launcher.rs 的平台分流注释）
const isLocalGame = computed(() => (props.game.platform ?? "local") === "local");

// 从 game 初始化表单
function initForm() {
  description.value = props.game.description ?? "";
  developer.value = props.game.developer ?? "";
  publisher.value = props.game.publisher ?? "";
  // 将 YYYY-MM-DD 字符串转为时间戳
  if (props.game.release_date) {
    const d = new Date(props.game.release_date);
    releaseDate.value = isNaN(d.getTime()) ? null : d.getTime();
  } else {
    releaseDate.value = null;
  }
  genres.value = [...(props.game.genres ?? [])];
  hltbMainStory.value = props.game.hltb_main_story ?? null;
  hltbMainExtra.value = props.game.hltb_main_extra ?? null;
  hltbCompletionist.value = props.game.hltb_completionist ?? null;
  savePaths.value = [...(props.game.save_paths ?? [])];
  launchArgs.value = props.game.launch_args ?? "";
}

watch(() => props.show, (val) => {
  if (val) initForm();
});

// 新增存档路径
function addSavePath() {
  savePaths.value.push("");
}

// 删除存档路径
function removeSavePath(index: number) {
  savePaths.value.splice(index, 1);
}

// 格式化日期为 YYYY-MM-DD
function formatTimestamp(ts: number | null): string | null {
  if (ts === null) return null;
  const d = new Date(ts);
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

async function handleSave() {
  saving.value = true;
  try {
    // 键名必须 camelCase：Tauri 按 camelCase 查键，snake_case 会被静默当成"没提交"
    // （见 lib/tauri.ts 里 GameMetaInput 的说明——0.8.3 之前这里一直是写不进去的）
    //
    // 清空一律用哨兵值、绝不传 null：Tauri 会把 JSON null 折叠成"没提交"，
    // 清空会静默失效（0.8.5 修复——此前的 `|| null` 写法把后端的空串清空分支全堵死了）。
    // 哨兵口径：文本 = 空串 / genres = 空数组 / HLTB = 0（数字框清空是 null，转 0）。
    await store.updateGameMeta(props.game.id, {
      description: description.value,
      developer: developer.value,
      publisher: publisher.value,
      releaseDate: formatTimestamp(releaseDate.value) ?? "",
      genres: genres.value,
      hltbMainStory: hltbMainStory.value ?? 0,
      hltbMainExtra: hltbMainExtra.value ?? 0,
      hltbCompletionist: hltbCompletionist.value ?? 0,
      savePaths: savePaths.value.filter((p) => p.trim() !== ""),
      // 空串即清空（后端按 trim 后为空处理）
      launchArgs: launchArgs.value.trim(),
    });
    message.success("游戏信息已保存");
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
    :style="{ width: '560px' }"
    :bordered="false"
    :closable="true"
    :mask-closable="true"
    @update:show="(val: boolean) => !val && emit('close')"
  >
    <template #header>
      <div class="modal-header">
        <n-icon :size="20" :component="CreateOutline" />
        <span>手动填写游戏信息 — {{ game.name }}</span>
      </div>
    </template>

    <div class="form-body">
      <!-- 描述 -->
      <div class="form-field">
        <label>游戏描述</label>
        <n-input
          v-model:value="description"
          type="textarea"
          placeholder="游戏的简短描述（中文，100字以内）"
          :rows="3"
          :maxlength="200"
          show-count
        />
      </div>

      <!-- 开发商 / 发行商 -->
      <div class="form-row">
        <div class="form-field">
          <label>开发商</label>
          <n-input v-model:value="developer" placeholder="开发商名称" />
        </div>
        <div class="form-field">
          <label>发行商</label>
          <n-input v-model:value="publisher" placeholder="发行商名称" />
        </div>
      </div>

      <!-- 发售日期 -->
      <div class="form-field">
        <label>发售日期</label>
        <n-date-picker
          v-model:value="releaseDate"
          type="date"
          clearable
          style="width: 100%"
        />
      </div>

      <!-- 类型标签 -->
      <div class="form-field">
        <label>游戏类型</label>
        <n-select
          v-model:value="genres"
          :options="genreOptions"
          multiple
          filterable
          tag
          clearable
          placeholder="输入或选择类型，可多选"
        />
      </div>

      <!-- HLTB 时长 -->
      <div class="form-section-label">HowLongToBeat 时长（分钟）</div>
      <div class="form-row form-row-3">
        <div class="form-field">
          <label>主线</label>
          <!-- min=1：0 被后端用作"清空"哨兵（清空的数字框传 0），不允许手输 0 -->
          <n-input-number
            v-model:value="hltbMainStory"
            :min="1"
            placeholder="分钟"
            clearable
          />
        </div>
        <div class="form-field">
          <label>主线+支线</label>
          <n-input-number
            v-model:value="hltbMainExtra"
            :min="1"
            placeholder="分钟"
            clearable
          />
        </div>
        <div class="form-field">
          <label>完美通关</label>
          <n-input-number
            v-model:value="hltbCompletionist"
            :min="1"
            placeholder="分钟"
            clearable
          />
        </div>
      </div>

      <!-- 存档路径 -->
      <div class="form-field">
        <label>存档路径</label>
        <div v-for="(_path, index) in savePaths" :key="index" class="save-path-row">
          <n-input
            v-model:value="savePaths[index]"
            placeholder="存档路径（支持 %%USERPROFILE%% 等环境变量）"
            size="small"
          />
          <n-button
            text
            type="error"
            size="small"
            @click="removeSavePath(index)"
          >
            <template #icon><n-icon :component="TrashOutline" /></template>
          </n-button>
        </div>
        <n-button text type="primary" size="small" @click="addSavePath">
          <template #icon><n-icon :component="AddOutline" /></template>
          添加路径
        </n-button>
      </div>

      <!-- 启动参数（仅本地游戏；平台游戏由客户端掌管启动选项） -->
      <div v-if="isLocalGame" class="form-field">
        <label>启动参数</label>
        <n-input
          v-model:value="launchArgs"
          placeholder="例如 -savetouserdir"
          clearable
        />
        <div class="form-hint">
          原样追加到 exe 之后，等价于写进 bat 启动器（如《寂静岭 f》需要
          <code>-savetouserdir</code>）。未经 cmd.exe，故环境变量
          <code>%USERPROFILE%</code> 不会展开，需要路径请写绝对路径。
        </div>
      </div>
    </div>

    <template #footer>
      <n-space justify="end">
        <n-button @click="emit('close')">取消</n-button>
        <n-button type="primary" :loading="saving" @click="handleSave">
          保存
        </n-button>
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
  gap: 16px;
  max-height: 60vh;
  overflow-y: auto;
  padding-right: 4px;
}

.form-field {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.form-field label {
  font-size: 13px;
  color: rgba(255, 255, 255, 0.7);
  font-weight: 500;
}

.form-row {
  display: flex;
  gap: 12px;
}

.form-row > .form-field {
  flex: 1;
}

.form-row-3 > .form-field {
  flex: 1;
}

.form-section-label {
  font-size: 13px;
  color: rgba(255, 255, 255, 0.5);
  font-weight: 500;
  margin-top: 4px;
}

.save-path-row {
  display: flex;
  gap: 6px;
  align-items: center;
  margin-bottom: 6px;
}

.save-path-row .n-input {
  flex: 1;
}

.form-hint {
  font-size: 12px;
  line-height: 1.5;
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
