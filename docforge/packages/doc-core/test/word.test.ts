import { describe, it, expect } from "vitest";
import { DocCore, createSeqIdFactory } from "../src/index.js";

function makeWord() {
  return new DocCore({ type: "word", idFactory: createSeqIdFactory() });
}

describe("Word mutation API", () => {
  it("insert_block 追加 / 顺序 / 返回 id", () => {
    const d = makeWord();
    const h = d.insert_block(null, { type: "heading", level: 1, text: "标题" });
    const p = d.insert_block(h, { type: "paragraph", text: "正文" });
    const ir = d.read_document();
    expect(ir.type).toBe("word");
    if (ir.type !== "word") return;
    expect(ir.blocks.map((b) => b.id)).toEqual([h, p]);
    expect(ir.blocks[0].type).toBe("heading");
    expect(ir.blocks[0].level).toBe(1);
    expect(ir.blocks[1].runs[0].text).toBe("正文");
  });

  it("insert_block 在指定块之后插入", () => {
    const d = makeWord();
    const a = d.insert_block(null, { type: "paragraph", text: "A" });
    const b = d.insert_block(a, { type: "paragraph", text: "B" });
    const c = d.insert_block(a, { type: "paragraph", text: "C" }); // 插在 A 后 -> A C B
    const ir = d.read_document();
    if (ir.type !== "word") return;
    expect(ir.blocks.map((x) => x.runs[0].text)).toEqual(["A", "C", "B"]);
    expect(ir.blocks.map((x) => x.id)).toEqual([a, c, b]);
  });

  it("insert_block afterId 不存在抛错", () => {
    const d = makeWord();
    expect(() => d.insert_block("nope", { type: "paragraph", text: "x" })).toThrow();
  });

  it("replace_text 替换整块文本", () => {
    const d = makeWord();
    const p = d.insert_block(null, { type: "paragraph", text: "原文" });
    d.replace_text(p, "新文本");
    const ir = d.read_document();
    if (ir.type !== "word") return;
    expect(ir.blocks[0].runs.map((r) => r.text).join("")).toBe("新文本");
  });

  it("apply_style 给整块文本加 marks", () => {
    const d = makeWord();
    const p = d.insert_block(null, { type: "paragraph", text: "加粗斜体" });
    d.apply_style(p, { bold: true, italic: true });
    const ir = d.read_document();
    if (ir.type !== "word") return;
    const marks = ir.blocks[0].runs[0].marks ?? [];
    expect(marks).toContain("bold");
    expect(marks).toContain("italic");
  });

  it("apply_style 命名样式落到 block.style", () => {
    const d = makeWord();
    const p = d.insert_block(null, { type: "paragraph", text: "引用" });
    d.apply_style(p, { style: "Quote" });
    const ir = d.read_document();
    if (ir.type !== "word") return;
    expect(ir.blocks[0].style).toBe("Quote");
  });

  it("get_outline 仅含标题", () => {
    const d = makeWord();
    const h1 = d.insert_block(null, { type: "heading", level: 1, text: "一级" });
    const p = d.insert_block(h1, { type: "paragraph", text: "正文" });
    d.insert_block(p, { type: "heading", level: 2, text: "二级" });
    const outline = d.get_outline();
    expect(outline.map((o) => o.text)).toEqual(["一级", "二级"]);
    expect(outline.map((o) => o.level)).toEqual([1, 2]);
  });

  it("PPT mutation 用在 word 文档上抛错", () => {
    const d = makeWord();
    expect(() => d.add_slide(0, "title")).toThrow();
  });
});
