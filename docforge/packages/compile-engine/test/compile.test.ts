import { describe, it, expect } from "vitest";
import { readFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { DocCore } from "@docforge/doc-core";
import { compileToBuffer, exportToFile, createExporter } from "../src/index.js";

/** zip 文件本地头里以未压缩明文存放条目名,可直接在 buffer 里搜。 */
function bufferIncludes(buf: Buffer, needle: string): boolean {
  return buf.includes(Buffer.from(needle, "utf8"));
}

function isZip(buf: Buffer): boolean {
  return buf[0] === 0x50 && buf[1] === 0x4b; // "PK"
}

function makeWordIR() {
  const d = new DocCore({ type: "word" });
  const h = d.insert_block(null, { type: "heading", level: 1, text: "标题" });
  const p = d.insert_block(h, { type: "paragraph", text: "正文一段" });
  d.apply_style(p, { bold: true });
  return d.read_document();
}

function makePptIR() {
  const d = new DocCore({ type: "ppt" });
  const s1 = d.add_slide(0, "title");
  d.add_slide(1, "titleBody");
  const ir = d.read_document();
  if (ir.type === "ppt") d.edit_element(s1, ir.slides[0].elements[0].id, { text: "封面" });
  return { ir: d.read_document(), firstSlideId: s1 };
}

describe("IR→OOXML 编译", () => {
  it("Word -> 合法 .docx(zip + OOXML 结构)", async () => {
    const buf = await compileToBuffer(makeWordIR(), "docx");
    expect(isZip(buf)).toBe(true);
    expect(bufferIncludes(buf, "[Content_Types].xml")).toBe(true);
    expect(bufferIncludes(buf, "word/document.xml")).toBe(true);
    expect(buf.byteLength).toBeGreaterThan(500);
  });

  it("PPT -> 合法 .pptx(含 presentation 与多张 slide)", async () => {
    const { ir } = makePptIR();
    const buf = await compileToBuffer(ir, "pptx");
    expect(isZip(buf)).toBe(true);
    expect(bufferIncludes(buf, "[Content_Types].xml")).toBe(true);
    expect(bufferIncludes(buf, "ppt/presentation.xml")).toBe(true);
    expect(bufferIncludes(buf, "ppt/slides/slide1.xml")).toBe(true);
    expect(bufferIncludes(buf, "ppt/slides/slide2.xml")).toBe(true);
  });

  it("PPT onlySlideId 只编当前页", async () => {
    const { ir, firstSlideId } = makePptIR();
    const buf = await compileToBuffer(ir, "pptx", { onlySlideId: firstSlideId });
    expect(bufferIncludes(buf, "ppt/slides/slide1.xml")).toBe(true);
    expect(bufferIncludes(buf, "ppt/slides/slide2.xml")).toBe(false);
  });

  it("格式与文档类型不匹配抛错", async () => {
    await expect(compileToBuffer(makeWordIR(), "pptx")).rejects.toThrow();
    await expect(compileToBuffer(makePptIR().ir, "docx")).rejects.toThrow();
  });

  it("exportToFile 写出真实文件", async () => {
    const out = join(tmpdir(), `docforge-test-${Date.now()}.docx`);
    const fp = await exportToFile(makeWordIR(), "docx", out);
    const stat = await readFile(fp);
    expect(stat.byteLength).toBeGreaterThan(500);
    expect(isZip(stat)).toBe(true);
    await rm(fp, { force: true });
  });

  it("createExporter 作为 doc-core 的 exporter 工作", async () => {
    const dir = join(tmpdir(), `docforge-exp-${Date.now()}`);
    const d = new DocCore({ type: "word", exporter: createExporter(dir) });
    d.insert_block(null, { type: "paragraph", text: "导出测试" });
    const fp = await d.export("docx");
    expect(fp.endsWith(".docx")).toBe(true);
    const buf = await readFile(fp);
    expect(isZip(buf)).toBe(true);
    await rm(dir, { recursive: true, force: true });
  });
});
