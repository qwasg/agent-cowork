import * as Y from "yjs";
import type {
  WordBlock,
  WordDocIR,
  WordMark,
  WordTextRun,
} from "../types.js";
import type { IdFactory } from "../ids.js";

/** 已知行内标记集合(其余 delta 属性视为非标记)。 */
const MARK_KEYS: WordMark[] = ["bold", "italic", "underline", "code", "strike"];

/** 取 Word 根 fragment。结构与 y-prosemirror 兼容:每个块是一个 XmlElement。 */
export function getWordRoot(doc: Y.Doc): Y.XmlFragment {
  return doc.getXmlFragment("word");
}

/** delta 段 -> WordTextRun。 */
function deltaToRuns(delta: Array<{ insert: unknown; attributes?: Record<string, unknown> }>): WordTextRun[] {
  const runs: WordTextRun[] = [];
  for (const op of delta) {
    if (typeof op.insert !== "string") continue;
    const attrs = op.attributes ?? {};
    const marks: WordMark[] = [];
    for (const m of MARK_KEYS) {
      if (attrs[m]) marks.push(m);
    }
    const run: WordTextRun = { text: op.insert };
    if (marks.length) run.marks = marks;
    if (typeof attrs.style === "string") run.style = attrs.style;
    runs.push(run);
  }
  return runs;
}

/** WordTextRun -> delta 属性。 */
function runAttributes(run: WordTextRun): Record<string, unknown> | undefined {
  const attrs: Record<string, unknown> = {};
  for (const m of run.marks ?? []) attrs[m] = true;
  if (run.style) attrs.style = run.style;
  return Object.keys(attrs).length ? attrs : undefined;
}

/** 读取单个块 XmlElement -> WordBlock。 */
export function readBlock(el: Y.XmlElement): WordBlock {
  const id = el.getAttribute("id") ?? "";
  const type = (el.nodeName === "heading" ? "heading" : "paragraph") as WordBlock["type"];
  const levelAttr = el.getAttribute("level");
  const styleAttr = el.getAttribute("style");

  // 收集 XmlText 子节点的 delta
  let runs: WordTextRun[] = [];
  for (let i = 0; i < el.length; i++) {
    const child = el.get(i) as Y.XmlText | Y.XmlElement | undefined;
    if (child instanceof Y.XmlText) {
      runs = runs.concat(deltaToRuns(child.toDelta()));
    }
  }

  const block: WordBlock = { id, type, runs };
  if (type === "heading") block.level = levelAttr ? Number(levelAttr) : 1;
  if (styleAttr) block.style = styleAttr;
  return block;
}

/** 读取整个 Word 文档 IR。 */
export function readWordIR(doc: Y.Doc): WordDocIR {
  const root = getWordRoot(doc);
  const blocks: WordBlock[] = [];
  for (let i = 0; i < root.length; i++) {
    const el = root.get(i);
    if (el instanceof Y.XmlElement) blocks.push(readBlock(el));
  }
  return { type: "word", blocks };
}

/** 由 WordBlock 构造一个 XmlElement(含 id 等属性与文本 runs)。 */
export function buildBlockElement(block: WordBlock, idFactory: IdFactory): Y.XmlElement {
  const el = new Y.XmlElement(block.type);
  el.setAttribute("id", block.id || idFactory("blk"));
  if (block.type === "heading") el.setAttribute("level", String(block.level ?? 1));
  if (block.style) el.setAttribute("style", block.style);

  const text = new Y.XmlText();
  let cursor = 0;
  for (const run of block.runs) {
    if (!run.text) continue;
    text.insert(cursor, run.text, runAttributes(run));
    cursor += run.text.length;
  }
  el.insert(0, [text]);
  return el;
}

/** 在 fragment 中按 id 找块索引;找不到返回 -1。 */
export function findBlockIndex(root: Y.XmlFragment, id: string): number {
  for (let i = 0; i < root.length; i++) {
    const el = root.get(i);
    if (el instanceof Y.XmlElement && el.getAttribute("id") === id) return i;
  }
  return -1;
}

/** 取块的(首个)XmlText 子节点。 */
export function getBlockText(el: Y.XmlElement): Y.XmlText | undefined {
  for (let i = 0; i < el.length; i++) {
    const child = el.get(i);
    if (child instanceof Y.XmlText) return child;
  }
  return undefined;
}

export { MARK_KEYS, runAttributes };
