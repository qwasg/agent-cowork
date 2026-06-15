import { readFileSync, writeFileSync } from "node:fs";
import { fromJSON, SLIDE_HEIGHT, SLIDE_WIDTH } from "../packages/doc-core/src/index.ts";
import type { Slide, SlideElement } from "../packages/doc-core/src/types.ts";

const irPath =
  process.argv[2] ??
  "H:/agent-debug-frontend-backend-copy-20260530/out-simple/docs/api-test.pptx.ir.json";
const outPath =
  process.argv[3] ??
  "H:/agent-debug-frontend-backend-copy-20260530/out-simple/docs/api-test-ppt-preview.html";

function esc(text: string): string {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function elementSvg(el: SlideElement): string {
  const text = String(el.props.text ?? "");
  const fontSize = Number(el.props.fontSize ?? 18) / 72;
  const fill = String(el.props.color ?? "111").replace("#", "");
  const weight = el.props.bold ? 700 : 400;
  const anchor =
    el.props.align === "center" ? "middle" : el.props.align === "right" ? "end" : "start";
  const x =
    el.props.align === "center"
      ? el.geo.x + el.geo.w / 2
      : el.props.align === "right"
        ? el.geo.x + el.geo.w
        : el.geo.x;
  const lines = text.split("\n");
  const lineHeight = fontSize * 1.25;
  const tspans = lines
    .map((line, i) => `<tspan x="${x}" dy="${i === 0 ? 0 : lineHeight}">${esc(line)}</tspan>`)
    .join("");
  return `<text x="${x}" y="${el.geo.y + fontSize}" font-size="${fontSize}" fill="#${fill}" font-weight="${weight}" text-anchor="${anchor}">${tspans}</text>`;
}

function slideSvg(slide: Slide): string {
  const body = slide.elements.map(elementSvg).join("");
  return `<svg xmlns="http://www.w3.org/2000/svg" width="960" height="540" viewBox="0 0 ${SLIDE_WIDTH} ${SLIDE_HEIGHT}"><rect width="${SLIDE_WIDTH}" height="${SLIDE_HEIGHT}" fill="#fff"/>${body}</svg>`;
}

const ir = fromJSON(readFileSync(irPath, "utf8"));
if (ir.type !== "ppt") throw new Error(`expected ppt IR, got ${ir.type}`);

const slides = ir.slides
  .map(
    (slide, i) => `
<section class="slide-card">
  <h2>幻灯片 ${i + 1} · ${slide.layout}</h2>
  <div class="stage">${slideSvg(slide)}</div>
</section>`,
  )
  .join("\n");

const html = `<!DOCTYPE html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8" />
  <title>api-test.pptx 预览</title>
  <style>
    body { font-family: "Segoe UI", system-ui, sans-serif; margin: 0; background: #f3f4f6; color: #111; }
    header { background: #fff; border-bottom: 1px solid #e5e7eb; padding: 16px 24px; position: sticky; top: 0; z-index: 1; }
    h1 { margin: 0 0 6px; font-size: 20px; }
    p { margin: 0; color: #6b7280; font-size: 14px; }
    main { max-width: 1040px; margin: 24px auto; padding: 0 16px 48px; display: grid; gap: 24px; }
    .slide-card { background: #fff; border: 1px solid #e5e7eb; border-radius: 12px; padding: 16px; box-shadow: 0 1px 2px rgba(0,0,0,.04); }
    .slide-card h2 { margin: 0 0 12px; font-size: 15px; font-weight: 600; }
    .stage { background: #fafafa; border: 1px solid #eee; border-radius: 8px; padding: 12px; overflow: auto; }
    .stage svg { display: block; max-width: 100%; height: auto; margin: 0 auto; }
  </style>
</head>
<body>
  <header>
    <h1>api-test.pptx 预览</h1>
    <p>由 Agent 生成的 ${ir.slides.length} 页演示文稿（DocForge L1 SVG 渲染）</p>
  </header>
  <main>${slides}</main>
</body>
</html>`;

writeFileSync(outPath, html, "utf8");
console.log(`[ppt-preview] wrote ${outPath}`);
