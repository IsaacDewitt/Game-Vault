import { defineStore } from "pinia";
import { ref, computed } from "vue";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { Review } from "../lib/tauri";
import * as api from "../lib/tauri";

export const useReviewsStore = defineStore("reviews", () => {
  // 状态
  const reviews = ref<Review[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);
  const selectedReview = ref<Review | null>(null);
  // 正在刷新信息的条目 id 集合
  const refreshingIds = ref<Set<string>>(new Set());
  // 筛选与排序状态
  const searchQuery = ref("");
  const statusFilter = ref("");
  const genreFilter = ref("");
  const sortBy = ref("added_at");
  const sortOrder = ref("desc");

  // 计算属性：前端过滤 + 排序
  const filteredReviews = computed(() => {
    let result = [...reviews.value];

    // 搜索
    if (searchQuery.value) {
      const query = searchQuery.value.toLowerCase();
      result = result.filter((r) => r.name.toLowerCase().includes(query));
    }

    // 状态筛选
    if (statusFilter.value) {
      result = result.filter((r) => r.status === statusFilter.value);
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
        result.sort((a, b) => a.name.localeCompare(b.name, "zh-Hans-CN") * order);
        break;
      case "rating":
        result.sort((a, b) => {
          if (a.rating == null && b.rating == null) return 0;
          if (a.rating == null) return 1; // 未评分恒排最后
          if (b.rating == null) return -1;
          return (a.rating - b.rating) * order;
        });
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

  /** 将封面本地路径转为 asset URL */
  function coverSrc(review: Review): string | null {
    const p = review.cover_local || review.cover_url;
    if (!p) return null;
    try {
      return convertFileSrc(p);
    } catch (e) {
      console.error("转换封面路径失败:", review.id, e);
      return null;
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

  /** 添加手账条目（只填名字），随后自动刷新元数据与封面 */
  async function addReview(name: string) {
    const review = await api.addReview(name);
    reviews.value.unshift(review);
    // 自动刷新信息（LLM + 封面），失败不阻断添加
    refreshReviewInfo(review.id).catch(() => {});
    return review;
  }

  /** 从游戏库导入，随后自动刷新缺失信息 */
  async function importFromGame(gameId: string) {
    const review = await api.importReviewFromGame(gameId);
    reviews.value.unshift(review);
    return review;
  }

  /** 刷新单个条目的元数据与封面 */
  async function refreshReviewInfo(reviewId: string) {
    refreshingIds.value.add(reviewId);
    try {
      const updated = await api.refreshReviewInfo(reviewId);
      updateReviewInStore(reviewId, updated);
      return updated;
    } finally {
      refreshingIds.value.delete(reviewId);
    }
  }

  /** 手动更新元数据（含改名） */
  async function updateMeta(reviewId: string, meta: api.ReviewMetaInput) {
    const updated = await api.updateReviewMeta(reviewId, meta);
    updateReviewInStore(reviewId, updated);
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

  /** 设置状态 */
  async function setStatus(reviewId: string, status: string) {
    const updated = await api.setReviewStatus(reviewId, status);
    updateReviewInStore(reviewId, updated);
    return updated;
  }

  /** 删除手账条目 */
  async function removeReview(reviewId: string) {
    await api.deleteReview(reviewId);
    reviews.value = reviews.value.filter((r) => r.id !== reviewId);
    if (selectedReview.value?.id === reviewId) {
      selectedReview.value = null;
    }
  }

  async function fetchCoverOptions(reviewId: string): Promise<api.CoverOption[]> {
    return api.fetchReviewCoverOptions(reviewId);
  }

  async function setCoverFromUrl(reviewId: string, url: string) {
    await api.setReviewCoverFromUrl(reviewId, url);
    const updated = await api.getReviewDetail(reviewId);
    if (updated) {
      updateReviewInStore(reviewId, updated);
    }
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
    statusFilter,
    genreFilter,
    sortBy,
    sortOrder,
    filteredReviews,
    coverSrc,
    loadReviews,
    addReview,
    importFromGame,
    refreshReviewInfo,
    updateMeta,
    setRating,
    setReview,
    setStatus,
    removeReview,
    fetchCoverOptions,
    setCoverFromUrl,
    selectReview,
    clearSelection,
    updateReviewInStore,
  };
});
