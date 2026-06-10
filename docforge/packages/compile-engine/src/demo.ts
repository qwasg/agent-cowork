/**
 * compile-engine demo:用 doc-core 造文档,编译导出真实 .docx / .pptx。
 *   pnpm --filter @docforge/compile-engine demo
 */
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { DocCore } from "@docforge/doc-core";
import { createExporter, createCompileQueue } from "./index.js";

const outDir = join(dirname(fileURLToPath(import.meta.url)), "../../../demo-output");

async function main() {
  // Word
  const word = new DocCore({ type: "word", exporter: createExporter(outDir) });
  const h = word.insert_block(null, { type: "heading", level: 1, text: "DocForge 编译演示" });
  const p = word.insert_block(h, { type: "paragraph", text: "这段文字会被加粗与下划线。" });
  word.apply_style(p, { bold: true, underline: true });
  word.insert_block(p, { type: "heading", level: 2, text: "小节" });
  const docxPath = await word.export("docx");
  console.log("已导出 Word:", docxPath);

  // PPT
  const ppt = new DocCore({ type: "ppt", exporter: createExporter(outDir) });
  const s1 = ppt.add_slide(0, "title");
  ppt.add_slide(1, "titleBody");
  const ir = ppt.read_document();
  if (ir.type === "ppt") {
    ppt.edit_element(s1, ir.slides[0].elements[0].id, { text: "DocForge-Core" });
    ppt.edit_element(s1, ir.slides[0].elements[1].id, { text: "实时编译 / 预览引擎" });
  }
  const pptxPath = await ppt.export("pptx");
  console.log("已导出 PPT:", pptxPath);

  // 队列:演示缓存命中
  const q = createCompileQueue({ debounceMs: 50 });
  await q.schedule({ ir: word.read_document(), format: "docx" });
  await q.schedule({ ir: word.read_document(), format: "docx" });
  console.log("队列统计:", q.stats);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
