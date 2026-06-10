import * as Y from "yjs";
import type {
  DocChangeEvent,
  DocIR,
  DocType,
  Exporter,
  ExportFormat,
  Geo,
  NewWordBlock,
  ObserveCallback,
  Outline,
  SlideElement,
  StyleInput,
  WordBlock,
} from "./types.js";
import { defaultIdFactory, type IdFactory } from "./ids.js";
import { contentHash } from "./hash.js";
import { getLayoutSeeds } from "./layouts.js";
import * as WordBind from "./binding/word.js";
import * as PptBind from "./binding/ppt.js";

/** 标识本地 mutation 的事务来源,用于区分 local / remote。 */
export const LOCAL_ORIGIN = Symbol("docforge.local");

export interface DocCoreOptions {
  type: DocType;
  /** 复用已存在的 Y.Doc(如 sync 场景);否则新建。 */
  doc?: Y.Doc;
  idFactory?: IdFactory;
  /** export 委托;不提供则调用 export 时抛错。 */
  exporter?: Exporter;
}

/**
 * DocCore —— 文档的唯一写入口与读取面。
 *
 * UI 与 agent 都只调这套契约,不直接碰 Yjs/OOXML。
 * compile/preview 仅通过 read_document()/observe() 只读访问。
 */
export class DocCore {
  readonly type: DocType;
  readonly doc: Y.Doc;
  private readonly idFactory: IdFactory;
  private readonly exporter?: Exporter;
  private readonly callbacks = new Set<ObserveCallback>();
  private updateHandler?: (update: Uint8Array, origin: unknown) => void;

  constructor(options: DocCoreOptions) {
    this.type = options.type;
    this.doc = options.doc ?? new Y.Doc();
    this.idFactory = options.idFactory ?? defaultIdFactory;
    this.exporter = options.exporter;
    this.attach();
  }

  /* ----------------------------- 读取 ----------------------------- */

  read_document(): DocIR {
    return this.type === "word"
      ? WordBind.readWordIR(this.doc)
      : PptBind.readPptIR(this.doc);
  }

  get_outline(): Outline {
    if (this.type === "word") {
      const ir = WordBind.readWordIR(this.doc);
      const out: Outline = [];
      for (const b of ir.blocks) {
        if (b.type !== "heading") continue;
        out.push({
          id: b.id,
          level: b.level ?? 1,
          text: b.runs.map((r) => r.text).join(""),
        });
      }
      return out;
    }
    const ir = PptBind.readPptIR(this.doc);
    return ir.slides.map((s, i) => {
      const titleEl = s.elements.find(
        (e) => e.type === "text" && typeof e.props.text === "string",
      );
      return {
        id: s.id,
        level: 1,
        text: titleEl ? String(titleEl.props.text) : `Slide ${i + 1}`,
      };
    });
  }

  /** 当前内容哈希(供编译/预览缓存)。 */
  hash(): string {
    return contentHash(this.read_document());
  }

  /* ------------------------- Word mutation ------------------------ */

  insert_block(afterId: string | null, node: NewWordBlock): string {
    this.assertType("word");
    const id = this.idFactory("blk");
    const block: WordBlock = {
      id,
      type: node.type,
      level: node.level,
      style: node.style,
      runs: node.runs ?? (node.text !== undefined ? [{ text: node.text }] : []),
    };
    this.transact(() => {
      const root = WordBind.getWordRoot(this.doc);
      const el = WordBind.buildBlockElement(block, this.idFactory);
      let index: number;
      if (afterId === null || afterId === undefined) {
        index = 0;
      } else {
        const found = WordBind.findBlockIndex(root, afterId);
        if (found < 0) throw new Error(`insert_block: afterId 未找到: ${afterId}`);
        index = found + 1;
      }
      root.insert(index, [el]);
    });
    return id;
  }

  replace_text(rangeId: string, text: string): void {
    this.assertType("word");
    this.transact(() => {
      const root = WordBind.getWordRoot(this.doc);
      const idx = WordBind.findBlockIndex(root, rangeId);
      if (idx < 0) throw new Error(`replace_text: rangeId 未找到: ${rangeId}`);
      const el = root.get(idx) as Y.XmlElement;
      let ytext = WordBind.getBlockText(el);
      if (!ytext) {
        ytext = new Y.XmlText();
        el.insert(el.length, [ytext]);
      }
      if (ytext.length) ytext.delete(0, ytext.length);
      if (text) ytext.insert(0, text);
    });
  }

  apply_style(rangeId: string, style: StyleInput): void {
    this.assertType("word");
    this.transact(() => {
      const root = WordBind.getWordRoot(this.doc);
      const idx = WordBind.findBlockIndex(root, rangeId);
      if (idx < 0) throw new Error(`apply_style: rangeId 未找到: ${rangeId}`);
      const el = root.get(idx) as Y.XmlElement;

      const attrs: Record<string, unknown> = {};
      for (const k of WordBind.MARK_KEYS) {
        if (k in style) attrs[k] = (style as Record<string, unknown>)[k] ? true : null;
      }
      if (style.style !== undefined) attrs.style = style.style;

      const ytext = WordBind.getBlockText(el);
      if (ytext && ytext.length && Object.keys(attrs).length) {
        ytext.format(0, ytext.length, attrs);
      }
      // 命名样式同时落到块级属性,便于 OOXML 段落样式映射
      if (style.style !== undefined) el.setAttribute("style", style.style);
    });
  }

  /* ------------------------- PPT mutation ------------------------- */

  add_slide(index: number, layout: string): string {
    this.assertType("ppt");
    const slideId = this.idFactory("sld");
    this.transact(() => {
      const root = PptBind.getPptRoot(this.doc);
      const seeds = getLayoutSeeds(layout);
      const slideMap = PptBind.buildSlide({
        id: slideId,
        layout,
        elements: seeds.map((s) => ({
          id: this.idFactory("el"),
          type: s.type,
          geo: { ...s.geo },
          props: { ...s.props, role: s.role },
        })),
      });
      const at = Math.max(0, Math.min(index, root.length));
      root.insert(at, [slideMap]);
    });
    return slideId;
  }

  edit_element(slideId: string, elId: string, props: Record<string, unknown>): void {
    this.assertType("ppt");
    this.transact(() => {
      const root = PptBind.getPptRoot(this.doc);
      const sIdx = PptBind.findSlideIndex(root, slideId);
      if (sIdx < 0) throw new Error(`edit_element: slideId 未找到: ${slideId}`);
      const el = PptBind.findElement(root.get(sIdx), elId);
      if (!el) throw new Error(`edit_element: elId 未找到: ${elId}`);
      const propsMap = el.get("props") as Y.Map<unknown>;
      for (const [k, v] of Object.entries(props)) {
        if (v === undefined) propsMap.delete(k);
        else propsMap.set(k, v);
      }
    });
  }

  move_element(slideId: string, elId: string, geo: Partial<Geo>): void {
    this.assertType("ppt");
    this.transact(() => {
      const root = PptBind.getPptRoot(this.doc);
      const sIdx = PptBind.findSlideIndex(root, slideId);
      if (sIdx < 0) throw new Error(`move_element: slideId 未找到: ${slideId}`);
      const el = PptBind.findElement(root.get(sIdx), elId);
      if (!el) throw new Error(`move_element: elId 未找到: ${elId}`);
      const geoMap = el.get("geo") as Y.Map<unknown>;
      for (const k of ["x", "y", "w", "h", "rot"] as const) {
        const v = geo[k];
        if (v !== undefined) geoMap.set(k, v);
      }
    });
  }

  /* ------------------------------ export -------------------------- */

  async export(format: ExportFormat): Promise<string> {
    if (!this.exporter) {
      throw new Error(
        "export: 未配置 exporter。请在创建 DocCore 时注入 compile-engine 的 exporter。",
      );
    }
    return this.exporter(this.read_document(), format);
  }

  /* ----------------------------- observe -------------------------- */

  /** 订阅任意 mutation(本地或远端)。返回取消订阅函数。 */
  observe(cb: ObserveCallback): () => void {
    this.callbacks.add(cb);
    return () => {
      this.callbacks.delete(cb);
    };
  }

  /** 释放底层资源与监听。 */
  destroy(): void {
    if (this.updateHandler) this.doc.off("update", this.updateHandler);
    this.callbacks.clear();
  }

  /* ----------------------------- 内部 ----------------------------- */

  private attach(): void {
    this.updateHandler = (_update, origin) => {
      const event: DocChangeEvent = {
        origin: origin === LOCAL_ORIGIN ? "local" : "remote",
        hash: this.hash(),
      };
      for (const cb of this.callbacks) cb(event);
    };
    this.doc.on("update", this.updateHandler);
  }

  private transact(fn: () => void): void {
    this.doc.transact(fn, LOCAL_ORIGIN);
  }

  private assertType(expected: DocType): void {
    if (this.type !== expected) {
      throw new Error(`该 mutation 仅用于 ${expected} 文档,当前文档类型为 ${this.type}`);
    }
  }
}
