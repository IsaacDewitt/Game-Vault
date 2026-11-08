import { defineStore } from "pinia";
import { ref, computed } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { Review } from "../lib/tauri";
import * as api from "../lib/tauri";
import { displayName } from "../lib/format";
import {
  draftFor,
  pruneDrafts,
  putDraft,
  removeDraft,
  type ReviewDraft,
  type ReviewDraftBook,
} from "../lib/reviewDrafts";

/**
 * 界面展示名：英文名优先（中英文名都搜，展示纯英文策略）。
 *
 * 实现与游戏库共用 `lib/format.ts` 的 `displayName`（原先两处各写一份，2026-10-17 合并）；
 * 此处 re-export 保持 `ReviewsView.vue` 等既有导入路径不变。
 */
export { displayName };

export const useReviewsStore = defineStore("reviews", () => {
  // 状态
  const reviews = ref<Review[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);
  const selectedReview = ref<Review | null>(null);
  // 正在刷新信息的条目 id 集合
  const refreshingIds = ref<Set<string>>(new Set());
  // 封面路径映射（键 `review:<id>`），含封面行的 updated_at 供破缓存用
  const coverPaths = ref<Record<string, api.CoverSet>>({});
  // 写库失败的托底草稿（按 reviewId 存；放 store 里，组件卸载也不丢）
  const unsavedDrafts = ref<ReviewDraftBook>({});
  function saveUnsavedDraft(reviewId: string, draft: ReviewDraft) {
    unsavedDrafts.value = putDraft(unsavedDrafts.value, reviewId, draft);
  }
  function getUnsavedDraft(reviewId: string) {
    return draftFor(unsavedDrafts.value, reviewId);
  }
  function clearUnsavedDraft(reviewId: string) {
    unsavedDrafts.value = removeDraft(unsavedDrafts.value, reviewId);
  }
  // 游玩时长映射（键 review id）：按名匹配游戏库得来，含已删游戏的历史（2026-10-24）
  const playTimes = ref<Record<string, api.ReviewPlayTime>>({});
  // 筛选与排序状态
  const searchQuery = ref("");
  const genreFilter = ref("");
  const sortBy = ref("added_at");
  const sortOrder = ref("desc");

  // 计算属性：前端过滤 + 排序
  const filteredReviews = computed(() => {
    let result = [...reviews.value];

    // 搜索（中英文名 + 评价正文，三处都匹配）
    if (searchQuery.value) {
      const query = searchQuery.value.toLowerCase();
      result = result.filter(
        (r) =>
          r.name.toLowerCase().includes(query) ||
          (r.name_en || "").toLowerCase().includes(query) ||
          // 评价写得越长越难靠回忆找条目，正文一并纳入检索（2026-10-24）
          (r.review || "").toLowerCase().includes(query)
      );
    }

    // 类型筛选
    if (genreFilter.value) {
      const genre = genreFilter.value.toLowerCase();
      result = result.filter((r) =>
        r.genres.some((g) => g.toLowerCase().includes(genre))
      );
    }

    // 排序
    const order = sortOrder.value === "asc" ? 1 : -1;
    switch (sortBy.value) {
      case "name":
        // 展示名排序（英文名优先，与界面显示一致）
        result.sort(
          (a, b) =>
            displayName(a).localeCompare(displayName(b), "zh-Hans-CN") * order
        );
        break;
      case "rating":
        result.sort((a, b) => {
          if (a.rating == null && b.rating == null) return 0;
          if (a.rating == null) return 1; // 未评分恒排最后
          if (b.rating == null) return -1;
          return (a.rating - b.rating) * order;
        });
        break;
      case "play_time":
        // 无游玩历史按 0 算，与游戏库 play_time 排序同口径（gameFilter.ts）
        result.sort(
          (a, b) =>
            ((playTimes.value[a.id]?.play_time_seconds ?? 0) -
              (playTimes.value[b.id]?.play_time_seconds ?? 0)) *
            order
        );
        break;
      case "updated_at":
        result.sort((a, b) =>
          ((a.updated_at || "").localeCompare(b.updated_at || "")) * order
        );
        break;
      default: // added_at
        result.sort((a, b) => a.added_at.localeCompare(b.added_at) * order);
    }

    return result;
  });

  /**
   * 将封面本地路径转为 asset URL，并拼破缓存参数防 WebView 命中旧图。
   *
   * 参数优先取**封面行**的 `updated_at`（`getAllCovers` 带回，键 `review:<id>`）——
   * 它只在封面真被替换时才变。取不到时回退 `review.updated_at`（旧行为）：
   * 后者偏"吵"，改评分、改评价、改名都会刷新它，会让 URL 无谓抖动、白烧一次
   * 图片解码（2026-09-24 收敛）。
   */
  function coverSrc(review: Review): string | null {
    const p = review.cover_local || review.cover_url;
    if (!p) return null;
    try {
      const url = convertFileSrc(p);
      const entry = coverPaths.value[api.coverKey("review", review.id)];
      const t = entry?.updated_at || review.updated_at || review.id;
      return `${url}${url.includes("?") ? "&" : "?"}t=${encodeURIComponent(t)}`;
    } catch (e) {
      console.error("转换封面路径失败:", review.id, e);
      return null;
    }
  }

  /** 拉取封面路径映射（含手账条目的封面行时间戳，供破缓存用） */
  async function loadAllCovers() {
    try {
      coverPaths.value = await api.getAllCovers();
    } catch (e) {
      console.error("加载封面路径失败:", e);
    }
  }

  /**
   * 拉取游玩时长映射（按名匹配游戏库，含已删游戏历史）。
   * 匹配口径是名字，故改名/导入后必须重拉；失败不影响列表展示，只是这一栏缺失。
   */
  async function loadPlayTimes() {
    try {
      playTimes.value = await api.getReviewPlayTimes();
    } catch (e) {
      console.error("加载游玩时长失败:", e);
    }
  }

  // 加载列表
  let loadLock = false;
  let pendingRefresh = false;
  async function loadReviews() {
    if (loadLock) {
      pendingRefresh = true;
      return;
    }
    loadLock = true;
    loading.value = true;
    error.value = null;
    try {
      reviews.value = await api.getReviews();
      // 条目可能被别的入口删掉，顺手清掉永远不会被认领的失败草稿。
      unsavedDrafts.value = pruneDrafts(
        unsavedDrafts.value,
        reviews.value.map((r) => r.id),
      );
      await loadAllCovers();
      await loadPlayTimes();
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      error.value = `加载手账列表失败: ${msg}`;
      console.error("加载手账列表失败:", e);
    } finally {
      loading.value = false;
      loadLock = false;
      if (pendingRefresh) {
        pendingRefresh = false;
        loadReviews();
      }
    }
  }

  /**
   * 添加手账条目（只填名字）：**纯本地入库，不发任何网络请求**（2026-11-08 按老爷要求改）。
   *
   * - 2026-10-30 砍掉自动拉封面：旧实现连带抓一张 SteamGridDB 的图，抓回来是什么全凭运气；
   *   现在封面三个来源（本地 / Steam 官方 / SteamGridDB）全归用户手动挑选。
   * - 2026-11-08 砍掉自动 LLM 补全：与游戏库 `addGameManual` 同一轮拍板，
   *   「添加」一律不发请求，元数据由详情页的「获取信息」手动触发。
   *
   * `loadPlayTimes()` 保留：它只读本地库做按名匹配，不出网。
   */
  async function addReview(name: string) {
    const review = await api.addReview(name);
    reviews.value.unshift(review);
    // 新名字可能直接对上游戏库的时长，补拉一次本地映射
    loadPlayTimes().catch(() => {});
    return review;
  }

  /**
   * 从游戏库导入条目。
   * 元数据与封面由后端在导入时**一并复用**（后端 import_review_from_game 内做封面共享），
   * 因此这里**不**再自动刷新——不要照 addReview 的写法补一个 refreshReviewInfo，
   * 那会多打一次无谓的 LLM / SteamGridDB 请求。
   */
  async function importFromGame(gameId: string) {
    const review = await api.importReviewFromGame(gameId);
    reviews.value.unshift(review);
    // 新条目按名即可匹配上游游戏的时长，补拉一次
    loadPlayTimes().catch(() => {});
    return review;
  }

  /** 刷新单个条目的元数据（只跑 LLM；封面自 2026-10-30 起由「更换封面」手动指定） */
  async function refreshReviewInfo(reviewId: string) {
    refreshingIds.value.add(reviewId);
    try {
      const updated = await api.refreshReviewInfo(reviewId);
      updateReviewInStore(reviewId, updated);
      // LLM 刷新也会改写 name / name_en，而游玩时长是按名认领的：
      // 不补拉的话，「我的游玩时长」会继续照着旧名字匹配（2026-10-28 审查指出）
      loadPlayTimes().catch(() => {});
      return updated;
    } finally {
      refreshingIds.value.delete(reviewId);
    }
  }

  /** 手动更新元数据（含改名） */
  async function updateMeta(reviewId: string, meta: api.ReviewMetaInput) {
    const updated = await api.updateReviewMeta(reviewId, meta);
    updateReviewInStore(reviewId, updated);
    // 改名会改变按名匹配的结果，重拉时长映射（懒惰点只影响这一栏，无所谓空跑）
    if (meta.name !== undefined || meta.name_en !== undefined) {
      loadPlayTimes().catch(() => {});
    }
    return updated;
  }

  /** 设置评分（0-10，null 清除） */
  async function setRating(reviewId: string, rating: number | null) {
    const updated = await api.setReviewRating(reviewId, rating);
    updateReviewInStore(reviewId, updated);
    return updated;
  }

  /** 设置评价 */
  async function setReview(reviewId: string, reviewText: string | null) {
    const updated = await api.setReviewReview(reviewId, reviewText);
    updateReviewInStore(reviewId, updated);
    return updated;
  }

  /** 指定截图目录（null 清除，回到自动推断） */
  async function setScreenshotDir(reviewId: string, dir: string | null) {
    const updated = await api.setReviewScreenshotDir(reviewId, dir);
    updateReviewInStore(reviewId, updated);
    return updated;
  }

  /** 删除手账条目。keepCover 默认 true：封面挪进 covers/archive 留档 */
  async function removeReview(reviewId: string, keepCover = true) {
    await api.deleteReview(reviewId, keepCover);
    reviews.value = reviews.value.filter((r) => r.id !== reviewId);
    clearUnsavedDraft(reviewId);
    if (selectedReview.value?.id === reviewId) {
      selectedReview.value = null;
    }
  }

  async function fetchCoverOptions(reviewId: string): Promise<api.CoverOption[]> {
    return api.fetchReviewCoverOptions(reviewId);
  }

  /**
   * 换封面后的统一收尾：条目字段（`cover_local`）与封面映射都得重拉。
   *
   * 映射里的时间戳是 asset URL 的破缓存参数，不重拉的话 WebView 仍显示旧图。
   */
  async function syncCoverState(reviewId: string) {
    const updated = await api.getReviewDetail(reviewId);
    if (updated) {
      updateReviewInStore(reviewId, updated);
    }
    await loadAllCovers();
  }

  async function setCoverFromUrl(reviewId: string, url: string) {
    await api.setReviewCoverFromUrl(reviewId, url);
    await syncCoverState(reviewId);
  }

  /** 本地图片当封面（只把内容收进封面库，用户选的原文件不动） */
  async function setCoverFromLocal(reviewId: string, coverPath: string) {
    await api.setReviewCoverFromLocal(reviewId, coverPath);
    await syncCoverState(reviewId);
  }

  /** Steam 官方封面（appid 先按名认领游戏库，认不到再查 Steam 商店搜索） */
  async function setCoverFromSteam(reviewId: string) {
    await api.setReviewCoverFromSteam(reviewId);
    await syncCoverState(reviewId);
  }

  function selectReview(review: Review) {
    selectedReview.value = review;
  }

  function clearSelection() {
    selectedReview.value = null;
  }

  /** 更新 store 中指定条目 */
  function updateReviewInStore(reviewId: string, updated: Review) {
    const idx = reviews.value.findIndex((r) => r.id === reviewId);
    if (idx !== -1) {
      reviews.value[idx] = updated;
    }
    if (selectedReview.value?.id === reviewId) {
      selectedReview.value = updated;
    }
  }

  return {
    reviews,
    loading,
    error,
    selectedReview,
    refreshingIds,
    searchQuery,
    genreFilter,
    sortBy,
    sortOrder,
    filteredReviews,
    coverSrc,
    coverPaths,
    playTimes,
    unsavedDrafts,
    saveUnsavedDraft,
    getUnsavedDraft,
    clearUnsavedDraft,
    loadAllCovers,
    loadPlayTimes,
    loadReviews,
    addReview,
    importFromGame,
    refreshReviewInfo,
    updateMeta,
    setRating,
    setReview,
    setScreenshotDir,
    removeReview,
    fetchCoverOptions,
    setCoverFromUrl,
    setCoverFromLocal,
    setCoverFromSteam,
    selectReview,
    clearSelection,
    updateReviewInStore,
  };
});
