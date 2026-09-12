<script setup lang="ts">
import { ref, computed, onMounted, h } from "vue";
import {
  NInput,
  NButton,
  NButtonGroup,
  NSpace,
  NIcon,
  NSpin,
  NEmpty,
  NModal,
  NDrawer,
  NDrawerContent,
  NInputNumber,
  NSelect,
  NTabs,
  NTabPane,
  NGrid,
  NGi,
  NTag,
  NText,
  useMessage,
  useDialog,
} from "naive-ui";
import {
  SearchOutline,
  AddOutline,
  StarOutline,
  RefreshOutline,
  TrashOutline,
  ImageOutline,
  ImagesOutline,
  FolderOpenOutline,
  CreateOutline,
  GameControllerOutline,
} from "@vicons/ionicons5";
import { useDebounceFn } from "@vueuse/core";
import { useReviewsStore, displayName } from "../stores/reviews";
import { useGamesStore } from "../stores/games";
import { getScreenshotDir, openScreenshotDir, listScreenshotDirs } from "../lib/tauri";
import type { CoverOption, ScreenshotDirInfo, ScreenshotDirOption } from "../lib/tauri";
import { DEBOUNCE_MS } from "../lib/constants";
import ContextMenu from "../components/ContextMenu.vue";
import type { ContextMenuItem } from "../components/ContextMenu.vue";

const store = useReviewsStore();
const gamesStore = useGamesStore();
const message = useMessage();
const dialog = useDialog();

// ==================== 状态与筛选 ====================

const statusOptions = [
  { label: "全部", value: "" },
  { label: "想玩", value: "wishlist" },
  { label: "游玩中", value: "playing" },
  { label: "已通关", value: "completed" },
  { label: "已弃坑", value: "abandoned" },
];

const statusMeta: Record<string, { label: string; color: string }> = {
  wishlist: { label: "想玩", color: "#4098ff" },
  playing: { label: "游玩中", color: "#36ad6a" },
  completed: { label: "已通关", color: "#d4a72c" },
  abandoned: { label: "已弃坑", color: "#8a8f98" },
};

const sortOptions = [
  { label: "添加时间", value: "added_at" },
  { label: "评分", value: "rating" },
  { label: "最近更新", value: "updated_at" },
  { label: "名称", value: "name" },
];

const sortOrderOptions = [
  { label: "降序", value: "desc" },
  { label: "升序", value: "asc" },
];

const statusButtonOptions = [
  { label: "想玩", value: "wishlist" },
  { label: "游玩中", value: "playing" },
  { label: "已通关", value: "completed" },
  { label: "已弃坑", value: "abandoned" },
];

// 类型筛选选项（从已有条目动态收集）
const genreOptions = computed(() => {
  const set = new Set<string>();
  store.reviews.forEach((r) => r.genres.forEach((g) => set.add(g)));
  return [
    { label: "全部类型", value: "" },
    ...[...set].sort((a, b) => a.localeCompare(b, "zh-Hans-CN")).map((g) => ({ label: g, value: g })),
  ];
});

const onSearch = useDebounceFn(() => {
  store.searchQuery = searchInput.value.trim();
}, DEBOUNCE_MS);
const searchInput = ref("");

// ==================== 添加弹窗 ====================

const showAddModal = ref(false);
const addTab = ref<"name" | "import">("name");
const newName = ref("");
const adding = ref(false);
const importGameId = ref<string | null>(null);

// 从游戏库导入的候选（排除已在手账中的游戏）
const importOptions = computed(() =>
  gamesStore.games
    .filter((g) => !store.reviews.some((r) => r.name === g.name))
    .map((g) => ({ label: g.name, value: g.id }))
);

function openAddModal() {
  addTab.value = "name";
  newName.value = "";
  importGameId.value = null;
  showAddModal.value = true;
}

async function confirmAdd() {
  if (adding.value) return;
  adding.value = true;
  try {
    if (addTab.value === "name") {
      const name = newName.value.trim();
      if (!name) {
        message.warning("请输入游戏名称");
        return;
      }
      await store.addReview(name);
      message.success(`已添加《${name}》，正在自动获取信息…`);
    } else {
      if (!importGameId.value) {
        message.warning("请选择要导入的游戏");
        return;
      }
      await store.importFromGame(importGameId.value);
      message.success("已从游戏库导入");
    }
    showAddModal.value = false;
  } catch (e) {
    message.error(e instanceof Error ? e.message : String(e));
  } finally {
    adding.value = false;
  }
}

// ==================== 详情抽屉 ====================

const showDetail = ref(false);
const editingName = ref("");
const editingNameEn = ref("");
const ratingInput = ref<number | null>(null);
const reviewText = ref("");
const savingReview = ref(false);

const selected = computed(() => store.selectedReview);

/** 主名是否为纯 ASCII（即原名本身就是英文） */
const selectedNameIsAscii = computed(() => {
  const name = selected.value?.name ?? "";
  return name.length > 0 && /^[\x20-\x7E]+$/.test(name);
});

function openDetail(review: (typeof store.reviews)[number]) {
  store.selectReview(review);
  editingName.value = review.name;
  editingNameEn.value = review.name_en ?? "";
  ratingInput.value = review.rating;
  reviewText.value = review.review ?? "";
  loadScreenshotInfo(review.id);
  showDetail.value = true;
}

// ==================== 截图库（与游戏库详情同款：路径 + 张数 + 打开资源管理器） ====================

const screenshotInfo = ref<ScreenshotDirInfo | null>(null);
const screenshotError = ref<string | null>(null);
const openingScreenshots = ref(false);

// 请求序号：快速切换手账条目时，旧条目的响应可能后到并覆盖新条目的截图信息
let screenshotSeq = 0;

async function loadScreenshotInfo(reviewId: string) {
  const seq = ++screenshotSeq;
  screenshotInfo.value = null;
  screenshotError.value = null;
  try {
    // 后端按 手动指定目录 > 同名游戏 exe > 同名墓碑 exe 定位（手账自持，删游戏不受影响）
    const info = await getScreenshotDir(undefined, reviewId);
    if (seq !== screenshotSeq) return;
    screenshotInfo.value = info;
  } catch (e) {
    if (seq !== screenshotSeq) return;
    screenshotError.value = e instanceof Error ? e.message : String(e);
  }
}

async function handleOpenScreenshots() {
  if (!selected.value) return;
  openingScreenshots.value = true;
  try {
    await openScreenshotDir(undefined, selected.value.id);
    // 目录可能刚被创建，打开后刷新数量
    await loadScreenshotInfo(selected.value.id);
  } catch (e) {
    message.error("打开截图文件夹失败: " + (e instanceof Error ? e.message : String(e)));
  } finally {
    openingScreenshots.value = false;
  }
}

// ---- 指定截图目录（手账自持：游戏从游戏库删除后截图库照旧可用） ----

const showDirPicker = ref(false);
const dirOptions = ref<ScreenshotDirOption[]>([]);
const loadingDirOptions = ref(false);
const savingDir = ref(false);
/** 选择器当前值；null = 自动推断 */
const dirInput = ref<string | null>(null);

const dirSelectOptions = computed(() =>
  dirOptions.value.map((o) => ({ label: `${o.name}（${o.count} 张）`, value: o.name }))
);

/** 当前定位来源的中文说明 */
const sourceLabel = computed(() => {
  switch (screenshotInfo.value?.source) {
    case "manual":
      return "手动指定";
    case "game":
      return "按同名游戏匹配";
    case "tombstone":
      return "按游戏历史归档匹配";
    default:
      return "";
  }
});

async function openDirPicker() {
  if (!selected.value) return;
  dirInput.value = selected.value.screenshot_dir ?? null;
  showDirPicker.value = true;
  loadingDirOptions.value = true;
  try {
    dirOptions.value = await listScreenshotDirs();
  } catch (e) {
    dirOptions.value = [];
    message.error("读取截图目录失败: " + (e instanceof Error ? e.message : String(e)));
  } finally {
    loadingDirOptions.value = false;
  }
}

async function saveDirPicker() {
  if (!selected.value) return;
  savingDir.value = true;
  try {
    const updated = await store.setScreenshotDir(selected.value.id, dirInput.value);
    await loadScreenshotInfo(updated.id);
    message.success(dirInput.value ? `已指定截图目录：${dirInput.value}` : "已恢复自动匹配截图目录");
    showDirPicker.value = false;
  } catch (e) {
    message.error("保存截图目录失败: " + (e instanceof Error ? e.message : String(e)));
  } finally {
    savingDir.value = false;
  }
}

function closeDetail() {
  showDetail.value = false;
  store.clearSelection();
}

function coverOf(review: (typeof store.reviews)[number]): string | null {
  return store.coverSrc(review);
}

function isRefreshing(reviewId: string) {
  return store.refreshingIds.has(reviewId);
}

/** 评分：点击数字按钮即保存 */
async function handleSetRating(value: number | null) {
  if (!selected.value) return;
  try {
    await store.setRating(selected.value.id, value);
    ratingInput.value = value;
    message.success(value == null ? "已清除评分" : `已评分 ${value} 分`);
  } catch (e) {
    message.error(e instanceof Error ? e.message : String(e));
  }
}

/** 保存评价 */
async function saveReviewText() {
  if (!selected.value || savingReview.value) return;
  savingReview.value = true;
  try {
    await store.setReview(selected.value.id, reviewText.value);
    message.success("评价已保存");
  } catch (e) {
    message.error(e instanceof Error ? e.message : String(e));
  } finally {
    savingReview.value = false;
  }
}

/** 改主名称（原名）。填英文时后端会自动同步一份到 name_en */
async function saveName() {
  if (!selected.value) return;
  const name = editingName.value.trim();
  if (!name || name === selected.value.name) return;
  try {
    const updated = await store.updateMeta(selected.value.id, { name });
    // 后端可能自动补了 name_en（纯英文原名），同步回输入框，
    // 否则下次 blur 英文名框时会拿旧值覆盖回去
    editingNameEn.value = updated.name_en ?? "";
    message.success("名称已更新");
  } catch (e) {
    message.error(e instanceof Error ? e.message : String(e));
  }
}

/** 改英文名（清空即清除；SteamGridDB 封面检索用） */
async function saveNameEn() {
  if (!selected.value) return;
  const trimmed = editingNameEn.value.trim();
  const current = selected.value.name_en ?? "";
  if (trimmed === current) return;
  try {
    await store.updateMeta(selected.value.id, { name_en: trimmed });
    message.success(trimmed ? "英文名已更新，点「刷新信息」可重新拉取封面" : "英文名已清除");
  } catch (e) {
    message.error(e instanceof Error ? e.message : String(e));
  }
}

/** 状态切换 */
async function handleSetStatus(status: string) {
  if (!selected.value) return;
  try {
    await store.setStatus(selected.value.id, status);
  } catch (e) {
    message.error(e instanceof Error ? e.message : String(e));
  }
}

/** 刷新信息（LLM + 封面） */
async function handleRefreshInfo(review: (typeof store.reviews)[number]) {
  try {
    await store.refreshReviewInfo(review.id);
    message.success(`《${displayName(review)}》信息已刷新`);
  } catch (e) {
    message.error(e instanceof Error ? e.message : String(e));
  }
}

/** 删除 */
function confirmDelete(review: (typeof store.reviews)[number]) {
  const hasCover = !!(review.cover_local || review.cover_url);

  const doDelete = async (keepCover: boolean) => {
    try {
      await store.removeReview(review.id, keepCover);
      showDetail.value = false;
      message.success(keepCover ? "已删除，封面图片已留档" : "已删除");
    } catch (e) {
      message.error(e instanceof Error ? e.message : String(e));
    }
  };

  let d: ReturnType<typeof dialog.warning>;
  d = dialog.warning({
    title: "删除手账",
    content:
      `确定删除《${displayName(review)}》吗？此操作不可恢复。` +
      (hasCover ? "封面图片请选择处理方式：" : ""),
    // 删除前每次都问一次封面去留（与游戏库删除口径一致）
    action: () =>
      hasCover
        ? h(NSpace, { justify: "end" }, () => [
            h(NButton, { size: "small", onClick: () => d.destroy() }, () => "取消"),
            h(
              NButton,
              { size: "small", type: "primary", ghost: true, onClick: () => doDelete(true) },
              () => "删除，保留封面"
            ),
            h(
              NButton,
              { size: "small", type: "error", onClick: () => doDelete(false) },
              () => "删除，一并删图"
            ),
          ])
        : h(NSpace, { justify: "end" }, () => [
            h(NButton, { size: "small", onClick: () => d.destroy() }, () => "取消"),
            h(NButton, { size: "small", type: "error", onClick: () => doDelete(true) }, () => "删除"),
          ]),
  });
}

// ==================== 封面选择弹窗 ====================

const showCoverPicker = ref(false);
const coverLoading = ref(false);
const coverSelecting = ref(false);
const coverOptions = ref<CoverOption[]>([]);
const coverFilter = ref<"portrait" | "landscape" | "all">("portrait");
const coverCount = ref(20);

const filteredCovers = computed(() => {
  let result = coverOptions.value;
  if (coverFilter.value === "portrait") {
    result = result.filter((c) => c.height > c.width);
  } else if (coverFilter.value === "landscape") {
    result = result.filter((c) => c.width > c.height);
  }
  return result;
});

const displayedCovers = computed(() => filteredCovers.value.slice(0, coverCount.value));
const hasMoreCovers = computed(() => coverCount.value < filteredCovers.value.length);

async function openCoverPicker(review: (typeof store.reviews)[number]) {
  store.selectReview(review);
  showCoverPicker.value = true;
  coverOptions.value = [];
  coverFilter.value = "portrait";
  coverCount.value = 20;
  coverLoading.value = true;
  try {
    coverOptions.value = await store.fetchCoverOptions(review.id);
    if (coverOptions.value.length === 0) {
      message.warning("未找到可选封面，请检查 SteamGridDB API Key 或游戏名称");
    }
  } catch (e) {
    message.error(e instanceof Error ? e.message : String(e));
  } finally {
    coverLoading.value = false;
  }
}

async function selectCover(option: CoverOption) {
  if (!selected.value || coverSelecting.value) return;
  coverSelecting.value = true;
  try {
    await store.setCoverFromUrl(selected.value.id, option.url);
    message.success("封面已更新");
    showCoverPicker.value = false;
  } catch (e) {
    message.error(e instanceof Error ? e.message : String(e));
  } finally {
    coverSelecting.value = false;
  }
}

// ==================== 右键菜单 ====================

const showMenu = ref(false);
const menuX = ref(0);
const menuY = ref(0);
const menuReview = ref<(typeof store.reviews)[number] | null>(null);

function handleContextMenu(e: MouseEvent, review: (typeof store.reviews)[number]) {
  const target = e.target as HTMLElement | null;
  if (target?.closest?.("input, textarea, [contenteditable='true']")) {
    return;
  }
  e.preventDefault();
  e.stopPropagation();
  menuReview.value = review;
  menuX.value = e.clientX;
  menuY.value = e.clientY;
  showMenu.value = true;
}

const menuItems = computed<ContextMenuItem[]>(() => {
  const review = menuReview.value;
  if (!review) return [];
  return [
    {
      label: "刷新信息",
      icon: "🔄",
      action: () => handleRefreshInfo(review),
    },
    {
      label: "更换封面",
      icon: "🖼️",
      action: () => openCoverPicker(review),
    },
    { label: "", icon: "", action: () => {}, divider: true },
    {
      label: "删除",
      icon: "🗑️",
      danger: true,
      action: () => confirmDelete(review),
    },
  ];
});

// ==================== HLTB 格式化 ====================

function formatHltb(minutes: number | null): string {
  if (minutes == null) return "-";
  if (minutes < 60) return `${minutes}m`;
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  return m > 0 ? `${h}h ${m}m` : `${h}h`;
}

// ==================== 初始化 ====================

onMounted(async () => {
  await store.loadReviews();
  // 确保游戏库列表已加载（供导入选择器使用）
  if (gamesStore.games.length === 0) {
    gamesStore.loadGames().catch(() => {});
  }
});
</script>

<template>
  <div class="reviews-view">
    <!-- 工具条 -->
    <div class="toolbar">
      <div class="toolbar-left">
        <n-input
          v-model:value="searchInput"
          class="search-input"
          placeholder="搜索手账游戏…"
          clearable
          @update:value="onSearch"
        >
          <template #prefix>
            <n-icon :component="SearchOutline" />
          </template>
        </n-input>
        <n-select
          v-model:value="store.statusFilter"
          class="filter-select"
          :options="statusOptions"
          placeholder="状态"
        />
        <n-select
          v-model:value="store.genreFilter"
          class="filter-select"
          :options="genreOptions"
          placeholder="类型"
        />
        <n-select
          v-model:value="store.sortBy"
          class="sort-select"
          :options="sortOptions"
        />
        <n-select
          v-model:value="store.sortOrder"
          class="sort-order-select"
          :options="sortOrderOptions"
        />
      </div>
      <div class="toolbar-right">
        <n-button type="primary" @click="openAddModal">
          <template #icon>
            <n-icon :component="AddOutline" />
          </template>
          添加手账
        </n-button>
      </div>
    </div>

    <!-- 内容区 -->
    <div class="content">
      <n-spin :show="store.loading">
        <template v-if="store.filteredReviews.length > 0">
          <div class="card-grid">
            <div
              v-for="review in store.filteredReviews"
              :key="review.id"
              class="review-card"
              @click="openDetail(review)"
              @contextmenu="handleContextMenu($event, review)"
            >
              <div class="card-cover-wrap">
                <img
                  v-if="coverOf(review)"
                  :src="coverOf(review)!"
                  class="card-cover"
                  draggable="false"
                  alt=""
                />
                <div v-else class="card-cover card-cover-placeholder">
                  <n-icon size="28" color="rgba(255,255,255,0.3)">
                    <GameControllerOutline />
                  </n-icon>
                </div>
                <!-- 刷新中遮罩 -->
                <div v-if="isRefreshing(review.id)" class="card-loading-mask">
                  <n-spin size="small" />
                </div>
                <!-- 评分徽章 -->
                <div v-if="review.rating != null" class="rating-badge">
                  <n-icon size="11" style="margin-right: 2px">
                    <StarOutline />
                  </n-icon>
                  {{ review.rating }}
                </div>
                <!-- 状态角标 -->
                <div
                  v-if="review.status"
                  class="status-corner"
                  :style="{ backgroundColor: statusMeta[review.status]?.color }"
                >
                  {{ statusMeta[review.status]?.label }}
                </div>
              </div>
              <div class="card-name" :title="displayName(review)">{{ displayName(review) }}</div>
            </div>
          </div>
        </template>
        <n-empty
          v-else-if="!store.loading"
          class="empty-state"
          description="还没有手账条目，点击右上角「添加手账」开始吧"
        />
      </n-spin>
    </div>

    <!-- 添加弹窗 -->
    <n-modal
      v-model:show="showAddModal"
      preset="card"
      title="添加手账"
      style="width: 480px"
      :mask-closable="!adding"
    >
      <n-tabs v-model:value="addTab" type="line" animated>
        <n-tab-pane name="name" tab="输入名称">
          <n-input
            v-model:value="newName"
            placeholder="输入游戏名称，如：空洞骑士"
            clearable
            @keyup.enter="confirmAdd"
          />
          <p class="add-hint">添加后将自动通过 LLM 拉取游戏信息、SteamGridDB 拉取封面</p>
        </n-tab-pane>
        <n-tab-pane name="import" tab="从游戏库导入">
          <n-select
            v-model:value="importGameId"
            :options="importOptions"
            placeholder="选择游戏库中的游戏（已在手账中的已过滤）"
            filterable
            clearable
          />
          <p class="add-hint">导入后复用游戏库的元数据与封面；英文原名会自动填入英文名</p>
        </n-tab-pane>
      </n-tabs>
      <template #footer>
        <n-button :disabled="adding" @click="showAddModal = false">取消</n-button>
        <n-button type="primary" :loading="adding" @click="confirmAdd">添加</n-button>
      </template>
    </n-modal>

    <!-- 详情抽屉 -->
    <n-drawer v-model:show="showDetail" :width="500" placement="right">
      <n-drawer-content closable @close="closeDetail">
        <template v-if="selected">
          <!-- 封面与操作 -->
          <div class="detail-cover-wrap">
            <img
              v-if="coverOf(selected)"
              :src="coverOf(selected)!"
              class="detail-cover"
              draggable="false"
              alt=""
            />
            <div v-else class="detail-cover detail-cover-placeholder">
              <n-icon size="56" color="rgba(255,255,255,0.25)">
                <GameControllerOutline />
              </n-icon>
            </div>
            <div class="detail-actions">
              <n-button size="small" :loading="isRefreshing(selected.id)" @click="handleRefreshInfo(selected)">
                <template #icon>
                  <n-icon :component="RefreshOutline" />
                </template>
                刷新信息
              </n-button>
              <n-button size="small" @click="openCoverPicker(selected)">
                <template #icon>
                  <n-icon :component="ImageOutline" />
                </template>
                更换封面
              </n-button>
            </div>
          </div>

          <!-- 名称与状态 -->
          <div class="detail-name-block">
            <div class="name-field">
              <div class="name-field-label">游戏名称（原名）</div>
              <div class="detail-name-row">
                <n-input
                  v-model:value="editingName"
                  placeholder="游戏名称"
                  @blur="saveName"
                  @keyup.enter="saveName"
                />
                <n-button type="primary" ghost @click="saveName">
                  <template #icon>
                    <n-icon :component="CreateOutline" :size="18" />
                  </template>
                </n-button>
              </div>
              <p v-if="selectedNameIsAscii" class="name-hint">
                原名即英文，已自动同步到下方英文名框（卡片展示与封面检索优先用英文名）
              </p>
            </div>
            <div class="name-field">
              <div class="name-field-label">官方英文名（检索与展示优先）</div>
              <div class="detail-name-row name-en-row">
                <n-input
                  v-model:value="editingNameEn"
                  placeholder="英文名（封面检索用，留空保存即清除）"
                  @blur="saveNameEn"
                  @keyup.enter="saveNameEn"
                />
                <n-button type="primary" ghost @click="saveNameEn">
                  <template #icon>
                    <n-icon :component="CreateOutline" :size="18" />
                  </template>
                </n-button>
              </div>
            </div>
          </div>
          <n-button-group class="status-segmented">
            <n-button
              v-for="opt in statusButtonOptions"
              :key="opt.value"
              size="small"
              :type="selected.status === opt.value ? 'primary' : 'default'"
              @click="handleSetStatus(opt.value)"
            >
              {{ opt.label }}
            </n-button>
          </n-button-group>

          <!-- 元数据 -->
          <n-grid v-if="selected.description || selected.developer || selected.genres.length" class="detail-meta" :cols="2" :x-gap="16" :y-gap="10">
            <n-gi v-if="selected.developer">
              <div class="meta-label">开发商</div>
              <div class="meta-value">{{ selected.developer }}</div>
            </n-gi>
            <n-gi v-if="selected.publisher">
              <div class="meta-label">发行商</div>
              <div class="meta-value">{{ selected.publisher }}</div>
            </n-gi>
            <n-gi v-if="selected.release_date">
              <div class="meta-label">发售日期</div>
              <div class="meta-value">{{ selected.release_date }}</div>
            </n-gi>
            <n-gi v-if="selected.hltb_main_story || selected.hltb_main_extra || selected.hltb_completionist">
              <div class="meta-label">通关时长参考</div>
              <div class="meta-value">
                <span v-if="selected.hltb_main_story">主线 {{ formatHltb(selected.hltb_main_story) }}</span>
                <span v-if="selected.hltb_main_extra"> / 主线+支线 {{ formatHltb(selected.hltb_main_extra) }}</span>
                <span v-if="selected.hltb_completionist"> / 完美 {{ formatHltb(selected.hltb_completionist) }}</span>
              </div>
            </n-gi>
            <n-gi v-if="selected.genres.length">
              <div class="meta-label">类型</div>
              <div class="meta-value genre-tags">
                <n-tag v-for="g in selected.genres" :key="g" size="small" :bordered="false" type="info">
                  {{ g }}
                </n-tag>
              </div>
            </n-gi>
            <n-gi v-if="selected.description" :span="2">
              <div class="meta-label">简介</div>
              <n-text depth="3" class="meta-desc">{{ selected.description }}</n-text>
            </n-gi>
          </n-grid>

          <!-- 评分 -->
          <div class="detail-section">
            <div class="section-title">
              <n-icon :component="StarOutline" />
              我的评分
            </div>
            <div class="rating-input-row">
              <n-input-number
                v-model:value="ratingInput"
                :min="0"
                :max="10"
                :step="1"
                placeholder="0-10"
                class="rating-input"
              />
              <n-button size="small" type="primary" @click="handleSetRating(ratingInput)">
                保存
              </n-button>
              <n-button size="small" quaternary @click="handleSetRating(null)">清除</n-button>
            </div>
            <div class="rating-quick">
              <button
                v-for="i in 10"
                :key="i"
                class="rating-btn"
                :class="{ active: selected.rating === i }"
                @click="handleSetRating(i)"
              >
                {{ i }}
              </button>
            </div>
          </div>

          <!-- 评价 -->
          <div class="detail-section">
            <div class="section-title">我的评价</div>
            <n-input
              v-model:value="reviewText"
              type="textarea"
              :rows="5"
              placeholder="写下你对这款游戏的评价…"
            />
            <div class="review-save-row">
              <n-button size="small" type="primary" :loading="savingReview" @click="saveReviewText">
                保存评价
              </n-button>
            </div>
          </div>

          <!-- 截图库 -->
          <div class="detail-section">
            <div class="section-title">
              <n-icon :component="ImagesOutline" />
              截图库
              <n-button size="tiny" quaternary style="margin-left: auto" @click="openDirPicker">
                <template #icon>
                  <n-icon :component="CreateOutline" />
                </template>
                指定目录
              </n-button>
              <n-button
                size="tiny"
                quaternary
                :loading="openingScreenshots"
                :disabled="!screenshotInfo"
                @click="handleOpenScreenshots"
              >
                <template #icon>
                  <n-icon :component="FolderOpenOutline" />
                </template>
                打开
              </n-button>
            </div>
            <div v-if="screenshotInfo" class="screenshot-item" @click="handleOpenScreenshots">
              <span class="save-path-text" :title="screenshotInfo.path">{{ screenshotInfo.path }}</span>
              <span v-if="screenshotInfo.count > 0" class="screenshot-count">
                {{ screenshotInfo.count }} 张
              </span>
              <span v-else class="screenshot-empty">暂无截图</span>
            </div>
            <div v-else class="screenshot-unavailable">
              <span class="screenshot-unavailable-text">
                {{ screenshotError ?? "正在读取截图目录…" }}
              </span>
              <n-button
                v-if="screenshotError"
                size="tiny"
                quaternary
                type="primary"
                @click="openDirPicker"
              >
                指定截图目录
              </n-button>
            </div>
            <div v-if="screenshotInfo && sourceLabel" class="screenshot-source">
              {{ sourceLabel }}
              <span v-if="selected?.screenshot_dir" class="screenshot-source-dir">
                {{ selected.screenshot_dir }}
              </span>
            </div>
          </div>

          <!-- 危险操作 -->
          <div class="detail-footer">
            <n-button size="small" type="error" quaternary @click="confirmDelete(selected)">
              <template #icon>
                <n-icon :component="TrashOutline" />
              </template>
              删除手账
            </n-button>
          </div>
        </template>
      </n-drawer-content>
    </n-drawer>

    <!-- 封面选择弹窗 -->
    <n-modal
      v-model:show="showCoverPicker"
      preset="card"
      title="选择封面"
      style="width: 680px"
    >
      <div class="cover-filter-row">
        <n-button-group size="small">
          <n-button
            :type="coverFilter === 'portrait' ? 'primary' : 'default'"
            @click="coverFilter = 'portrait'"
          >
            竖版
          </n-button>
          <n-button
            :type="coverFilter === 'landscape' ? 'primary' : 'default'"
            @click="coverFilter = 'landscape'"
          >
            横版
          </n-button>
          <n-button
            :type="coverFilter === 'all' ? 'primary' : 'default'"
            @click="coverFilter = 'all'"
          >
            全部
          </n-button>
        </n-button-group>
      </div>
      <div class="cover-grid">
        <n-spin :show="coverLoading">
          <template v-if="displayedCovers.length > 0">
            <div class="cover-grid-inner">
              <div
                v-for="(cover, idx) in displayedCovers"
                :key="idx"
                class="cover-option"
                :class="{ loading: coverSelecting }"
                @click="selectCover(cover)"
              >
                <img :src="cover.thumb_url" loading="lazy" draggable="false" alt="" />
              </div>
            </div>
            <div v-if="hasMoreCovers" class="cover-more">
              <n-button size="small" quaternary @click="coverCount += 20">加载更多</n-button>
            </div>
          </template>
          <n-empty v-else-if="!coverLoading" description="没有可用封面" />
        </n-spin>
      </div>
    </n-modal>

    <!-- 截图目录选择弹窗 -->
    <n-modal
      v-model:show="showDirPicker"
      preset="card"
      title="指定截图目录"
      style="width: 520px"
    >
      <div class="dir-picker-hint">
        手账自己记住这个截图文件夹，之后从游戏库删除游戏、改名字都不影响。
        留空则按同名游戏自动匹配（游戏已删除时靠历史归档匹配）。
      </div>
      <n-select
        v-model:value="dirInput"
        :options="dirSelectOptions"
        :loading="loadingDirOptions"
        clearable
        filterable
        tag
        placeholder="选择或输入截图根目录下的文件夹名"
      />
      <template #footer>
        <div class="dir-picker-footer">
          <n-button size="small" @click="showDirPicker = false">取消</n-button>
          <n-button size="small" type="primary" :loading="savingDir" @click="saveDirPicker">
            保存
          </n-button>
        </div>
      </template>
    </n-modal>

    <!-- 右键菜单 -->
    <ContextMenu
      v-if="showMenu"
      :items="menuItems"
      :x="menuX"
      :y="menuY"
      @close="showMenu = false"
    />
  </div>
</template>

<style scoped>
.reviews-view {
  display: flex;
  flex-direction: column;
  height: 100%;
}

/* ==================== 工具条 ==================== */
.toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 16px;
  flex-wrap: wrap;
}

.toolbar-left {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.search-input {
  width: 220px;
}

.filter-select {
  width: 110px;
}

.sort-select {
  width: 110px;
}

.sort-order-select {
  width: 80px;
}

/* ==================== 内容区 ==================== */
.content {
  flex: 1;
  overflow-y: auto;
  padding-bottom: 24px;
}

.card-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
  gap: 16px;
}

.review-card {
  cursor: pointer;
  user-select: none;
  transition: transform 0.15s ease;
}

.review-card:hover {
  transform: translateY(-3px);
}

.card-cover-wrap {
  position: relative;
  aspect-ratio: 2 / 3;
  border-radius: 8px;
  overflow: hidden;
  background: rgba(255, 255, 255, 0.04);
}

.card-cover {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}

.card-cover-placeholder {
  display: flex;
  align-items: center;
  justify-content: center;
}

.card-loading-mask {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.5);
}

.rating-badge {
  position: absolute;
  top: 8px;
  left: 8px;
  display: flex;
  align-items: center;
  padding: 2px 8px;
  border-radius: 999px;
  background: rgba(0, 0, 0, 0.75);
  color: #ffd75e;
  font-size: 12px;
  font-weight: 700;
}

.status-corner {
  position: absolute;
  top: 8px;
  right: 8px;
  padding: 2px 8px;
  border-radius: 999px;
  color: #fff;
  font-size: 11px;
  font-weight: 600;
}

.card-name {
  margin-top: 6px;
  font-size: 13px;
  text-align: center;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  color: var(--text-color, rgba(255, 255, 255, 0.85));
}

.empty-state {
  margin-top: 80px;
}

/* ==================== 添加弹窗 ==================== */
.add-hint {
  margin-top: 8px;
  font-size: 12px;
  opacity: 0.6;
}

/* ==================== 详情抽屉 ==================== */
.detail-cover-wrap {
  position: relative;
  width: 220px;
  margin: 0 auto 16px;
  border-radius: 10px;
  overflow: hidden;
  aspect-ratio: 2 / 3;
  background: rgba(255, 255, 255, 0.04);
}

.detail-cover {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}

.detail-cover-placeholder {
  display: flex;
  align-items: center;
  justify-content: center;
}

.detail-actions {
  position: absolute;
  bottom: 8px;
  left: 0;
  right: 0;
  display: flex;
  justify-content: center;
  gap: 8px;
  background: linear-gradient(to top, rgba(0, 0, 0, 0.7), transparent);
  padding: 16px 8px 8px;
}

.detail-name-block {
  margin-bottom: 12px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.detail-name-row {
  display: flex;
  gap: 8px;
  margin-bottom: 0;
}

.name-field {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.name-field-label {
  font-size: 11px;
  opacity: 0.55;
}

.name-hint {
  margin: 0;
  font-size: 11px;
  line-height: 1.5;
  opacity: 0.5;
}

.name-en-row .n-input {
  font-style: italic;
}

.status-segmented {
  width: 100%;
  margin-bottom: 16px;
}

.detail-meta {
  margin-bottom: 16px;
  padding: 12px;
  border-radius: 8px;
  background: rgba(255, 255, 255, 0.04);
}

.meta-label {
  font-size: 11px;
  opacity: 0.55;
  margin-bottom: 2px;
}

.meta-value {
  font-size: 13px;
}

.genre-tags {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}

.meta-desc {
  font-size: 12px;
  line-height: 1.6;
  display: block;
  margin-top: 2px;
}

.detail-section {
  margin-bottom: 20px;
}

.section-title {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  font-weight: 600;
  margin-bottom: 10px;
}

.rating-input-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 10px;
}

.rating-input {
  width: 120px;
}

.rating-quick {
  display: grid;
  grid-template-columns: repeat(10, 1fr);
  gap: 4px;
}

.rating-btn {
  aspect-ratio: 1;
  border: 1px solid rgba(255, 255, 255, 0.12);
  border-radius: 6px;
  background: transparent;
  color: inherit;
  font-size: 12px;
  cursor: pointer;
  transition: all 0.15s;
}

.rating-btn:hover {
  border-color: var(--accent-color, #6366f1);
  color: var(--accent-color, #6366f1);
}

.rating-btn.active {
  background: #ffd75e;
  border-color: #ffd75e;
  color: #1a1a2e;
  font-weight: 700;
}

.review-save-row {
  margin-top: 8px;
  display: flex;
  justify-content: flex-end;
}

/* ==================== 截图库 ==================== */
.screenshot-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 4px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.05);
  border-radius: 4px;
  cursor: pointer;
  transition: background-color 0.2s;
}

.screenshot-item:hover {
  background-color: rgba(255, 255, 255, 0.05);
}

.save-path-text {
  flex: 1;
  font-size: 11px;
  color: #aaa;
  word-break: break-all;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.screenshot-count {
  flex-shrink: 0;
  font-size: 11px;
  font-weight: 600;
  color: var(--accent-color, #6366f1);
}

.screenshot-empty {
  flex-shrink: 0;
  font-size: 11px;
  color: #666;
}

.screenshot-unavailable {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px;
  font-size: 11px;
  color: #666;
}

.screenshot-unavailable-text {
  flex: 1;
  line-height: 1.5;
}

.screenshot-source {
  padding: 4px 4px 0;
  font-size: 11px;
  color: #777;
}

.screenshot-source-dir {
  margin-left: 4px;
  font-weight: 600;
  color: var(--accent-color, #6366f1);
}

/* ==================== 截图目录选择弹窗 ==================== */
.dir-picker-hint {
  margin-bottom: 12px;
  font-size: 12px;
  line-height: 1.6;
  color: #999;
}

.dir-picker-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.detail-footer {
  border-top: 1px solid rgba(255, 255, 255, 0.08);
  padding-top: 12px;
}

/* ==================== 封面选择弹窗 ==================== */
.cover-filter-row {
  margin-bottom: 12px;
}

.cover-grid-inner {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
  gap: 10px;
  max-height: 480px;
  overflow-y: auto;
}

.cover-option {
  aspect-ratio: 2 / 3;
  border-radius: 6px;
  overflow: hidden;
  cursor: pointer;
  border: 2px solid transparent;
  transition: border-color 0.15s;
}

.cover-option:hover {
  border-color: var(--accent-color, #6366f1);
}

.cover-option.loading {
  cursor: wait;
  opacity: 0.6;
}

.cover-option img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}

.cover-more {
  text-align: center;
  margin-top: 12px;
}

/* 亮色主题适配 */
:global(.light-theme) .card-cover-wrap,
:global(.light-theme) .detail-cover-wrap {
  background: rgba(0, 0, 0, 0.06);
}

:global(.light-theme) .card-name {
  color: rgba(0, 0, 0, 0.85);
}

:global(.light-theme) .detail-meta {
  background: rgba(0, 0, 0, 0.04);
}

:global(.light-theme) .rating-btn {
  border-color: rgba(0, 0, 0, 0.15);
}

:global(.light-theme) .detail-footer {
  border-top-color: rgba(0, 0, 0, 0.08);
}

:global(.light-theme) .screenshot-item {
  border-bottom-color: rgba(0, 0, 0, 0.05);
}

:global(.light-theme) .screenshot-item:hover {
  background-color: rgba(0, 0, 0, 0.04);
}

:global(.light-theme) .save-path-text,
:global(.light-theme) .screenshot-empty,
:global(.light-theme) .screenshot-unavailable {
  color: rgba(0, 0, 0, 0.55);
}

:global(.light-theme) .screenshot-source,
:global(.light-theme) .dir-picker-hint {
  color: rgba(0, 0, 0, 0.5);
}
</style>
