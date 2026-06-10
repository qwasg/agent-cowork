import type { ElementType, Geo } from "./types.js";

/** 版式中的种子元素模板(id 在落库时分配)。 */
export interface SeedElement {
  /** 语义角色,便于 UI/编译识别(title/body/...)。 */
  role: string;
  type: ElementType;
  geo: Geo;
  props: Record<string, unknown>;
}

/** 标准 16:9 幻灯片逻辑尺寸(英寸)。 */
export const SLIDE_WIDTH = 10;
export const SLIDE_HEIGHT = 5.625;

/**
 * 版式 -> 种子元素。新建 slide 时按版式注入占位元素,
 * 这样 edit_element / move_element 立即有可操作对象(契约里没有 add_element)。
 */
export const LAYOUTS: Record<string, SeedElement[]> = {
  blank: [],
  title: [
    {
      role: "title",
      type: "text",
      geo: { x: 1, y: 2, w: 8, h: 1.2 },
      props: { text: "标题", fontSize: 40, bold: true, align: "center" },
    },
    {
      role: "subtitle",
      type: "text",
      geo: { x: 1.5, y: 3.3, w: 7, h: 0.8 },
      props: { text: "副标题", fontSize: 20, color: "666666", align: "center" },
    },
  ],
  titleBody: [
    {
      role: "title",
      type: "text",
      geo: { x: 0.6, y: 0.4, w: 8.8, h: 1 },
      props: { text: "标题", fontSize: 32, bold: true },
    },
    {
      role: "body",
      type: "text",
      geo: { x: 0.6, y: 1.6, w: 8.8, h: 3.5 },
      props: { text: "正文内容", fontSize: 18 },
    },
  ],
  twoContent: [
    {
      role: "title",
      type: "text",
      geo: { x: 0.6, y: 0.4, w: 8.8, h: 1 },
      props: { text: "标题", fontSize: 32, bold: true },
    },
    {
      role: "left",
      type: "text",
      geo: { x: 0.6, y: 1.6, w: 4.2, h: 3.5 },
      props: { text: "左侧", fontSize: 16 },
    },
    {
      role: "right",
      type: "text",
      geo: { x: 5.2, y: 1.6, w: 4.2, h: 3.5 },
      props: { text: "右侧", fontSize: 16 },
    },
  ],
};

export function getLayoutSeeds(layout: string): SeedElement[] {
  return LAYOUTS[layout] ?? LAYOUTS.blank;
}
