import { describe, it, expect } from "vitest";
import { performance } from "node:perf_hooks";
import { DocCore } from "@docforge/doc-core";
import { renderL1, wordToHTML, slideToSVG } from "../src/index.js";

describe("L1 即时预览", () => {
  it("Word IR -> HTML", () => {
    const d = new DocCore({ type: "word" });
    const h = d.insert_block(null, { type: "heading", level: 1, text: "标题" });
    const p = d.insert_block(h, { type: "paragraph", text: "正文" });
    d.apply_style(p, { bold: true });
    const ir = d.read_document();
    expect(ir.type).toBe("word");
    if (ir.type !== "word") return;
    const html = wordToHTML(ir);
    expect(html).toContain("<h1");
    expect(html).toContain("标题");
    expect(html).toContain("<strong>正文</strong>");
  });

  it("PPT slide -> SVG", () => {
    const d = new DocCore({ type: "ppt" });
    d.add_slide(0, "title");
    const ir = d.read_document();
    expect(ir.type).toBe("ppt");
    if (ir.type !== "ppt") return;
    const svg = slideToSVG(ir.slides[0]);
    expect(svg).toContain("<svg");
    expect(svg).toContain("viewBox");
    expect(svg).toContain("标题");
  });

  it("renderL1 分派 word/html 与 ppt/svg", () => {
    const word = new DocCore({ type: "word" });
    word.insert_block(null, { type: "paragraph", text: "w" });
    expect(renderL1(word.read_document()).kind).toBe("html");

    const ppt = new DocCore({ type: "ppt" });
    ppt.add_slide(0, "blank");
    expect(renderL1(ppt.read_document()).kind).toBe("svg");
  });

  it("小文档 L1 渲染低于 16ms", () => {
    const d = new DocCore({ type: "word" });
    let prev: string | null = null;
    for (let i = 0; i < 120; i++) {
      prev = d.insert_block(prev, { type: i % 10 === 0 ? "heading" : "paragraph", level: 2, text: `块 ${i}` });
    }
    const t0 = performance.now();
    const out = renderL1(d.read_document());
    const dt = performance.now() - t0;
    expect(out.markup.length).toBeGreaterThan(100);
    expect(dt).toBeLessThan(16);
  });
});
