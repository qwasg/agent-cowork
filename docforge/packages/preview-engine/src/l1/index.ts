import type { DocIR } from "@docforge/doc-core";
import { wordToHTML } from "./word.js";
import { slideToSVG } from "./ppt.js";

export { wordToHTML } from "./word.js";
export { slideToSVG, type SlideSvgOptions } from "./ppt.js";

export interface L1Options {
  /** PPT:渲染哪一张(默认第 0 张)。 */
  slideIndex?: number;
}

export interface L1Result {
  kind: "html" | "svg";
  /** HTML(word) 或 SVG(单张 slide) 字符串。 */
  markup: string;
}

/**
 * L1 即时渲染:IR -> DOM 标记(零编译)。Word 返回 HTML,PPT 返回当前 slide 的 SVG。
 */
export function renderL1(ir: DocIR, options: L1Options = {}): L1Result {
  if (ir.type === "word") {
    return { kind: "html", markup: wordToHTML(ir) };
  }
  const idx = options.slideIndex ?? 0;
  const slide = ir.slides[idx];
  const markup = slide
    ? slideToSVG(slide)
    : `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 5.625"><rect width="10" height="5.625" fill="#fff"/></svg>`;
  return { kind: "svg", markup };
}
