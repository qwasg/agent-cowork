import * as Y from "yjs";
import type { DocIR, PptDocIR, WordDocIR } from "./types.js";
import { defaultIdFactory, type IdFactory } from "./ids.js";
import * as WordBind from "./binding/word.js";
import * as PptBind from "./binding/ppt.js";

/** IR -> JSON 字符串。 */
export function toJSON(ir: DocIR): string {
  return JSON.stringify(ir, null, 2);
}

/** JSON 字符串 -> IR。 */
export function fromJSON(json: string): DocIR {
  return JSON.parse(json) as DocIR;
}

/**
 * 将 IR 快照灌入一个(空)Y.Doc,用于导入 / 测试初始化。
 * 缺失 id 时用 idFactory 补齐。
 */
export function applyIRToDoc(
  doc: Y.Doc,
  ir: DocIR,
  idFactory: IdFactory = defaultIdFactory,
): void {
  if (ir.type === "word") {
    const root = WordBind.getWordRoot(doc);
    doc.transact(() => {
      for (const block of (ir as WordDocIR).blocks) {
        const el = WordBind.buildBlockElement(
          { ...block, id: block.id || idFactory("blk") },
          idFactory,
        );
        root.insert(root.length, [el]);
      }
    });
  } else {
    const root = PptBind.getPptRoot(doc);
    doc.transact(() => {
      for (const slide of (ir as PptDocIR).slides) {
        const slideMap = PptBind.buildSlide({
          ...slide,
          id: slide.id || idFactory("sld"),
          elements: slide.elements.map((e) => ({
            ...e,
            id: e.id || idFactory("el"),
          })),
        });
        root.insert(root.length, [slideMap]);
      }
    });
  }
}
