import { Worker } from "node:worker_threads";
import { fileURLToPath } from "node:url";
import type { DocIR, ExportFormat } from "@docforge/doc-core";
import type { WorkerRequest, WorkerResponse } from "./worker.js";

export interface CompileSidecarOptions {
  /** worker 入口;默认指向本包 worker.ts(dev/test 用 tsx 加载)。 */
  workerUrl?: URL;
  /** worker 进程额外参数;dev/test 需 tsx 加载 .ts。 */
  execArgv?: string[];
}

/**
 * 主线程一侧的编译 sidecar 客户端。把编译放进 worker_threads,
 * UI 线程只发消息 / 收 buffer,绝不在主线程跑 OOXML 序列化。
 */
export class CompileSidecar {
  private readonly worker: Worker;
  private seq = 0;
  private readonly pending = new Map<
    number,
    { resolve: (b: Buffer) => void; reject: (e: unknown) => void }
  >();

  constructor(options: CompileSidecarOptions = {}) {
    // 默认走 .mjs 引导(内部注册 tsx 再加载 worker.ts);生产可传入编译后的 worker.js。
    const url = options.workerUrl ?? new URL("./worker-bootstrap.mjs", import.meta.url);
    this.worker = new Worker(fileURLToPath(url), {
      execArgv: options.execArgv ?? [],
    });
    this.worker.on("message", (res: WorkerResponse) => {
      const p = this.pending.get(res.id);
      if (!p) return;
      this.pending.delete(res.id);
      if (res.ok && res.buffer) p.resolve(Buffer.from(res.buffer));
      else p.reject(new Error(res.error ?? "worker compile failed"));
    });
    this.worker.on("error", (err) => {
      for (const p of this.pending.values()) p.reject(err);
      this.pending.clear();
    });
  }

  compile(ir: DocIR, format: ExportFormat, onlySlideId?: string): Promise<Buffer> {
    const id = ++this.seq;
    const req: WorkerRequest = { id, ir, format, onlySlideId };
    return new Promise<Buffer>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.worker.postMessage(req);
    });
  }

  async dispose(): Promise<void> {
    await this.worker.terminate();
  }
}
