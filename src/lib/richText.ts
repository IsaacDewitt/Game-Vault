/**
 * 手账评价的「极简 Markdown」渲染与摘要工具。
 *
 * 口径（2026-10-24）：
 * - 只认三样：空行分段、行首 `- ` / `* ` / `+ ` 列表、`**加粗**`；
 * - 其余语法（标题 / 表格 / 图片 / 链接 / 代码块 / 嵌套列表）一律原样当文本，
 *   不做半吊子猜测；
 * - **存储永远存用户敲的原文**，本模块只在阅读时穿衣。切回编辑看到的一定是原料，
 *   一个星号都不会被吃掉；
 * - 解析结果是结构化数据（块 → 内联片段），由模板 `v-for` 渲染，
 *   全程不碰 `innerHTML` / `v-html`，从根上不给出注入面。
 */

/** 段落/列表项内的一段文字；`bold` 恒有值，模板不用做 undefined 判断 */
export interface RichInline {
  text: string;
  bold: boolean;
}

export interface RichParagraph {
  type: "paragraph";
  inlines: RichInline[];
}

export interface RichList {
  type: "list";
  items: RichInline[][];
}

export type RichBlock = RichParagraph | RichList;

/** 行首列表标记：允许前导空格，标记后必须跟空白 */
const LIST_ITEM_RE = /^[ \t]*[-*+][ \t]+(.*)$/;

/**
 * 拆内联加粗。
 *
 * 手写扫描而非正则，为的是把「未闭合的 `**`」原样留在文本里——
 * 正则遇到半截源码很容易整段吞掉，用户写的星号会凭空消失。
 */
function parseInlines(text: string): RichInline[] {
  const out: RichInline[] = [];
  let buffer = "";
  let i = 0;

  while (i < text.length) {
    if (text[i] === "*" && text[i + 1] === "*") {
      const end = text.indexOf("**", i + 2);
      // `end > i + 2` 保证中间至少有一个字符（`****` 不算加粗）
      if (end > i + 2) {
        if (buffer) {
          out.push({ text: buffer, bold: false });
          buffer = "";
        }
        out.push({ text: text.slice(i + 2, end), bold: true });
        i = end + 2;
        continue;
      }
    }
    buffer += text[i];
    i += 1;
  }

  if (buffer) out.push({ text: buffer, bold: false });
  return out;
}

/**
 * 把原文解析成渲染块。
 *
 * 空行是唯一的分段信号：段内的单换行保留在文本里，靠容器 `white-space: pre-wrap`
 * 还原（这样既不用插 `<br>`，也不破坏加粗片段的分段）。
 */
export function parseRichText(src: string): RichBlock[] {
  const blocks: RichBlock[] = [];
  if (!src) return blocks;

  const lines = src.replace(/\r\n?/g, "\n").split("\n");
  let paragraph: string[] = [];
  let list: RichInline[][] | null = null;

  const flushParagraph = () => {
    if (paragraph.length === 0) return;
    blocks.push({ type: "paragraph", inlines: parseInlines(paragraph.join("\n")) });
    paragraph = [];
  };
  const flushList = () => {
    if (!list) return;
    blocks.push({ type: "list", items: list });
    list = null;
  };

  for (const raw of lines) {
    if (raw.trim() === "") {
      flushParagraph();
      flushList();
      continue;
    }

    const item = LIST_ITEM_RE.exec(raw);
    if (item) {
      flushParagraph();
      if (!list) list = [];
      list.push(parseInlines(item[1]));
      continue;
    }

    flushList();
    paragraph.push(raw);
  }

  flushParagraph();
  flushList();
  return blocks;
}

/** 字数：不计空白字符（空格 / 换行 / 制表符），按码点算，emoji 记 1 字 */
export function countChars(src: string): number {
  if (!src) return 0;
  return Array.from(src.replace(/\s/g, "")).length;
}

/**
 * 生成卡片墙摘要：抹掉 Markdown 记号、把换行压成空格、超长截断加省略号。
 *
 * 摘要只求「一眼知道写了什么」，不保留排版；因此列表符号也一并去掉，
 * 否则多条列表会连成 `· xxx · yyy` 的怪样子。
 */
export function previewText(src: string, maxLen = 60): string {
  if (!src) return "";
  const flat = src
    .replace(/\r\n?/g, "\n")
    .replace(/\*\*(.+?)\*\*/g, "$1")
    .replace(/^[ \t]*[-*+][ \t]+/gm, "")
    .replace(/\s+/g, " ")
    .trim();

  const chars = Array.from(flat);
  if (chars.length <= maxLen) return flat;
  return chars.slice(0, maxLen).join("").trimEnd() + "…";
}
