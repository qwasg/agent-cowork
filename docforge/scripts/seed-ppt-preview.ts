import { readFileSync } from "node:fs";
import { applyIRToDoc, fromJSON } from "../packages/doc-core/src/index.ts";
import { SyncClient } from "../packages/sync-server/src/client.ts";

const irPath =
  process.argv[2] ??
  "H:/agent-debug-frontend-backend-copy-20260530/out-simple/docs/api-test.pptx.ir.json";
const room = process.argv[3] ?? "docforge-ppt";
const url = process.env.VITE_SYNC_URL ?? "ws://127.0.0.1:1234";

const ir = fromJSON(readFileSync(irPath, "utf8"));
if (ir.type !== "ppt") {
  throw new Error(`expected ppt IR, got ${ir.type}`);
}

const client = new SyncClient(url, room).connect();
await client.whenSynced;

client.doc.transact(() => {
  const root = client.doc.getArray("ppt");
  while (root.length > 0) {
    root.delete(0, 1);
  }
  applyIRToDoc(client.doc, ir);
});

await new Promise((resolve) => setTimeout(resolve, 300));
console.log(`[seed-ppt] loaded ${ir.slides.length} slide(s) into room "${room}"`);
client.destroy();
