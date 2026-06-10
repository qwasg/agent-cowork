import { join } from "node:path";
import { DocCore } from "@docforge/doc-core";
import { renderL1, L2PreviewEngine, FallbackRasterRenderer } from "./index.js";

const outDir = join(process.cwd(), "demo-output", "preview");

async function main() {
  const word = new DocCore({ type: "word" });
  const h = word.insert_block(null, { type: "heading", level: 1, text: "DocForge 预览" });
  word.insert_block(h, { type: "paragraph", text: "L1 零编译 HTML,L2 输出 PNG 缩略。" });
  const l1 = renderL1(word.read_document());
  console.log("Word L1 kind:", l1.kind, "chars:", l1.markup.length);

  const ppt = new DocCore({ type: "ppt" });
  const slideId = ppt.add_slide(0, "title");
  const ir = ppt.read_document();
  if (ir.type === "ppt") {
    ppt.edit_element(slideId, ir.slides[0].elements[0].id, { text: "DocForge-Core" });
  }
  const engine = new L2PreviewEngine({ renderer: new FallbackRasterRenderer(), debounceMs: 50 });
  const png = await engine.previewNow({ ir: ppt.read_document(), slideId, outDir, width: 960 });
  console.log("PPT L2 png:", png.result.pngPath, "renderer:", png.result.renderer);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
