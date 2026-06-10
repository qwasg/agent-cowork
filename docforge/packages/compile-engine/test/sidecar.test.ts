import { describe, it, expect, afterAll } from "vitest";
import { DocCore } from "@docforge/doc-core";
import { CompileSidecar } from "../src/index.js";

let sidecar: CompileSidecar | undefined;

afterAll(async () => {
  await sidecar?.dispose();
});

describe("CompileSidecar(worker_threads)", () => {
  it("worker 中编译 Word,返回合法 .docx", async () => {
    sidecar = new CompileSidecar();
    const d = new DocCore({ type: "word" });
    d.insert_block(null, { type: "heading", level: 1, text: "Sidecar" });
    const buf = await sidecar.compile(d.read_document(), "docx");
    expect(buf[0]).toBe(0x50); // PK
    expect(buf.includes(Buffer.from("word/document.xml"))).toBe(true);
  });
});
