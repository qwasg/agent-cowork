import type { WordDocIR, WordTextRun } from "@docforge/doc-core";
import { esc } from "../util.js";

function runToHTML(run: WordTextRun): string {
  let html = esc(run.text);
  const marks = new Set(run.marks ?? []);
  if (marks.has("code")) html = `<code>${html}</code>`;
  if (marks.has("strike")) html = `<s>${html}</s>`;
  if (marks.has("underline")) html = `<u>${html}</u>`;
  if (marks.has("italic")) html = `<em>${html}</em>`;
  if (marks.has("bold")) html = `<strong>${html}</strong>`;
  if (run.style) html = `<span class="cs-${esc(run.style)}">${html}</span>`;
  return html;
}

/**
 * L1:Word IR -> 语义 HTML(零编译,目标 <16ms)。
 * 直接用于编辑态预览,也作为 L2 fallback 栅格的输入。
 */
export function wordToHTML(ir: WordDocIR): string {
  const parts: string[] = [];
  for (const block of ir.blocks) {
    const inner = block.runs.map(runToHTML).join("") || "<br/>";
    const styleAttr = block.style ? ` class="ps-${esc(block.style)}"` : "";
    if (block.type === "heading") {
      const lvl = Math.min(6, Math.max(1, block.level ?? 1));
      parts.push(`<h${lvl} data-block-id="${esc(block.id)}"${styleAttr}>${inner}</h${lvl}>`);
    } else {
      parts.push(`<p data-block-id="${esc(block.id)}"${styleAttr}>${inner}</p>`);
    }
  }
  return `<div class="docforge-word">${parts.join("")}</div>`;
}
