import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { Resvg } from "@resvg/resvg-js";
import {
  SLIDE_HEIGHT,
  SLIDE_WIDTH,
  contentHash,
  type DocIR,
  type PptDocIR,
  type Slide,
} from "@docforge/doc-core";
import { esc, color } from "../util.js";
import { wordToHTML } from "../l1/word.js";
import type { RasterRenderer, RasterRequest, RasterResult } from "./types.js";

const DEFAULT_WIDTH = 960;

function wrapWordSvg(ir: DocIR, width: number): string {
  const height = Math.round(width * 1.294); // approx A4 portrait preview
  const html = ir.type === "word" ? wordToHTML(ir) : "";
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}">
  <rect width="${width}" height="${height}" fill="#ffffff"/>
  <foreignObject x="48" y="40" width="${width - 96}" height="${height - 80}">
    <div xmlns="http://www.w3.org/1999/xhtml" style="font-family:Segoe UI,Arial,sans-serif;font-size:18px;line-height:1.55;color:#222;">
      ${html}
    </div>
  </foreignObject>
</svg>`;
}

function textLines(text: string): string[] {
  return text.split(/\r?\n/g).flatMap((line) => (line ? [line] : [" "]));
}

function pureSlideSvg(slide: Slide, width: number): string {
  const height = Math.round((width * SLIDE_HEIGHT) / SLIDE_WIDTH);
  const sx = width / SLIDE_WIDTH;
  const sy = height / SLIDE_HEIGHT;
  const parts: string[] = [
    `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}">`,
    `<rect width="${width}" height="${height}" fill="#ffffff"/>`,
  ];
  for (const el of slide.elements) {
    const x = el.geo.x * sx;
    const y = el.geo.y * sy;
    const w = el.geo.w * sx;
    const h = el.geo.h * sy;
    const p = el.props as Record<string, unknown>;
    const rot = el.geo.rot ? ` rotate(${el.geo.rot} ${x} ${y})` : "";
    if (el.type === "shape") {
      parts.push(`<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="6" fill="${color(p.fill, "#4A90D9")}" transform="${rot}"/>`);
    } else if (el.type === "image" && typeof p.src === "string" && p.src) {
      parts.push(`<image href="${esc(p.src)}" x="${x}" y="${y}" width="${w}" height="${h}" preserveAspectRatio="xMidYMid meet" transform="${rot}"/>`);
    } else if (el.type === "text") {
      const fontSize = Number(p.fontSize) || 18;
      const fontWeight = p.bold ? "700" : "400";
      const fontStyle = p.italic ? "italic" : "normal";
      const fill = color(p.color);
      const text = typeof p.text === "string" ? p.text : "";
      parts.push(`<g transform="${rot}">`);
      textLines(text).forEach((line, i) => {
        parts.push(
          `<text x="${x}" y="${y + fontSize * (i + 1)}" font-family="Segoe UI,Arial,sans-serif" font-size="${fontSize}" font-weight="${fontWeight}" font-style="${fontStyle}" fill="${fill}">${esc(line)}</text>`,
        );
      });
      parts.push("</g>");
    }
  }
  parts.push("</svg>");
  return parts.join("");
}

function selectSlide(ir: PptDocIR, slideId?: string): Slide | undefined {
  return slideId ? ir.slides.find((s) => s.id === slideId) : ir.slides[0];
}

/**
 * 自搓 L2 fallback:不依赖 LibreOffice,直接把 IR 渲为 SVG,再用 resvg 栅格化 PNG。
 * 这不是 Office 保真,但能保证装不上 soffice 时仍有可运行缩略预览。
 */
export class FallbackRasterRenderer implements RasterRenderer {
  readonly name = "fallback-svg-resvg";

  async isAvailable(): Promise<boolean> {
    return true;
  }

  async render(request: RasterRequest): Promise<RasterResult> {
    const width = request.width ?? DEFAULT_WIDTH;
    const effective =
      request.ir.type === "ppt" && request.slideId
        ? { ...request.ir, slides: request.ir.slides.filter((s) => s.id === request.slideId) }
        : request.ir;
    const hash = contentHash({ ir: effective, width, renderer: this.name });
    await mkdir(request.outDir, { recursive: true });
    const pngPath = join(request.outDir, `${hash}.png`);
    if (request.signal?.aborted) throw new Error("render aborted");
    const svg =
      request.ir.type === "word"
        ? wrapWordSvg(request.ir, width)
        : pureSlideSvg(selectSlide(request.ir, request.slideId) ?? { id: "empty", layout: "blank", elements: [] }, width);
    const png = new Resvg(svg, { fitTo: { mode: "width", value: width } }).render().asPng();
    if (request.signal?.aborted) throw new Error("render aborted");
    await writeFile(pngPath, png);
    return { pngPath, renderer: this.name, hash };
  }
}
