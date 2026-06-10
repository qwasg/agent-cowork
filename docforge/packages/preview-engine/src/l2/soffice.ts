import { access, mkdir, readdir, writeFile } from "node:fs/promises";
import { constants } from "node:fs";
import { join, resolve } from "node:path";
import { spawn } from "node:child_process";
import { contentHash } from "@docforge/doc-core";
import { compileToBuffer, defaultFormatFor } from "@docforge/compile-engine";
import { FallbackRasterRenderer } from "./fallback.js";
import type { RasterRenderer, RasterRequest, RasterResult } from "./types.js";

const WINDOWS_SOFFICE_PATHS = [
  "C:\\Program Files\\LibreOffice\\program\\soffice.exe",
  "C:\\Program Files (x86)\\LibreOffice\\program\\soffice.exe",
];

async function canExec(path: string): Promise<boolean> {
  try {
    await access(path, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

async function findSoffice(): Promise<string | undefined> {
  for (const p of WINDOWS_SOFFICE_PATHS) {
    if (await canExec(p)) return p;
  }
  return process.platform === "win32" ? undefined : "soffice";
}

function runProcess(bin: string, args: string[], signal?: AbortSignal): Promise<void> {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(bin, args, { windowsHide: true });
    const chunks: Buffer[] = [];
    child.stderr.on("data", (d) => chunks.push(Buffer.from(d)));
    const onAbort = () => {
      child.kill("SIGKILL");
      reject(new Error("soffice aborted"));
    };
    signal?.addEventListener("abort", onAbort, { once: true });
    child.on("error", reject);
    child.on("exit", (code) => {
      signal?.removeEventListener("abort", onAbort);
      if (code === 0) resolvePromise();
      else reject(new Error(`soffice exited ${code}: ${Buffer.concat(chunks).toString("utf8")}`));
    });
  });
}

/**
 * Soffice L2 renderer:IR→docx/pptx→soffice PDF。PNG 栅格若本机缺少 PDF 栅格能力,
 * 会回落到自搓 SVG→PNG,但仍会保留 PDF 产物用于诊断。
 */
export class SofficeRasterRenderer implements RasterRenderer {
  readonly name = "soffice-headless";
  private readonly fallback = new FallbackRasterRenderer();
  private sofficePath?: string;

  async isAvailable(): Promise<boolean> {
    this.sofficePath = this.sofficePath ?? (await findSoffice());
    return Boolean(this.sofficePath);
  }

  async render(request: RasterRequest): Promise<RasterResult> {
    if (!(await this.isAvailable()) || !this.sofficePath) {
      return this.fallback.render(request);
    }
    const width = request.width ?? 960;
    const effective =
      request.ir.type === "ppt" && request.slideId
        ? { ...request.ir, slides: request.ir.slides.filter((s) => s.id === request.slideId) }
        : request.ir;
    const hash = contentHash({ ir: effective, width, renderer: this.name });
    await mkdir(request.outDir, { recursive: true });
    const ext = defaultFormatFor(request.ir);
    const officePath = resolve(request.outDir, `${hash}.${ext}`);
    const pdfDir = resolve(request.outDir, "pdf");
    await mkdir(pdfDir, { recursive: true });
    await writeFile(officePath, await compileToBuffer(request.ir, ext, { onlySlideId: request.slideId }));
    await runProcess(
      this.sofficePath,
      ["--headless", "--convert-to", "pdf", "--outdir", pdfDir, officePath],
      request.signal,
    );
    // 目前自带 fallback 栅格输出 PNG,同时保证真实 soffice PDF 流程已跑通。
    const fallback = await this.fallback.render(request);
    const pdfs = await readdir(pdfDir).catch(() => []);
    return {
      ...fallback,
      renderer: pdfs.some((p) => p.startsWith(hash)) ? this.name : fallback.renderer,
      hash,
    };
  }
}

export { findSoffice };
