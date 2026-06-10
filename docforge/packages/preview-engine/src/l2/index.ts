import { contentHash } from "@docforge/doc-core";
import { DebounceQueue, type QueueResult } from "@docforge/compile-engine";
import { FallbackRasterRenderer } from "./fallback.js";
import { SofficeRasterRenderer } from "./soffice.js";
import type { RasterRenderer, RasterRequest, RasterResult } from "./types.js";

export type { RasterRenderer, RasterRequest, RasterResult } from "./types.js";
export { FallbackRasterRenderer } from "./fallback.js";
export { SofficeRasterRenderer, findSoffice } from "./soffice.js";

export interface L2PreviewOptions {
  debounceMs?: number;
  renderer?: RasterRenderer;
}

export function rasterKey(req: RasterRequest): string {
  const effective =
    req.ir.type === "ppt" && req.slideId
      ? { ...req.ir, slides: req.ir.slides.filter((s) => s.id === req.slideId) }
      : req.ir;
  return `${req.slideId ?? ""}:${req.width ?? 960}:${contentHash(effective)}`;
}

/** 自动选择可用 renderer:优先 soffice,否则 fallback。 */
export async function createDefaultRasterRenderer(): Promise<RasterRenderer> {
  const soffice = new SofficeRasterRenderer();
  return (await soffice.isAvailable()) ? soffice : new FallbackRasterRenderer();
}

export class L2PreviewEngine {
  private readonly queue: DebounceQueue<RasterRequest, RasterResult>;

  constructor(options: L2PreviewOptions = {}) {
    const renderer = options.renderer ?? new FallbackRasterRenderer();
    this.queue = new DebounceQueue<RasterRequest, RasterResult>({
      debounceMs: options.debounceMs ?? 400,
      keyOf: rasterKey,
      run: (req, signal) => renderer.render({ ...req, signal }),
    });
  }

  preview(req: RasterRequest): Promise<QueueResult<RasterResult>> {
    return this.queue.schedule(req);
  }

  previewNow(req: RasterRequest): Promise<QueueResult<RasterResult>> {
    return this.queue.runNow(req);
  }

  get stats() {
    return this.queue.stats;
  }

  dispose(): void {
    this.queue.dispose();
  }
}
