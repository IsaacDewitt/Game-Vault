<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, nextTick, watch, h } from "vue";
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
  NDropdown,
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
import { open } from "@tauri-apps/plugin-dialog";
import { useReviewsStore, displayName } from "../stores/reviews";
import { useGamesStore } from "../stores/games";
import { getScreenshotDir, openScreenshotDir, listScreenshotDirs } from "../lib/tauri";
import type { CoverOption, ScreenshotDirInfo, ScreenshotDirOption } from "../lib/tauri";
import { DEBOUNCE_MS } from "../lib/constants";
import { formatPlayTime, formatDate } from "../lib/format";
import { countChars, parseRichText, previewText } from "../lib/richText";
import ContextMenu from "../components/ContextMenu.vue";
import type { ContextMenuItem } from "../components/ContextMenu.vue";

const store = useReviewsStore();
const gamesStore = useGamesStore();
const message = useMessage();
const dialog = useDialog();

// ==================== 状态与筛选 ====================

// 手账里记录的必然都是玩过的游戏，故**没有状态分类**（2026-10-28 老爷拍板）：
// 「想玩 / 游玩中 / 已通关 / 已弃坑」四态就此从手账彻底移除 —— 筛选、角标、
// 详情切换一并去掉，`reviews.status` 只留库内历史值（新增默认 completed）。
const sortOptions = [
  { label: "添加时间", value: "added_at" },
  { label: "评分", value: "rating" },
  { label: "游玩时长", value: "play_time" },
  { label: "最近更新", value: "updated_at" },
  { label: "名称", value: "name" },
];

const sortOrderOptions = [
  { label: "降序", value: "desc" },
  { label: "升序", value: "asc" },
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

/** 空态「清空筛选」：搜索框是本地 ref，store 里另有搜索词与类型筛选 */
function clearReviewFilters() {
  searchInput.value = "";
  store.searchQuery = "";
  store.genreFilter = "";
}

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

// ==================== 评价：阅读 / 编辑双态 ====================
//
// 口径（2026-10-24）：写成长文后，5 行高的文本框里扒拉太难受，故默认**阅读态**：
// 按段落渲染（空行分段、`- ` 列表、`**加粗**`，见 `lib/richText.ts`），超 8 行折叠；
// 点「编辑」才切回输入框。两点刻意为之：
// - 点正文**不进**编辑态——读的时候手一抖就变输入框，比多按一次按钮更烦；
// - 退出编辑只认「完成」与 Ctrl/Cmd+Enter，**不用 Esc**——中文输入法里 Esc 是
//   取消候选词，绑上去必然误伤。
const reviewEditing = ref(false);
const reviewExpanded = ref(false);
const reviewInputRef = ref<InstanceType<typeof NInput> | null>(null);

/** 折叠阈值：8 行或 260 字，任一超出即折叠（纯行数判不出长句，纯字数判不出短行列表） */
const REVIEW_COLLAPSE_LINES = 8;
const REVIEW_COLLAPSE_CHARS = 260;
/** 对象字面量要提到外面：写在模板里每次渲染都是新引用，会让 n-input 重算高度 */
const REVIEW_AUTOSIZE = { minRows: 4, maxRows: 14 };

const reviewBlocks = computed(() => parseRichText(reviewText.value));
const reviewCharCount = computed(() => countChars(reviewText.value));
const reviewShouldCollapse = computed(() => {
  if (!reviewText.value.trim()) return false;
  return (
    reviewCharCount.value > REVIEW_COLLAPSE_CHARS ||
    reviewText.value.split("\n").length > REVIEW_COLLAPSE_LINES
  );
});

/** 进入编辑态：预填当前文本并把焦点交给输入框 */
function startReviewEdit() {
  reviewEditing.value = true;
  nextTick(() => reviewInputRef.value?.focus());
}

/**
 * 退出编辑态。
 *
 * 先在本地收刀，再等落库；**保存失败则退回编辑态**，否则错误提示会跟着
 * 编辑框一起消失，用户只会以为写进去了。
 */
async function finishReviewEdit() {
  reviewEditing.value = false;
  const ok = await saveReviewNow();
  if (!ok) reviewEditing.value = true;
}

/**
 * 评价框的快捷键：Ctrl/Cmd + Enter 收刀。
 *
 * `isComposing` 守卫不能省：中文输入法还在组字时按回车是「上屏」，
 * 直接收刀会把没上屏的候选词连同输入框一起扔掉。
 */
function onReviewKeydown(e: KeyboardEvent) {
  if (e.isComposing) return;
  if (e.key !== "Enter" || !(e.ctrlKey || e.metaKey)) return;
  e.preventDefault();
  void finishReviewEdit();
}

// ==================== 评价自动保存 ====================
//
// 口径（2026-10-17）：
// - 停手 AUTOSAVE_IDLE_MS 后自动落库，不弹成功提示（否则每段话都弹一次，吵）；
// - 切换条目 / 关闭抽屉时**立即 flush**，不能等防抖——否则旧条目的编辑会被新条目覆盖；
// - 无变化不写库（`text === savedText`），避免打开条目就产生一次无谓 UPDATE
//   （它会推进 updated_at，让「最近更新」排序凭空跳动）；
// - 失败时保留本地文本并露出重试入口，不静默吞掉。
const AUTOSAVE_IDLE_MS = 1000;

type ReviewSaveState = "idle" | "pending" | "saving" | "saved" | "error";

/** 当前正在编辑的条目 id（比 selected 可靠：关闭抽屉后 selected 会先被清空） */
let editingReviewId: string | null = null;
/** 详情打开的请求序号：快速连点两个条目时，先发起的那次作废（避免状态串台） */
let detailOpenSeq = 0;
/** 已成功落库的文本，用于跳过无变化保存 */
let savedText = "";

const reviewSaveState = ref<ReviewSaveState>("idle");
const reviewSavedAt = ref<Date | null>(null);
const reviewSaveError = ref<string | null>(null);

/** 「已保存 · HH:mm」的时间部分 */
const reviewSavedAtText = computed(() => {
  const d = reviewSavedAt.value;
  if (!d) return "";
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
});

/** 正在飞的写库链尾；并发调用方接在它后面，保证「最新文本」一定被写一次 */
let savingPromise: Promise<boolean> | null = null;

/**
 * 写库失败时的托底草稿放在 `useReviewsStore` 的 `unsavedDrafts` 里。
 *
 * 组件本地单槽会在不同条目连续失败时互相覆盖，也会随视图卸载消失；
 * store 里的草稿簿按 reviewId 保留多份，重开条目时原样恢复并自动重试。
 */

/**
 * 立即把当前文本落库；返回的 promise **覆盖到最新文本真正落库**。
 *
 * 在飞期间又来了保存请求时，旧实现返回旧快照那次写库的 promise 并
 * fire-and-forget 补写 —— 调用方（切条目 / 关抽屉 / 退出编辑态）拿到
 * `ok=true` 就以为存好了。现在把补写串进同一条链，await 到的就是
 * 「当前文本」的最终结果；链尾结束才清 `savingPromise`。
 */
function saveReviewNow(): Promise<boolean> {
  const prev = savingPromise;
  const next = (prev ?? Promise.resolve(true)).then(() => saveLatestReview());
  savingPromise = next;
  void next.finally(() => {
    // 只有仍是链尾才清空，防止后到的并发请求把标记提前抹掉
    if (savingPromise === next) savingPromise = null;
  });
  return next;
}

async function saveLatestReview(): Promise<boolean> {
  const id = editingReviewId;
  if (!id) return true;

  const text = reviewText.value;
  if (text === savedText) {
    // 无变化：把「待保存」的指示收回，回到「修改后自动保存」。
    // 不覆盖错误态——那个要用户手动重试，不能被一次空跑抹掉。
    if (reviewSaveState.value === "pending") reviewSaveState.value = "idle";
    // 例外：恢复托底草稿后若发现内容与库里一致，说明上次其实已落库、
    // 只是回执丢了（store 抛错发生在写库之后）；此时要清掉这条误报，
    // 否则「保存失败」会一直挂着且永远等不到重试。只清同文本的草稿，
    // 其它文本/其它条目的失败草稿必须继续保留。
    if (reviewSaveState.value === "error") {
      const draft = store.getUnsavedDraft(id);
      if (!draft || draft.text === text) {
        reviewSaveState.value = "saved";
        reviewSaveError.value = null;
        reviewSavedAt.value = new Date();
        store.clearUnsavedDraft(id);
      }
    }
    return true;
  }

  reviewSaveState.value = "saving";
  try {
    await store.setReview(id, text);
    savedText = text;
    // 确认落库后按 id 清掉本条目的失败草稿，不比对文本：
    // 链式补写是串行的，能走到这里说明「本条目的最新文本」已入库，
    // 之前那次失败留下的旧草稿必须作废，否则重开条目会把旧字
    // 恢复回来、反倒盖掉刚保存成功的新内容。其它条目不受影响。
    store.clearUnsavedDraft(id);
    // 保存期间可能已切到别的条目：只在仍是同一条目时更新可见状态
    if (editingReviewId === id) {
      reviewSaveState.value = "saved";
      reviewSavedAt.value = new Date();
      reviewSaveError.value = null;
    }
    return true;
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    store.saveUnsavedDraft(id, { text, error: msg });
    // 无条件记录错误：调用方可能已经切条目 / 关抽屉，旧版的
    // `editingReviewId === id` 守卫会把这次失败静默吞掉。
    reviewSaveState.value = "error";
    reviewSaveError.value = msg;
    return false;
  }
}

const debouncedSaveReview = useDebounceFn(() => {
  void saveReviewNow();
}, AUTOSAVE_IDLE_MS);

watch(reviewText, () => {
  if (!editingReviewId) return;
  if (reviewText.value === savedText) return;
  // 正在写库时不要退回「待保存」，否则状态会在 saving 与 pending 之间闪
  if (reviewSaveState.value !== "saving") reviewSaveState.value = "pending";
  debouncedSaveReview();
});

/** 关闭抽屉（点 X / 点遮罩都会走到这里，故清理由 watch(showDetail) 统一做） */
function closeDetail() {
  showDetail.value = false;
}

watch(showDetail, (open) => {
  if (open) return;
  // 关闭前把手上的文本冲掉，再清选中态。
  // 用 detailOpenSeq 做守卫：写库是异步的，若这段时间用户又点开了别的条目，
  // 本次清理必须让位——否则会把新条目刚设好的 editingReviewId / selected 清掉。
  const seq = detailOpenSeq;
  const closingId = editingReviewId;
  void saveReviewNow().then((ok) => {
    if (detailOpenSeq !== seq) return;
    if (!ok) {
      // 写库失败：抽屉已经关了，但内容已在 store 草稿簿按 id 托底，不会丢。
      // 保留 editingReviewId 与错误态，重开这一条会自动恢复并提示重试。
      reviewSaveState.value = "error";
      message.error("评价没能保存到本地，内容已保留；重新打开该条目即可继续重试");
      return;
    }
    store.clearSelection();
    if (editingReviewId === closingId) editingReviewId = null;
    reviewSaveState.value = "idle";
    // 双态与展开状态跟着条目走：下次打开任何条目都从阅读态、未展开开始
    reviewEditing.value = false;
    reviewExpanded.value = false;
  });
});

// ==================== 元数据卡片内「点击改名」（中文名 / 英文名） ====================
// 就地编辑：点一下值 → 变输入框 → 失焦/回车提交，Esc 取消。
// 与上方「名称&英文名」两个大输入框是同一份数据的两个入口，各自独立保存、互不干扰。
type InlineNameField = "name" | "name_en";
const editingField = ref<InlineNameField | null>(null);
const inlineName = ref("");
const inlineNameEn = ref("");
const nameEditRef = ref<InstanceType<typeof NInput> | null>(null);
const nameEnEditRef = ref<InstanceType<typeof NInput> | null>(null);

/** 进入某字段的就地编辑态（预填当前值），并把焦点交给输入框 */
function startNameEdit(field: InlineNameField) {
  if (!selected.value) return;
  editingField.value = field;
  inlineName.value = selected.value.name ?? "";
  inlineNameEn.value = selected.value.name_en ?? "";
  nextTick(() => {
    const ref = field === "name" ? nameEditRef.value : nameEnEditRef.value;
    ref?.focus();
  });
}

/** 放弃就地编辑，不改动任何数据 */
function cancelNameEdit() {
  editingField.value = null;
}

/**
 * 提交就地编辑。
 *
 * 口径与上方大输入框保持一致：
 * - 中文名：空值或未变 → 不动；否则改名（后端对纯 ASCII 原名会自动补 name_en）。
 * - 英文名：空串 → 清除（哨兵语义）；未变 → 不动。
 */
async function commitNameEdit() {
  const field = editingField.value;
  if (!field || !selected.value) return;
  const id = selected.value.id;
  editingField.value = null;
  try {
    if (field === "name") {
      const name = inlineName.value.trim();
      if (!name || name === selected.value.name) return;
      const updated = await store.updateMeta(id, { name });
      // 后端可能自动补了 name_en（纯英文原名），同步回大输入框，
      // 否则下次 blur 英文名框时会拿旧值覆盖回去
      editingName.value = updated.name;
      editingNameEn.value = updated.name_en ?? "";
      message.success("中文名已更新");
    } else {
      const trimmed = inlineNameEn.value.trim();
      const current = selected.value.name_en ?? "";
      if (trimmed === current) return;
      await store.updateMeta(id, { name_en: trimmed });
      editingNameEn.value = trimmed;
      message.success(trimmed ? "英文名已更新" : "英文名已清除");
    }
  } catch (e) {
    message.error(e instanceof Error ? e.message : String(e));
  }
}

const selected = computed(() => store.selectedReview);

/** 主名是否为纯 ASCII（即原名本身就是英文） */
const selectedNameIsAscii = computed(() => {
  const name = selected.value?.name ?? "";
  return name.length > 0 && /^[\x20-\x7E]+$/.test(name);
});

async function openDetail(review: (typeof store.reviews)[number]) {
  const seq = ++detailOpenSeq;
  const previousId = editingReviewId;
  const sameEntry = previousId === review.id;

  // 切条目 / 同条目快速重开，都要先把未落库的评价冲掉。
  // 【2026-11-08 复审】失败时**不能 return**：错误条、重试按钮、缓冲恢复
  // 全在抽屉里，提前 return 会让点卡片毫无反应，把内容锁死在看不见的地方。
  let flushFailed = false;
  if (previousId && (!sameEntry || reviewText.value !== savedText)) {
    flushFailed = !(await saveReviewNow());
    if (flushFailed) reviewSaveState.value = "error";
  }

  // 等待写库期间用户可能又点了别的条目——本次让位，否则会用旧条目的
  // 数据覆盖新条目刚设好的 editingReviewId / reviewText。
  if (seq !== detailOpenSeq) return;

  editingReviewId = review.id;

  // 【2026-11-08 复审】绝不能用点击瞬间传进来的 `review.review` 重置
  // savedText：store 的 updateReviewInStore 是「替换数组元素」，旧引用的
  // .review 仍是上一次的文本。关抽屉 flush 成功后再立刻重开同一条目时，
  // 拿旧对象会把 savedText / reviewText 一起打回旧值，用户再敲一个字就
  // 会用「旧文本+新字」覆盖掉刚存好的新内容。
  //  - 重开同条目且 flush 成功：链尾已保证最新文本落库，直接沿用 savedText；
  //  - 其余情况：从 **store 当前值**取，而不是点击时的快照。
  if (!(sameEntry && !flushFailed)) {
    const current = store.reviews.find((r) => r.id === review.id);
    savedText = current?.review ?? review.review ?? "";
  }

  // 草稿簿按 id 保存：只取当前条目的并清空；其它条目的失败草稿继续留在 store。
  const buffered = store.getUnsavedDraft(review.id);
  if (buffered) store.clearUnsavedDraft(review.id);

  reviewSaveState.value = buffered || flushFailed ? "error" : "idle";
  reviewSaveError.value = buffered?.error ?? (flushFailed ? reviewSaveError.value : null);
  reviewSavedAt.value = null;

  store.selectReview(review);
  editingName.value = review.name;
  editingNameEn.value = review.name_en ?? "";
  ratingInput.value = review.rating;
  reviewText.value = buffered?.text ?? savedText;
  if (buffered) {
    message.warning("上次有内容没保存成功，已为你恢复并重新保存");
    void saveReviewNow();
  } else if (flushFailed && !sameEntry) {
    message.error("上一条评价没能保存，内容已保留；重新打开那一条即可继续重试");
  }
  // 换条目一律回到阅读态、收起长文：沿用上一条的展开状态会让人以为换了一篇
  reviewEditing.value = false;
  reviewExpanded.value = false;
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
    message.success(trimmed ? "英文名已更新" : "英文名已清除");
  } catch (e) {
    message.error(e instanceof Error ? e.message : String(e));
  }
}

/** 刷新信息（只跑 LLM；封面走「更换封面」三源手动指定） */
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
    // 声明在 try 之外：catch 块与 try 块是不同作用域，写在里面 catch 看不见
    let prevEditingId: string | null = null;
    try {
      // 条目即将消失：断开自动保存的归属，避免关闭抽屉时又
      // 对着一个已删除的 id 发起 UPDATE（会以「条目不存在」失败告终）。
      // 删除失败必须恢复，否则这条的自动保存被永久解除（显示「修改后自动保存」，实则不落库）。
      prevEditingId = editingReviewId;
      editingReviewId = null;
      await store.removeReview(review.id, keepCover);
      showDetail.value = false;
      message.success(keepCover ? "已删除，封面图片已留档" : "已删除");
    } catch (e) {
      editingReviewId = prevEditingId;
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
  // 与 openDetail 共用同一个序号：关抽屉的清理逻辑靠它判断「期间是否又开了新东西」。
  // 不递增的话，写库收尾时会把这里刚选中的条目清空，表现为「更换封面」点了没反应。
  detailOpenSeq += 1;
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

// ==================== 封面来源（三源并列，与游戏库详情页同口径） ====================
//
// 2026-10-30 补齐：此前手账只有 SteamGridDB 一个来源，比游戏库少了「本地图片」和
// 「Steam 官方封面」。三个来源互不合并 —— 用户点哪个就是哪个，不做自动串接。

const coverDropdownOptions = [
  { label: "从本地选择", key: "local" },
  { label: "使用 Steam 官方封面", key: "steam" },
  { label: "从 SteamGridDB 选择", key: "steamgriddb" },
];

async function handleCoverDropdown(key: string) {
  const review = selected.value;
  if (!review) return;
  if (key === "local") await pickLocalCover(review);
  else if (key === "steam") await useSteamCover(review);
  else if (key === "steamgriddb") await openCoverPicker(review);
}

/** 本地图片当封面：只把内容收进封面库，用户选的原文件不动 */
async function pickLocalCover(review: (typeof store.reviews)[number]) {
  try {
    const picked = await open({
      multiple: false,
      filters: [{ name: "图片", extensions: ["jpg", "jpeg", "png", "webp"] }],
      title: "选择封面图片",
    });
    if (!picked) return;
    await store.setCoverFromLocal(review.id, picked as string);
    message.success("封面已更换");
  } catch (e) {
    message.error(e instanceof Error ? e.message : String(e));
  }
}

/** Steam 官方封面：appid 先按名认领游戏库，认不到再查 Steam 商店搜索 */
async function useSteamCover(review: (typeof store.reviews)[number]) {
  const tip = message.loading("正在获取 Steam 官方封面…", { duration: 0 });
  try {
    await store.setCoverFromSteam(review.id);
    message.success("已使用 Steam 官方封面");
  } catch (e) {
    message.error(e instanceof Error ? e.message : String(e));
  } finally {
    tip.destroy();
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
    // 封面三源（与详情页下拉同口径）
    {
      label: "封面：从本地选择",
      icon: "📁",
      action: () => pickLocalCover(review),
    },
    {
      label: "封面：使用 Steam 官方",
      icon: "🎮",
      action: () => useSteamCover(review),
    },
    {
      label: "封面：从 SteamGridDB 选",
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

// ==================== 我的游玩时长（按名匹配游戏库，含已删游戏历史） ====================

/** 选中条目的游玩时长（无匹配/无历史为 undefined，整栏不渲染） */
const selectedPlayTime = computed(() =>
  selected.value ? store.playTimes[selected.value.id] : undefined
);

// ==================== 初始化 ====================

onUnmounted(() => {
  // 防抖队列里那次保存可能还没跑（停手不足 1s 就切走），这里兜一次。
  // 即使这次写库失败，失败草稿也会写进 Pinia store，不随本组件卸载消失。
  if (editingReviewId) void saveReviewNow();
});

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
                  loading="lazy"
                  decoding="async"
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
              </div>
              <div class="card-name" :title="displayName(review)">{{ displayName(review) }}</div>
              <!-- 评价摘要：长文在墙上也留个抓手（Markdown 记号会被抹平，只求一眼看懂） -->
              <div v-if="review.review && review.review.trim()" class="card-summary">
                {{ previewText(review.review, 56) }}
              </div>
            </div>
          </div>
        </template>
        <!-- 加载失败必须单独给错误态：旧版一律显示「还没有手账条目」，
             把 DB 异常伪装成空库，还把用户往「添加」按钮上引 -->
        <n-empty v-else-if="store.error" class="empty-state" description="手账列表加载失败">
          <template #extra>
            <div class="empty-error">
              <span>{{ store.error }}</span>
              <n-button size="small" @click="store.loadReviews()">重试</n-button>
            </div>
          </template>
        </n-empty>
        <!-- 库空 vs 筛选无结果：两者该给的建议完全不同 -->
        <n-empty
          v-else-if="!store.loading"
          class="empty-state"
          :description="
            store.reviews.length === 0
              ? '还没有手账条目，点击右上角「添加手账」开始吧'
              : '没有符合条件的手账，换个关键词或清空筛选试试'
          "
        >
          <template v-if="store.reviews.length > 0" #extra>
            <n-button size="small" @click="clearReviewFilters">清空筛选</n-button>
          </template>
        </n-empty>
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
          <p class="add-hint">添加后自动通过 LLM 补全游戏信息与命名；封面在详情页「更换封面」里手动挑选</p>
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
              <n-dropdown
                :options="coverDropdownOptions"
                trigger="click"
                @select="handleCoverDropdown"
              >
                <n-button size="small">
                  <template #icon>
                    <n-icon :component="ImageOutline" />
                  </template>
                  更换封面
                </n-button>
              </n-dropdown>
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

          <!-- 元数据 -->
          <n-grid
            v-if="selected.description || selected.developer || selected.genres.length || selected.publisher || selected.name_en || selected.name"
            class="detail-meta"
            :cols="2"
            :x-gap="16"
            :y-gap="10"
          >
            <!-- 中文名 / 英文名：点击即可就地编辑（放在发行商等字段之前） -->
            <n-gi>
              <div class="meta-label">中文名</div>
              <div
                class="meta-value meta-editable"
                :title="'点击修改中文名'"
                @click="startNameEdit('name')"
              >
                <template v-if="editingField === 'name'">
                  <n-input
                    ref="nameEditRef"
                    v-model:value="inlineName"
                    size="small"
                    placeholder="中文名（留空不改）"
                    @blur="commitNameEdit"
                    @keyup.enter="commitNameEdit"
                    @keyup.esc="cancelNameEdit"
                    @click.stop
                  />
                </template>
                <template v-else>
                  <span>{{ selected.name || "—" }}</span>
                  <n-icon class="meta-edit-pen" :component="CreateOutline" :size="14" />
                </template>
              </div>
            </n-gi>
            <n-gi>
              <div class="meta-label">英文名</div>
              <div
                class="meta-value meta-editable"
                :title="'点击修改英文名'"
                @click="startNameEdit('name_en')"
              >
                <template v-if="editingField === 'name_en'">
                  <n-input
                    ref="nameEnEditRef"
                    v-model:value="inlineNameEn"
                    size="small"
                    placeholder="英文名（封面检索用，留空保存即清除）"
                    @blur="commitNameEdit"
                    @keyup.enter="commitNameEdit"
                    @keyup.esc="cancelNameEdit"
                    @click.stop
                  />
                </template>
                <template v-else>
                  <span>{{ selected.name_en || "—" }}</span>
                  <n-icon class="meta-edit-pen" :component="CreateOutline" :size="14" />
                </template>
              </div>
            </n-gi>

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
            <n-gi v-if="selectedPlayTime">
              <div class="meta-label">我的游玩时长</div>
              <div class="meta-value">
                {{ formatPlayTime(selectedPlayTime.play_time_seconds) }}
                <template v-if="selectedPlayTime.last_played">
                  <span class="last-played"> · 最近游玩 {{ formatDate(selectedPlayTime.last_played) }}</span>
                </template>
              </div>
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

          <!-- 评价：默认阅读态，点「编辑」才切输入框 -->
          <div class="detail-section">
            <div class="section-title">
              我的评价
              <div v-if="!reviewEditing" class="section-title-right">
                <span v-if="reviewCharCount > 0" class="section-count">{{ reviewCharCount }} 字</span>
                <!-- 阅读态看不到下面的保存状态行，落库回执挪到标题右，免得「点完成到底存没存」靠猜 -->
                <span
                  v-if="reviewSaveState === 'saved' && reviewSavedAtText"
                  class="section-count saved"
                >
                  已保存 · {{ reviewSavedAtText }}
                </span>
                <!-- 阅读态也必须看得见失败：关掉编辑框后若失败态被藏起来，
                     用户会以为内容已存好（2026-11-08 修复静默丢字） -->
                <span
                  v-if="reviewSaveState === 'error'"
                  class="section-count failed"
                  :title="reviewSaveError ?? ''"
                >
                  保存失败 · 内容已保留
                  <n-button text size="tiny" type="error" @click="saveReviewNow()">重试</n-button>
                </span>
                <n-button text size="tiny" type="primary" @click="startReviewEdit">
                  <template #icon>
                    <n-icon :component="CreateOutline" />
                  </template>
                  编辑
                </n-button>
              </div>
            </div>

            <template v-if="!reviewEditing">
              <!-- 空评价：给个顺手的入口，而不是一个空文本框 -->
              <button v-if="reviewBlocks.length === 0" class="review-empty" @click="startReviewEdit">
                还没写评价，点此开始写
              </button>
              <template v-else>
                <div
                  class="review-body"
                  :class="{ 'review-body--collapsed': reviewShouldCollapse && !reviewExpanded }"
                >
                  <template v-for="(block, bi) in reviewBlocks" :key="bi">
                    <ul v-if="block.type === 'list'" class="review-list">
                      <li v-for="(item, ii) in block.items" :key="ii">
                        <template v-for="(seg, si) in item" :key="si">
                          <strong v-if="seg.bold">{{ seg.text }}</strong>
                          <span v-else>{{ seg.text }}</span>
                        </template>
                      </li>
                    </ul>
                    <p v-else class="review-para">
                      <template v-for="(seg, si) in block.inlines" :key="si">
                        <strong v-if="seg.bold">{{ seg.text }}</strong>
                        <span v-else>{{ seg.text }}</span>
                      </template>
                    </p>
                  </template>
                </div>
                <button
                  v-if="reviewShouldCollapse"
                  class="review-toggle"
                  @click="reviewExpanded = !reviewExpanded"
                >
                  {{ reviewExpanded ? "收起" : "展开全文" }}
                </button>
              </template>
            </template>

            <template v-else>
              <n-input
                ref="reviewInputRef"
                v-model:value="reviewText"
                type="textarea"
                :autosize="REVIEW_AUTOSIZE"
                placeholder="写下你对这款游戏的评价…（空行分段、行首 - 列表、**加粗**）"
                @keydown="onReviewKeydown"
              />
              <div class="review-save-row">
                <span class="save-hint muted">{{ reviewCharCount }} 字</span>
                <div class="review-save-status">
                  <span
                    v-if="reviewSaveState === 'error'"
                    class="save-hint error"
                    :title="reviewSaveError ?? ''"
                  >
                    保存失败
                    <n-button text size="tiny" type="primary" @click="saveReviewNow()">
                      重试
                    </n-button>
                  </span>
                  <span v-else-if="reviewSaveState === 'saving'" class="save-hint">保存中…</span>
                  <span v-else-if="reviewSaveState === 'pending'" class="save-hint">待保存…</span>
                  <span v-else-if="reviewSaveState === 'saved'" class="save-hint saved">
                    已保存<template v-if="reviewSavedAtText"> · {{ reviewSavedAtText }}</template>
                  </span>
                  <span v-else class="save-hint muted">修改后自动保存</span>
                  <n-button size="tiny" type="primary" @click="finishReviewEdit">完成</n-button>
                </div>
              </div>
            </template>
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

.empty-error {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
  max-width: 420px;
  margin: 0 auto;
  font-size: 12px;
  color: #8a8f98;
  word-break: break-all;
}

/* 评价摘要：固定 2 行，无评价不渲染，让卡片高度差得最少 */
.card-summary {
  margin-top: 2px;
  font-size: 11px;
  line-height: 1.45;
  text-align: center;
  color: var(--text-color, rgba(255, 255, 255, 0.85));
  opacity: 0.5;
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  line-clamp: 2;
  overflow: hidden;
  word-break: break-word;
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

/* 「最近游玩」日期跟在时长后面，弱化一档不抢主值 */
.last-played {
  font-size: 12px;
  opacity: 0.65;
}

/* 可就地编辑的元数据值（中文名 / 英文名）：悬停浮现笔形图标，点击进入编辑态。
 *
 * 对齐口径（2026-10-05）：负外边距与内边距**严格相等**（各 4px），两者抵消后
 * 文字的左缘与相邻普通 `.meta-value` 逐像素对齐；`min-height` 取「标签行 +
 * 值行」的统一高度，避免本格比同排的普通格高出一截、把第二排顶下去。
 * 切勿只改内边距不改外边距（或反之），那会让本列文字整体左右偏移。
 */
.meta-editable {
  display: flex;
  align-items: center;
  gap: 4px;
  cursor: pointer;
  border-radius: 4px;
  padding: 1px 4px;
  margin: -1px -4px;
  min-height: 24px;
  transition: background-color 0.15s ease;
}

.meta-editable:hover {
  background: rgba(255, 255, 255, 0.08);
}

/* 笔形提示图标：平时隐藏，悬停才现身，避免干扰阅读 */
.meta-edit-pen {
  opacity: 0;
  flex-shrink: 0;
  transition: opacity 0.15s ease;
}

.meta-editable:hover .meta-edit-pen {
  opacity: 0.5;
}

/* 编辑态：输入框撑满，不再显示悬停底色 */
.meta-editable:has(.n-input) {
  cursor: default;
  background: transparent;
  padding: 0;
  margin: 0;
}

.meta-editable :deep(.n-input) {
  width: 100%;
}

:global(.light-theme) .meta-editable:hover {
  background: rgba(0, 0, 0, 0.06);
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

/* ==================== 评价：阅读态 ==================== */
.section-title-right {
  margin-left: auto;
  display: flex;
  align-items: center;
  gap: 10px;
  font-weight: 400;
}

.section-count {
  font-size: 12px;
  opacity: 0.45;
}

.section-count.saved {
  color: #36ad6a;
  opacity: 1;
}

.section-count.failed {
  color: #e88080;
  opacity: 1;
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.review-body {
  font-size: 13px;
  line-height: 1.75;
  /* 跟随 naive-ui 抽屉文字色，亮暗主题都不用改 */
  color: inherit;
  word-break: break-word;
}

/* 折叠：8 行高度（13px × 1.75 ≈ 22.75px）后渐隐，按钮把选择权交回用户 */
.review-body--collapsed {
  max-height: 182px;
  overflow: hidden;
  -webkit-mask-image: linear-gradient(to bottom, #000 62%, transparent 100%);
  mask-image: linear-gradient(to bottom, #000 62%, transparent 100%);
}

.review-para {
  margin: 0 0 10px;
  /* 段内单换行原样保留，不必往 DOM 里插 <br> */
  white-space: pre-wrap;
}

.review-para:last-child {
  margin-bottom: 0;
}

.review-list {
  margin: 0 0 10px;
  padding-left: 18px;
}

.review-list:last-child {
  margin-bottom: 0;
}

.review-list li {
  white-space: pre-wrap;
  margin-bottom: 2px;
}

.review-list li::marker {
  color: var(--accent-color, #6366f1);
}

.review-body strong {
  font-weight: 600;
}

.review-toggle,
.review-empty {
  border: none;
  background: transparent;
  padding: 0;
  cursor: pointer;
  font-family: inherit;
}

.review-toggle {
  margin-top: 8px;
  font-size: 12px;
  color: var(--accent-color, #6366f1);
}

.review-toggle:hover {
  text-decoration: underline;
}

.review-empty {
  font-size: 13px;
  line-height: 1.6;
  opacity: 0.5;
  text-align: left;
}

.review-empty:hover {
  opacity: 0.8;
}

/* ==================== 评价：编辑态 ==================== */
.review-save-row {
  margin-top: 8px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 8px;
  /* 固定高度：状态文字在「待保存 / 保存中 / 已保存」之间切换时不抖动 */
  min-height: 24px;
}

.review-save-status {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-left: auto;
}

.save-hint {
  font-size: 12px;
  color: #8a8f98;
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.save-hint.saved {
  color: #36ad6a;
}

.save-hint.error {
  color: #e88080;
}

.save-hint.muted {
  opacity: 0.55;
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
