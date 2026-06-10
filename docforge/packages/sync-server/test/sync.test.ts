import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { DocCore } from "@docforge/doc-core";
import { createSyncServer, SyncClient, type SyncServerHandle } from "../src/index.js";

let server: SyncServerHandle;
let url: string;

beforeAll(async () => {
  server = await createSyncServer({ port: 0 });
  url = `ws://${server.host}:${server.port}`;
});

afterAll(async () => {
  await server.close();
});

function wait(ms: number) {
  return new Promise((r) => setTimeout(r, ms));
}

/** 轮询直到条件满足或超时。 */
async function until(cond: () => boolean, timeout = 5000): Promise<void> {
  const start = Date.now();
  while (!cond()) {
    if (Date.now() - start > timeout) throw new Error("until: 超时");
    await wait(20);
  }
}

describe("sync-server 往返", () => {
  it("两个 client 连同一 room,文本双向收敛", async () => {
    const a = new SyncClient(url, "roomA").connect();
    const b = new SyncClient(url, "roomA").connect();
    await Promise.all([a.whenSynced, b.whenSynced]);

    a.doc.getText("t").insert(0, "hello");
    await until(() => b.doc.getText("t").toString() === "hello");
    expect(b.doc.getText("t").toString()).toBe("hello");

    b.doc.getText("t").insert(5, " world");
    await until(() => a.doc.getText("t").toString() === "hello world");
    expect(a.doc.getText("t").toString()).toBe("hello world");

    a.destroy();
    b.destroy();
  });

  it("通过 DocCore 协同:A insert_block,B 可见", async () => {
    const a = new SyncClient(url, "wordRoom").connect();
    const b = new SyncClient(url, "wordRoom").connect();
    await Promise.all([a.whenSynced, b.whenSynced]);

    const coreA = new DocCore({ type: "word", doc: a.doc });
    const coreB = new DocCore({ type: "word", doc: b.doc });

    const id = coreA.insert_block(null, { type: "heading", level: 1, text: "协同标题" });
    coreA.insert_block(id, { type: "paragraph", text: "协同正文" });

    await until(() => {
      const ir = coreB.read_document();
      return ir.type === "word" && ir.blocks.length === 2;
    });

    const irB = coreB.read_document();
    expect(irB.type).toBe("word");
    if (irB.type === "word") {
      expect(irB.blocks.map((x) => x.runs.map((r) => r.text).join(""))).toEqual([
        "协同标题",
        "协同正文",
      ]);
    }
    // 两端 hash 收敛一致
    expect(coreA.hash()).toBe(coreB.hash());

    a.destroy();
    b.destroy();
  });

  it("PPT 协同:A add_slide + edit_element,B 收敛", async () => {
    const a = new SyncClient(url, "pptRoom").connect();
    const b = new SyncClient(url, "pptRoom").connect();
    await Promise.all([a.whenSynced, b.whenSynced]);

    const coreA = new DocCore({ type: "ppt", doc: a.doc });
    const coreB = new DocCore({ type: "ppt", doc: b.doc });

    const s = coreA.add_slide(0, "title");
    const ir = coreA.read_document();
    if (ir.type === "ppt") {
      coreA.edit_element(s, ir.slides[0].elements[0].id, { text: "协同封面" });
    }

    await until(() => coreB.read_document().type === "ppt" && coreB.get_outline().length === 1);
    expect(coreB.get_outline()[0].text).toBe("协同封面");
    expect(coreA.hash()).toBe(coreB.hash());

    a.destroy();
    b.destroy();
  });

  it("room 隔离:不同 room 互不影响", async () => {
    const a = new SyncClient(url, "isoA").connect();
    const b = new SyncClient(url, "isoB").connect();
    await Promise.all([a.whenSynced, b.whenSynced]);

    a.doc.getText("t").insert(0, "only-A");
    await wait(200);
    expect(b.doc.getText("t").toString()).toBe("");

    a.destroy();
    b.destroy();
  });
});
