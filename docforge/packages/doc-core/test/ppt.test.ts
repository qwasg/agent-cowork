import { describe, it, expect } from "vitest";
import { DocCore, createSeqIdFactory } from "../src/index.js";

function makePpt() {
  return new DocCore({ type: "ppt", idFactory: createSeqIdFactory() });
}

describe("PPT mutation API", () => {
  it("add_slide 按 index 插入并返回 id,版式注入种子元素", () => {
    const d = makePpt();
    const s1 = d.add_slide(0, "title");
    const s2 = d.add_slide(1, "titleBody");
    const s0 = d.add_slide(0, "blank"); // 插到最前
    const ir = d.read_document();
    if (ir.type !== "ppt") return;
    expect(ir.slides.map((s) => s.id)).toEqual([s0, s1, s2]);
    expect(ir.slides[1].layout).toBe("title");
    expect(ir.slides[1].elements.length).toBeGreaterThan(0);
    expect(ir.slides[0].elements.length).toBe(0); // blank 无种子
  });

  it("edit_element 合并 props", () => {
    const d = makePpt();
    const s = d.add_slide(0, "title");
    const ir1 = d.read_document();
    if (ir1.type !== "ppt") return;
    const elId = ir1.slides[0].elements[0].id;
    d.edit_element(s, elId, { text: "新标题", color: "FF0000" });
    const ir2 = d.read_document();
    if (ir2.type !== "ppt") return;
    const el = ir2.slides[0].elements[0];
    expect(el.props.text).toBe("新标题");
    expect(el.props.color).toBe("FF0000");
  });

  it("move_element 部分更新 geo", () => {
    const d = makePpt();
    const s = d.add_slide(0, "title");
    const ir1 = d.read_document();
    if (ir1.type !== "ppt") return;
    const elId = ir1.slides[0].elements[0].id;
    const before = ir1.slides[0].elements[0].geo;
    d.move_element(s, elId, { x: 2.5, y: 3.5 });
    const ir2 = d.read_document();
    if (ir2.type !== "ppt") return;
    const geo = ir2.slides[0].elements[0].geo;
    expect(geo.x).toBe(2.5);
    expect(geo.y).toBe(3.5);
    expect(geo.w).toBe(before.w); // 未提供保持不变
  });

  it("edit_element slideId/elId 不存在抛错", () => {
    const d = makePpt();
    const s = d.add_slide(0, "title");
    expect(() => d.edit_element("nope", "x", {})).toThrow();
    expect(() => d.edit_element(s, "nope", {})).toThrow();
  });

  it("get_outline 返回每页标题", () => {
    const d = makePpt();
    const s1 = d.add_slide(0, "title");
    const ir = d.read_document();
    if (ir.type !== "ppt") return;
    d.edit_element(s1, ir.slides[0].elements[0].id, { text: "封面" });
    expect(d.get_outline()[0].text).toBe("封面");
  });
});
