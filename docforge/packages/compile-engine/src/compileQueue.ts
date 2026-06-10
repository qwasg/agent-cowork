import { contentHash, type DocIR, type ExportFormat } from "@docforge/doc-core";
import { compileToBuffer } from "./compiler.js";
import { DebounceQueue, type QueueResult } from "./queue.js";

export interface CompileJob {
  ir: DocIR;
  format: ExportFormat;
  /** PPT 预览只编当前 slide。 */
  onlySlideId?: string;
}

export function compileJobKey(job: CompileJob): string {
  const effective =
    job.onlySlideId !== undefined && job.ir.type === "ppt"
      ? { ...job.ir, slides: job.ir.slides.filter((s) => s.id === job.onlySlideId) }
      : job.ir;
  return `${job.format}:${job.onlySlideId ?? ""}:${contentHash(effective)}`;
}

export interface CompileQueueOptions {
  debounceMs?: number;
  maxCache?: number;
  /** 自定义执行体(默认进程内 compileToBuffer);可换成 sidecar.compile。 */
  run?: (job: CompileJob, signal: AbortSignal) => Promise<Buffer>;
}

/**
 * 编译队列:debounce + cancel + hash-cache。
 * 默认进程内编译;生产可注入 sidecar 执行体把编译放到 worker。
 */
export function createCompileQueue(
  options: CompileQueueOptions = {},
): DebounceQueue<CompileJob, Buffer> {
  const run =
    options.run ??
    ((job: CompileJob, _signal: AbortSignal) =>
      compileToBuffer(job.ir, job.format, { onlySlideId: job.onlySlideId }));
  return new DebounceQueue<CompileJob, Buffer>({
    debounceMs: options.debounceMs ?? 400,
    maxCache: options.maxCache,
    keyOf: compileJobKey,
    run,
  });
}

export type CompileQueueResult = QueueResult<Buffer>;
