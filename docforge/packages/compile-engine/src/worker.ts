/**
 * 编译 worker(worker_threads sidecar)。绝不阻塞 UI 线程。
 * 协议:主线程 postMessage({ id, ir, format, onlySlideId });
 *       worker 回 postMessage({ id, ok, buffer? , error? })。
 */
import { parentPort } from "node:worker_threads";
import type { DocIR, ExportFormat } from "@docforge/doc-core";
import { compileToBuffer } from "./compiler.js";

export interface WorkerRequest {
  id: number;
  ir: DocIR;
  format: ExportFormat;
  onlySlideId?: string;
}

export interface WorkerResponse {
  id: number;
  ok: boolean;
  buffer?: ArrayBuffer;
  error?: string;
}

if (parentPort) {
  parentPort.on("message", async (req: WorkerRequest) => {
    try {
      const buf = await compileToBuffer(req.ir, req.format, { onlySlideId: req.onlySlideId });
      const ab = buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength);
      const res: WorkerResponse = { id: req.id, ok: true, buffer: ab };
      parentPort!.postMessage(res, [ab]);
    } catch (err) {
      const res: WorkerResponse = {
        id: req.id,
        ok: false,
        error: err instanceof Error ? err.message : String(err),
      };
      parentPort!.postMessage(res);
    }
  });
}
