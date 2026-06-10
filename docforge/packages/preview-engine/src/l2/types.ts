import type { DocIR } from "@docforge/doc-core";

export interface RasterRequest {
  ir: DocIR;
  /** PPT L2 预览只编当前 slide;Word 忽略。 */
  slideId?: string;
  /** 输出目录。 */
  outDir: string;
  /** 输出像素宽。 */
  width?: number;
  signal?: AbortSignal;
}

export interface RasterResult {
  /** PNG 缩略图路径。 */
  pngPath: string;
  /** 渲染器名字,便于诊断 soffice/fallback。 */
  renderer: string;
  /** 当前渲染输入的内容 hash。 */
  hash: string;
}

export interface RasterRenderer {
  readonly name: string;
  isAvailable(): Promise<boolean>;
  render(request: RasterRequest): Promise<RasterResult>;
}
