import * as Y from "yjs";
import type { Geo, PptDocIR, Slide, SlideElement } from "../types.js";

/** 取 PPT 根 array(每项是一个 slide Y.Map)。 */
export function getPptRoot(doc: Y.Doc): Y.Array<Y.Map<unknown>> {
  return doc.getArray<Y.Map<unknown>>("ppt");
}

function readGeo(m: Y.Map<unknown> | undefined): Geo {
  if (!m) return { x: 0, y: 0, w: 1, h: 1 };
  const geo: Geo = {
    x: Number(m.get("x") ?? 0),
    y: Number(m.get("y") ?? 0),
    w: Number(m.get("w") ?? 1),
    h: Number(m.get("h") ?? 1),
  };
  const rot = m.get("rot");
  if (rot !== undefined) geo.rot = Number(rot);
  return geo;
}

function readProps(m: Y.Map<unknown> | undefined): Record<string, unknown> {
  if (!m) return {};
  return m.toJSON() as Record<string, unknown>;
}

export function readElement(m: Y.Map<unknown>): SlideElement {
  return {
    id: String(m.get("id") ?? ""),
    type: (m.get("type") as SlideElement["type"]) ?? "text",
    geo: readGeo(m.get("geo") as Y.Map<unknown> | undefined),
    props: readProps(m.get("props") as Y.Map<unknown> | undefined),
  };
}

export function readSlide(m: Y.Map<unknown>): Slide {
  const elementsArr = m.get("elements") as Y.Array<Y.Map<unknown>> | undefined;
  const elements: SlideElement[] = [];
  if (elementsArr) {
    for (let i = 0; i < elementsArr.length; i++) {
      elements.push(readElement(elementsArr.get(i)));
    }
  }
  return {
    id: String(m.get("id") ?? ""),
    layout: String(m.get("layout") ?? "blank"),
    elements,
  };
}

export function readPptIR(doc: Y.Doc): PptDocIR {
  const root = getPptRoot(doc);
  const slides: Slide[] = [];
  for (let i = 0; i < root.length; i++) {
    slides.push(readSlide(root.get(i)));
  }
  return { type: "ppt", slides };
}

/** 构造 geo Y.Map。 */
export function buildGeo(geo: Geo): Y.Map<unknown> {
  const m = new Y.Map<unknown>();
  m.set("x", geo.x);
  m.set("y", geo.y);
  m.set("w", geo.w);
  m.set("h", geo.h);
  if (geo.rot !== undefined) m.set("rot", geo.rot);
  return m;
}

/** 构造 props Y.Map。 */
export function buildProps(props: Record<string, unknown>): Y.Map<unknown> {
  const m = new Y.Map<unknown>();
  for (const [k, v] of Object.entries(props)) m.set(k, v);
  return m;
}

/** 构造 element Y.Map。 */
export function buildElement(el: SlideElement): Y.Map<unknown> {
  const m = new Y.Map<unknown>();
  m.set("id", el.id);
  m.set("type", el.type);
  m.set("geo", buildGeo(el.geo));
  m.set("props", buildProps(el.props));
  return m;
}

/** 构造 slide Y.Map(elements 为 Y.Array)。 */
export function buildSlide(slide: Slide): Y.Map<unknown> {
  const m = new Y.Map<unknown>();
  m.set("id", slide.id);
  m.set("layout", slide.layout);
  const elements = new Y.Array<Y.Map<unknown>>();
  elements.push(slide.elements.map(buildElement));
  m.set("elements", elements);
  return m;
}

/** 按 id 找 slide 索引。 */
export function findSlideIndex(root: Y.Array<Y.Map<unknown>>, id: string): number {
  for (let i = 0; i < root.length; i++) {
    if (String(root.get(i).get("id")) === id) return i;
  }
  return -1;
}

/** 在某 slide 内按 elId 找元素 Y.Map。 */
export function findElement(
  slideMap: Y.Map<unknown>,
  elId: string,
): Y.Map<unknown> | undefined {
  const arr = slideMap.get("elements") as Y.Array<Y.Map<unknown>> | undefined;
  if (!arr) return undefined;
  for (let i = 0; i < arr.length; i++) {
    const el = arr.get(i);
    if (String(el.get("id")) === elId) return el;
  }
  return undefined;
}
