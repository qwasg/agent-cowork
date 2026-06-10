import PptxGenJS from "pptxgenjs";
import { SLIDE_WIDTH, SLIDE_HEIGHT, type PptDocIR, type SlideElement } from "@docforge/doc-core";

const LAYOUT_NAME = "DOCFORGE_WIDE";

function num(v: unknown, dflt: number): number {
  const n = Number(v);
  return Number.isFinite(n) ? n : dflt;
}

function addElement(slide: PptxGenJS.Slide, pptx: PptxGenJS, el: SlideElement): void {
  const { x, y, w, h, rot } = el.geo;
  const base = { x, y, w, h, rotate: rot ?? 0 };
  const props = el.props as Record<string, unknown>;

  if (el.type === "text") {
    const text = typeof props.text === "string" ? props.text : "";
    slide.addText(text, {
      ...base,
      fontSize: num(props.fontSize, 18),
      color: typeof props.color === "string" ? props.color : "222222",
      bold: Boolean(props.bold),
      italic: Boolean(props.italic),
      align: (props.align as "left" | "center" | "right") ?? "left",
      valign: "top",
    });
  } else if (el.type === "shape") {
    slide.addShape(pptx.ShapeType.rect, {
      ...base,
      fill: { color: typeof props.fill === "string" ? props.fill : "4A90D9" },
    });
  } else if (el.type === "image") {
    const src = typeof props.src === "string" ? props.src : "";
    if (!src) return;
    const opts: PptxGenJS.ImageProps = { ...base };
    if (src.startsWith("data:")) opts.data = src;
    else opts.path = src;
    slide.addImage(opts);
  }
}

/** PptDocIR -> pptxgenjs 实例。 */
export function buildPptx(ir: PptDocIR): PptxGenJS {
  const pptx = new PptxGenJS();
  pptx.defineLayout({ name: LAYOUT_NAME, width: SLIDE_WIDTH, height: SLIDE_HEIGHT });
  pptx.layout = LAYOUT_NAME;

  if (ir.slides.length === 0) {
    pptx.addSlide();
  }
  for (const slide of ir.slides) {
    const s = pptx.addSlide();
    for (const el of slide.elements) addElement(s, pptx, el);
  }
  return pptx;
}

/**
 * PptDocIR -> .pptx 二进制(nodebuffer)。
 * 若传入 onlySlideId,仅编译该 slide(L2 预览只编当前页)。
 */
export async function compilePpt(ir: PptDocIR, onlySlideId?: string): Promise<Buffer> {
  const target =
    onlySlideId !== undefined
      ? { ...ir, slides: ir.slides.filter((s) => s.id === onlySlideId) }
      : ir;
  const pptx = buildPptx(target);
  const out = (await pptx.write({ outputType: "nodebuffer" })) as Buffer;
  return out;
}
