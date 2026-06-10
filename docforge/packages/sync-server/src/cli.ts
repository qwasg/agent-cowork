#!/usr/bin/env node
import { createSyncServer } from "./server.js";

const port = Number(process.env.DOCFORGE_SYNC_PORT ?? process.argv[2] ?? 1234);
const host = process.env.DOCFORGE_SYNC_HOST ?? "127.0.0.1";

createSyncServer({ port, host }).then((handle) => {
  console.log(`[docforge-sync] listening ws://${handle.host}:${handle.port}`);
  const shutdown = () => {
    console.log("[docforge-sync] shutting down");
    handle.close().then(() => process.exit(0));
  };
  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);
});
