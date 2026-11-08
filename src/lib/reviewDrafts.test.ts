import { describe, it, expect } from "vitest";
import { draftFor, pruneDrafts, putDraft, removeDraft } from "./reviewDrafts";

describe("reviewDrafts", () => {
  it("按 reviewId 保留多条失败草稿，互不覆盖", () => {
    let book = putDraft({}, "a", { text: "A 未保存", error: "db locked" });
    book = putDraft(book, "b", { text: "B 未保存", error: "disk full" });

    expect(draftFor(book, "a")).toEqual({ text: "A 未保存", error: "db locked" });
    expect(draftFor(book, "b")).toEqual({ text: "B 未保存", error: "disk full" });
  });

  it("清除一条草稿不影响其它条目，且无变化时保持引用", () => {
    let book = putDraft({}, "a", { text: "A", error: "e1" });
    book = putDraft(book, "b", { text: "B", error: "e2" });
    const before = book;

    book = removeDraft(book, "a");
    expect(draftFor(book, "a")).toBeNull();
    expect(draftFor(book, "b")).toEqual({ text: "B", error: "e2" });
    expect(removeDraft(book, "missing")).toBe(book);
    expect(before.a.text).toBe("A");
  });

  it("条目删除后清掉孤儿草稿", () => {
    let book = putDraft({}, "gone", { text: "旧条目", error: "e" });
    book = putDraft(book, "live", { text: "活条目", error: "e" });
    const before = book;

    book = pruneDrafts(book, ["live"]);
    expect(draftFor(book, "gone")).toBeNull();
    expect(draftFor(book, "live")?.text).toBe("活条目");
    expect(pruneDrafts(book, ["live"])).toBe(book);
    expect(before.gone.text).toBe("旧条目");
  });
});
