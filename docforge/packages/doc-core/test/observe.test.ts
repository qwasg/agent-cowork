import { describe, it, expect, vi } from "vitest";
import * as Y from "yjs";
import { DocCore } from "../src/index.js";

describe("observe", () => {
  it("每次 mutation 触发回调,携带 local origin 与 hash", () => {
    const d = new DocCore({ type: "word" });
    const cb = vi.fn();
    d.observe(cb);
    d.insert_block(null, { type: "paragraph", text: "x" });
    expect(cb).toHaveBeenCalledTimes(1);
    const evt = cb.mock.calls[0][0];
    expect(evt.origin).toBe("local");
    expect(typeof evt.hash).toBe("string");
    expect(evt.hash.length).toBe(16);
  });

  it("hash 随内容变化,内容相同则相同", () => {
    const d = new DocCore({ type: "word" });
    const h0 = d.hash();
    const id = d.insert_block(null, { type: "paragraph", text: "abc" });
    const h1 = d.hash();
    expect(h1).not.toBe(h0);
    d.replace_text(id, "abc"); // 改成相同内容
    expect(d.hash()).toBe(h1);
  });

  it("远端更新触发 remote origin", () => {
    const A = new DocCore({ type: "word" });
    const B = new DocCore({ type: "word" });
    const cb = vi.fn();
    B.observe(cb);
    A.insert_block(null, { type: "paragraph", text: "remote" });
    Y.applyUpdate(B.doc, Y.encodeStateAsUpdate(A.doc));
    expect(cb).toHaveBeenCalled();
    expect(cb.mock.calls.at(-1)![0].origin).toBe("remote");
  });

  it("取消订阅后不再触发", () => {
    const d = new DocCore({ type: "word" });
    const cb = vi.fn();
    const off = d.observe(cb);
    off();
    d.insert_block(null, { type: "paragraph", text: "x" });
    expect(cb).not.toHaveBeenCalled();
  });

  it("export 未注入 exporter 抛错;注入后返回路径", async () => {
    const d = new DocCore({
      type: "word",
      exporter: async (_ir, fmt) => `/tmp/out.${fmt}`,
    });
    await expect(d.export("docx")).resolves.toBe("/tmp/out.docx");

    const d2 = new DocCore({ type: "word" });
    await expect(d2.export("docx")).rejects.toThrow();
  });
});
