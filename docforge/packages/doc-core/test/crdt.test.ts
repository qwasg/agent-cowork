import { describe, it, expect } from "vitest";
import * as Y from "yjs";
import { DocCore } from "../src/index.js";

/** 把 a 的更新同步给 b、b 的更新同步给 a(模拟双向 CRDT 合并)。 */
function sync(a: Y.Doc, b: Y.Doc) {
  const ua = Y.encodeStateAsUpdate(a, Y.encodeStateVector(b));
  const ub = Y.encodeStateAsUpdate(b, Y.encodeStateVector(a));
  Y.applyUpdate(b, ua);
  Y.applyUpdate(a, ub);
}

describe("CRDT 并发写无丢失", () => {
  it("两端各自插入块,合并后两块都在", () => {
    const A = new DocCore({ type: "word" });
    const B = new DocCore({ type: "word" });

    const a1 = A.insert_block(null, { type: "paragraph", text: "来自A" });
    const b1 = B.insert_block(null, { type: "paragraph", text: "来自B" });

    sync(A.doc, B.doc);

    const irA = A.read_document();
    const irB = B.read_document();
    if (irA.type !== "word" || irB.type !== "word") return;

    const textsA = irA.blocks.map((x) => x.runs.map((r) => r.text).join(""));
    const textsB = irB.blocks.map((x) => x.runs.map((r) => r.text).join(""));

    expect(textsA.sort()).toEqual(["来自A", "来自B"]);
    // 两端收敛到完全一致的状态
    expect(textsA).toEqual(textsB);
    expect(irA.blocks.map((x) => x.id)).toEqual(irB.blocks.map((x) => x.id));
    expect(a1).not.toBe(b1);
  });

  it("并发对同一文本块输入,字符不丢失", () => {
    const A = new DocCore({ type: "word" });
    const B = new DocCore({ type: "word" });
    const id = A.insert_block(null, { type: "paragraph", text: "" });
    sync(A.doc, B.doc);

    A.replace_text(id, "AAAA");
    B.replace_text(id, "BBBB");
    sync(A.doc, B.doc);

    const irA = A.read_document();
    const irB = B.read_document();
    if (irA.type !== "word" || irB.type !== "word") return;
    const tA = irA.blocks[0].runs.map((r) => r.text).join("");
    // 收敛一致
    expect(tA).toBe(irB.blocks[0].runs.map((r) => r.text).join(""));
    // CRDT 不丢字符:两端文本都在(交织但无丢失)
    expect(tA).toContain("A");
    expect(tA).toContain("B");
  });

  it("PPT 并发加 slide 合并无丢失", () => {
    const A = new DocCore({ type: "ppt" });
    const B = new DocCore({ type: "ppt" });
    A.add_slide(0, "title");
    B.add_slide(0, "titleBody");
    sync(A.doc, B.doc);
    const irA = A.read_document();
    const irB = B.read_document();
    if (irA.type !== "ppt" || irB.type !== "ppt") return;
    expect(irA.slides.length).toBe(2);
    expect(irA.slides.map((s) => s.id)).toEqual(irB.slides.map((s) => s.id));
  });
});
