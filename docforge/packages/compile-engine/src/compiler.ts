import { writeFile, mkdir } from "node:fs/promises";
import { dirname, join } from "node:path";
import { contentHash, type DocIR, type ExportFormat, type Exporter } from "@docforge/doc-core";
import { compileWord } from "./compileWord.js";
import { compilePpt } from "./compilePpt.js";

export interface CompileOptions {
  /** PPT:仅编译该 slide(预览用)。 */
  onlySlideId?: string;
}

/** IR -> 二进制产物(.docx / .pptx / .json)。 */
export async function compileToBuffer(
  ir: DocIR,
  format: ExportFormat,
  options: CompileOptions = {},
): Promise<Buffer> {
  if (format === "json") {
    return Buffer.from(JSON.stringify(ir, null, 2), "utf8");
  }
  if (format === "docx") {
    if (ir.type !== "word") throw new Error("docx 仅支持 word 文档");
    return compileWord(ir);
  }
  if (format === "pptx") {
    if (ir.type !== "ppt") throw new Error("pptx 仅支持 ppt 文档");
    return compilePpt(ir, options.onlySlideId);
  }
  throw new Error(`未知导出格式: ${format}`);
}

/** 文档类型对应的默认导出格式。 */
export function defaultFormatFor(ir: DocIR): ExportFormat {
  return ir.type === "word" ? "docx" : "pptx";
}

/** IR -> 写入文件,返回 filepath。 */
export async function exportToFile(
  ir: DocIR,
  format: ExportFormat,
  outPath: string,
  options: CompileOptions = {},
): Promise<string> {
  const buf = await compileToBuffer(ir, format, options);
  await mkdir(dirname(outPath), { recursive: true });
  await writeFile(outPath, buf);
  return outPath;
}

/**
 * 生成可注入 doc-core 的 Exporter:export(format) -> filepath。
 * 文件名按内容 hash,便于复用/缓存。
 */
export function createExporter(outDir: string): Exporter {
  return async (ir: DocIR, format: ExportFormat): Promise<string> => {
    const ext = format === "json" ? "json" : format;
    const name = `${ir.type}-${contentHash(ir)}.${ext}`;
    return exportToFile(ir, format, join(outDir, name));
  };
}
