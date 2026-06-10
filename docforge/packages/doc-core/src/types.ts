/**
 * DocForge-Core IR 类型定义。
 *
 * IR 即文档(单一事实源)。Word 与 PPT 两种 doc-type 共用一套对外契约。
 * 这里的类型是序列化快照(read_document 的返回形态),Yjs 是其活体表示。
 */

export type DocType = "word" | "ppt";

/* ----------------------------- Word IR ----------------------------- */

/** 文本行内标记。与 ProseMirror mark 一一对应,便于 y-prosemirror 绑定。 */
export type WordMark = "bold" | "italic" | "underline" | "code" | "strike";

/** 一段连续、样式一致的文本(run)。 */
export interface WordTextRun {
  text: string;
  marks?: WordMark[];
  /** 字符级命名样式(如 "accent"),映射到 OOXML character style。 */
  style?: string;
}

export type WordBlockType = "paragraph" | "heading";

/** Word 文档的块级节点(段落 / 标题)。 */
export interface WordBlock {
  id: string;
  type: WordBlockType;
  /** heading 的级别 1-6;paragraph 忽略。 */
  level?: number;
  /** 块级命名样式(如 "Quote"),映射到 OOXML paragraph style。 */
  style?: string;
  runs: WordTextRun[];
}

export interface WordDocIR {
  type: "word";
  blocks: WordBlock[];
}

/* ------------------------------ PPT IR ----------------------------- */

/** 元素几何。坐标单位为 EMU-无关的逻辑英寸(导出时换算)。 */
export interface Geo {
  x: number;
  y: number;
  w: number;
  h: number;
  /** 旋转角度(度)。 */
  rot?: number;
}

export type ElementType = "text" | "shape" | "image";

export interface SlideElement {
  id: string;
  type: ElementType;
  geo: Geo;
  /**
   * 类型相关属性:
   *  - text:  { text, fontSize?, color?, bold?, italic?, align? }
   *  - shape: { shape, fill?, line? }
   *  - image: { src, alt? }
   */
  props: Record<string, unknown>;
}

export interface Slide {
  id: string;
  /** 版式名(blank / title / titleBody / twoContent 等)。 */
  layout: string;
  elements: SlideElement[];
}

export interface PptDocIR {
  type: "ppt";
  slides: Slide[];
}

/* ------------------------------ Union ------------------------------ */

export type DocIR = WordDocIR | PptDocIR;

/* --------------------------- API I/O 类型 -------------------------- */

/** insert_block 入参:不含 id(由引擎分配)。 */
export interface NewWordBlock {
  type: WordBlockType;
  level?: number;
  style?: string;
  /** 便捷写法:纯文本;与 runs 二选一。 */
  text?: string;
  runs?: WordTextRun[];
}

/** apply_style 入参。布尔标记 + 可选块/字符样式名。 */
export interface StyleInput {
  bold?: boolean;
  italic?: boolean;
  underline?: boolean;
  code?: boolean;
  strike?: boolean;
  /** 命名样式;word 块上写到 block.style。 */
  style?: string;
}

export interface OutlineItem {
  id: string;
  level: number;
  text: string;
}

export type Outline = OutlineItem[];

export type ExportFormat = "docx" | "pptx" | "json";

/** 任一 mutation 后触发的事件。 */
export interface DocChangeEvent {
  /** 触发来源:本地 mutation / 远端同步 / 初始化。 */
  origin: "local" | "remote" | "init";
  /** 最新文档内容哈希,便于编译/预览缓存判定。 */
  hash: string;
}

export type ObserveCallback = (event: DocChangeEvent) => void;

/** export 委托给上层注入的编译器(避免 doc-core 依赖 compile-engine)。 */
export type Exporter = (ir: DocIR, format: ExportFormat) => Promise<string>;
