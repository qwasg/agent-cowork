/**
 * doc-core demo:跑一串 mutation,打印 outline 与 IR。
 *   pnpm --filter @docforge/doc-core demo
 */
import { DocCore } from "./index.js";

function section(title: string): void {
  console.log("\n" + "=".repeat(48));
  console.log(title);
  console.log("=".repeat(48));
}

/* --------------------------- Word demo --------------------------- */
section("WORD 文档");
const word = new DocCore({ type: "word" });

let observed = 0;
word.observe((e) => {
  observed++;
});

const h1 = word.insert_block(null, { type: "heading", level: 1, text: "DocForge 报告" });
const p1 = word.insert_block(h1, { type: "paragraph", text: "这是第一段正文。" });
const h2 = word.insert_block(p1, { type: "heading", level: 2, text: "背景" });
const p2 = word.insert_block(h2, { type: "paragraph", text: "需要加粗的一段。" });

word.replace_text(p1, "替换后的第一段内容。");
word.apply_style(p2, { bold: true, italic: true });

console.log("IR:", JSON.stringify(word.read_document(), null, 2));
console.log("\nOutline:", word.get_outline());
console.log("Hash:", word.hash());
console.log("observe 触发次数:", observed);

/* --------------------------- PPT demo ---------------------------- */
section("PPT 文档");
const ppt = new DocCore({ type: "ppt" });

const s1 = ppt.add_slide(0, "title");
const s2 = ppt.add_slide(1, "titleBody");

const slide1 = ppt.read_document();
if (slide1.type === "ppt") {
  const titleEl = slide1.slides[0].elements[0];
  ppt.edit_element(s1, titleEl.id, { text: "DocForge-Core" });
  ppt.move_element(s1, titleEl.id, { x: 0.5, y: 1.0 });
}

console.log("IR:", JSON.stringify(ppt.read_document(), null, 2));
console.log("\nOutline:", ppt.get_outline());
console.log("Hash:", ppt.hash());
console.log("slide ids:", s1, s2);
