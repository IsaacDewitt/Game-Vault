/**
 * 手账评价「写库失败后的托底草稿簿」（纯逻辑，有 vitest 单测）。
 *
 * 旧版只在 ReviewsView 组件里放一个 `unsavedBuffer` 槽位：两个条目连续保存
 * 失败会互相覆盖；组件卸载后槽位随实例消失。这里按 reviewId 存多份，放在
 * Pinia store 中跨视图生命周期保留，删除条目时按现有 id 清理。
 */
export interface ReviewDraft {
  text: string;
  error: string;
}

export type ReviewDraftBook = Record<string, ReviewDraft>;

/** 写入/覆盖某一条目的失败草稿；不修改入参。 */
export function putDraft(
  book: ReviewDraftBook,
  reviewId: string,
  draft: ReviewDraft,
): ReviewDraftBook {
  if (!reviewId) return book;
  return { ...book, [reviewId]: draft };
}

/** 读取某一条目的失败草稿；不存在返回 null。 */
export function draftFor(book: ReviewDraftBook, reviewId: string): ReviewDraft | null {
  return book[reviewId] ?? null;
}

/** 清除某一条目的失败草稿；本就不存在时保持同一对象引用。 */
export function removeDraft(book: ReviewDraftBook, reviewId: string): ReviewDraftBook {
  if (!(reviewId in book)) return book;
  const next = { ...book };
  delete next[reviewId];
  return next;
}

/** 条目被删除后清掉永远不会再被认领的草稿。 */
export function pruneDrafts(
  book: ReviewDraftBook,
  liveReviewIds: readonly string[],
): ReviewDraftBook {
  const live = new Set(liveReviewIds);
  let changed = false;
  const next: ReviewDraftBook = {};
  for (const [id, draft] of Object.entries(book)) {
    if (live.has(id)) {
      next[id] = draft;
    } else {
      changed = true;
    }
  }
  return changed ? next : book;
}
