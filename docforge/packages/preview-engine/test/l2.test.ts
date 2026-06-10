import { describe, it, expect } from "vitest";
import { readFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { DocCore } from "@docforge/doc-core";
import { FallbackRasterRenderer, L2PreviewEngine, rasterKey } from "../src/index.js";

function makePpt() {
  const d = new DocCore({ type: "ppt" });
  const slideId = d.add_slide(0, "title");
  const ir = d.read_document();
  if (ir.type === "ppt") {
    d.edit_element(slideId, ir.slides[0].elements[0].id, { text: "缩略图" });
  }
  return { ir: d.read_document(), slideId };
}

describe("L2 预览", () => {
  it("fallback renderer 输出非空 PNG", async () => {
    const outDir = join(tmpdir(), `docforge-preview-${Date.now()}`);
    const { ir, slideId } = makePpt();
    const renderer = new FallbackRasterRenderer();
    const res = await renderer.render({ ir, slideId, outDir, width: 640 });
    const png = await readFile(res.pngPath);
    expect(png[0]).toBe(0x89);
    expect(png[1]).toBe(0x50);
    expect(png.byteLength).toBeGreaterThan(500);
    expect(res.renderer).toBe("fallback-svg-resvg");
    await rm(outDir, { recursive: true, force: true });
  });

  it("L2PreviewEngine 使用 hash-cache 跳过无变更", async () => {
    const outDir = join(tmpdir(), `docforge-preview-cache-${Date.now()}`);
    const { ir, slideId } = makePpt();
    const engine = new L2PreviewEngine({ renderer: new FallbackRasterRenderer(), debounceMs: 10 });
    const req = { ir, slideId, outDir, width: 640 };
    const a = await engine.preview(req);
    const b = await engine.preview(req);
    expect(a.cached).toBe(false);
    expect(b.cached).toBe(true);
    expect(a.result.pngPath).toBe(b.result.pngPath);
    expect(engine.stats.compiles).toBe(1);
    expect(engine.stats.cacheHits).toBe(1);
    expect(rasterKey(req)).toBe(rasterKey(req));
    await rm(outDir, { recursive: true, force: true });
  });

  it("Word 也能 fallback 栅格化", async () => {
    const outDir = join(tmpdir(), `docforge-preview-word-${Date.now()}`);
    const d = new DocCore({ type: "word" });
    d.insert_block(null, { type: "heading", level: 1, text: "Word 预览" });
    d.insert_block(null, { type: "paragraph", text: "正文" });
    const res = await new FallbackRasterRenderer().render({ ir: d.read_document(), outDir, width: 500 });
    const png = await readFile(res.pngPath);
    expect(png[0]).toBe(0x89);
    expect(png.byteLength).toBeGreaterThan(500);
    await rm(outDir, { recursive: true, force: true });
  });
});
