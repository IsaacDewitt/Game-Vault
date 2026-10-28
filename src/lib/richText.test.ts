import { describe, expect, it } from "vitest";
import { countChars, parseRichText, previewText } from "./richText";

/** 把解析结果压成便于断言的扁平形态 */
function shape(src: string) {
  return parseRichText(src).map((b) =>
    b.type === "paragraph"
      ? { type: "p", text: b.inlines.map((i) => (i.bold ? `**${i.text}**` : i.text)).join("") }
      : {
          type: "ul",
          items: b.items.map((item) =>
            item.map((i) => (i.bold ? `**${i.text}**` : i.text)).join("")
          ),
        }
  );
}

describe("parseRichText", () => {
  it("空输入不产出任何块", () => {
    expect(parseRichText("")).toEqual([]);
    expect(parseRichText("   \n\t\n ")).toEqual([]);
  });

  it("单段落原样保留（含段内换行）", () => {
    expect(shape("第一行\n第二行")).toEqual([{ type: "p", text: "第一行\n第二行" }]);
  });

  it("空行是唯一的分段信号", () => {
    expect(shape("甲\n\n乙")).toEqual([
      { type: "p", text: "甲" },
      { type: "p", text: "乙" },
    ]);
    // 连续空行不产生空段落
    expect(shape("甲\n\n\n\n乙")).toEqual([
      { type: "p", text: "甲" },
      { type: "p", text: "乙" },
    ]);
  });

  it("行首 - * + 归为同一列表", () => {
    expect(shape("- 甲\n* 乙\n+ 丙")).toEqual([{ type: "ul", items: ["甲", "乙", "丙"] }]);
  });

  it("列表与段落互相打断，不串味", () => {
    expect(shape("开场白\n- 甲\n- 乙\n收尾")).toEqual([
      { type: "p", text: "开场白" },
      { type: "ul", items: ["甲", "乙"] },
      { type: "p", text: "收尾" },
    ]);
  });

  it("空行拆开的是两个列表", () => {
    expect(shape("- 甲\n\n- 乙")).toEqual([
      { type: "ul", items: ["甲"] },
      { type: "ul", items: ["乙"] },
    ]);
  });

  it("** 包裹的内容标记为加粗，其余片段不动", () => {
    const blocks = parseRichText("**结论**：值 9 分");
    expect(blocks).toHaveLength(1);
    const p = blocks[0];
    expect(p.type).toBe("paragraph");
    if (p.type !== "paragraph") return;
    expect(p.inlines).toEqual([
      { text: "结论", bold: true },
      { text: "：值 9 分", bold: false },
    ]);
  });

  it("列表项内的加粗同样生效", () => {
    expect(shape("- **战斗**手感极好")).toEqual([{ type: "ul", items: ["**战斗**手感极好"] }]);
  });

  it("未闭合的 ** 原样保留，不被吞掉", () => {
    expect(shape("先说**结论")).toEqual([{ type: "p", text: "先说**结论" }]);
    expect(shape("**未闭合")).toEqual([{ type: "p", text: "**未闭合" }]);
    expect(shape("****")).toEqual([{ type: "p", text: "****" }]);
  });

  it("不认识的语法（标题 / 引用 / 表格）当普通文本", () => {
    expect(shape("# 标题\n> 引用\n| a | b |")).toEqual([
      { type: "p", text: "# 标题\n> 引用\n| a | b |" },
    ]);
  });

  it("CRLF 与 CR 都归一成换行", () => {
    expect(shape("甲\r\n\r\n乙")).toEqual([
      { type: "p", text: "甲" },
      { type: "p", text: "乙" },
    ]);
    expect(shape("甲\r乙")).toEqual([{ type: "p", text: "甲\n乙" }]);
  });
});

describe("countChars", () => {
  it("不计空白字符", () => {
    expect(countChars("")).toBe(0);
    expect(countChars("  \n\t ")).toBe(0);
    expect(countChars("空洞 骑士")).toBe(4);
  });

  it("emoji 按一个字算（按码点而非 UTF-16 长度）", () => {
    expect(countChars("🎮")).toBe(1);
  });
});

describe("previewText", () => {
  it("抹掉加粗与列表记号后压成一行", () => {
    expect(previewText("- **好**玩\n- 耐玩")).toBe("好玩 耐玩");
  });

  it("超过上限截断并加省略号", () => {
    const out = previewText("一".repeat(50), 10);
    expect(out).toBe("一".repeat(10) + "…");
  });

  it("恰好等于上限时不截断", () => {
    expect(previewText("一".repeat(10), 10)).toBe("一".repeat(10));
  });

  it("空输入返回空串", () => {
    expect(previewText("")).toBe("");
    expect(previewText("  \n ")).toBe("");
  });
});
