import {
  Document,
  Packer,
  Paragraph,
  TextRun,
  HeadingLevel,
  type IParagraphOptions,
} from "docx";
import type { WordBlock, WordDocIR, WordTextRun } from "@docforge/doc-core";

const HEADING_BY_LEVEL: Record<number, (typeof HeadingLevel)[keyof typeof HeadingLevel]> = {
  1: HeadingLevel.HEADING_1,
  2: HeadingLevel.HEADING_2,
  3: HeadingLevel.HEADING_3,
  4: HeadingLevel.HEADING_4,
  5: HeadingLevel.HEADING_5,
  6: HeadingLevel.HEADING_6,
};

function buildRun(run: WordTextRun): TextRun {
  const marks = new Set(run.marks ?? []);
  return new TextRun({
    text: run.text,
    bold: marks.has("bold"),
    italics: marks.has("italic"),
    underline: marks.has("underline") ? {} : undefined,
    strike: marks.has("strike"),
    font: marks.has("code") ? "Consolas" : undefined,
  });
}

function buildParagraph(block: WordBlock): Paragraph {
  const options: IParagraphOptions = {
    children: block.runs.length ? block.runs.map(buildRun) : [new TextRun("")],
    ...(block.type === "heading"
      ? { heading: HEADING_BY_LEVEL[block.level ?? 1] ?? HeadingLevel.HEADING_1 }
      : {}),
  };
  return new Paragraph(options);
}

/** WordDocIR -> docx Document。 */
export function buildWordDocument(ir: WordDocIR): Document {
  const children = ir.blocks.map(buildParagraph);
  return new Document({
    sections: [{ properties: {}, children: children.length ? children : [new Paragraph("")] }],
  });
}

/** WordDocIR -> .docx 二进制(nodebuffer)。 */
export async function compileWord(ir: WordDocIR): Promise<Buffer> {
  const doc = buildWordDocument(ir);
  return Packer.toBuffer(doc);
}
