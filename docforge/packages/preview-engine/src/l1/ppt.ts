import { SLIDE_WIDTH, SLIDE_HEIGHT, type Slide, type SlideElement } from "@docforge/doc-core";
import { esc, color } from "../util.js";

export interface SlideSvgOptions {
  /** 输出像素宽(高按 16:9 推算)。不给则只设 viewBox(矢量缩放)。 */
  pxWidth?: number;
}

function elementSVG(el: SlideElement): string {
  const { x, y, w, h, rot } = el.geo;
  const transform = `translate(${x} ${y})${rot ? ` rotate(${rot})` : ""}`;
  const p = el.props as Record<string, unknown>;

  if (el.type === "shape") {
    return `<g transform="${transform}"><rect width="${w}" height="${h}" rx="0.05" fill="${color(p.fill, "#4A90D9")}"/></g>`;
  }
  if (el.type === "image") {
    const src = typeof p.src === "string" ? p.src : "";
    if (!src) return "";
    return `<g transform="${transform}"><image href="${esc(src)}" width="${w}" height="${h}" preserveAspectRatio="xMidYMid meet"/></g>`;
  }
  // text:用 foreignObject 承载,fontSize 以英寸为单位(viewBox 即英寸)
  const text = typeof p.text === "string" ? p.text : "";
  const fontSize = (Number(p.fontSize) || 18) / 96; // px -> inch (96dpi)
  const weight = p.bold ? 700 : 400;
  const style = p.italic ? "italic" : "normal";
  const align = (p.align as string) ?? "left";
  return (
    `<g transform="${transform}">` +
    `<foreignObject width="${w}" height="${h}">` +
    `<div xmlns="http://www.w3.org/1999/xhtml" style="width:100%;height:100%;` +
    `font-size:${fontSize}px;line-height:1.2;color:${color(p.color)};font-weight:${weight};` +
    `font-style:${style};text-align:${align};overflow:hidden;font-family:Segoe UI,Arial,sans-serif;">` +
    `${esc(text)}</div></foreignObject></g>`
  );
}

/**
 * L1:单张 slide -> 独立 SVG 字符串。viewBox 单位为英寸(10 x 5.625)。
 * 同时用于编辑态预览与 L2 fallback 栅格输入。
 */
export function slideToSVG(slide: Slide, options: SlideSvgOptions = {}): string {
  const body =
    `<rect x="0" y="0" width="${SLIDE_WIDTH}" height="${SLIDE_HEIGHT}" fill="#ffffff"/>` +
    slide.elements.map(elementSVG).join("");
  const sizeAttrs =
    options.pxWidth !== undefined
      ? ` width="${options.pxWidth}" height="${(options.pxWidth * SLIDE_HEIGHT) / SLIDE_WIDTH}"`
      : "";
  return (
    `<svg xmlns="http://www.w3.org/2000/svg"${sizeAttrs} viewBox="0 0 ${SLIDE_WIDTH} ${SLIDE_HEIGHT}">` +
    body +
    `</svg>`
  );
}
