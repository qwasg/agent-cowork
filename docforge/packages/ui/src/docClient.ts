import * as Y from "yjs";
import { WebsocketProvider } from "y-websocket";
import { DocCore, type DocType } from "@docforge/doc-core";

export interface DocClient {
  core: DocCore;
  doc: Y.Doc;
  provider: WebsocketProvider;
  room: string;
  destroy: () => void;
}

/** 默认 sync-server 地址(可被 Tauri/env 覆盖)。 */
export function syncUrl(): string {
  const env = (import.meta as { env?: Record<string, string> }).env;
  return env?.VITE_SYNC_URL ?? "ws://127.0.0.1:1234";
}

/**
 * 创建一个连到 sync-server 的协同文档。
 * UI 与 agent 都通过同一 room 共写同一 doc;UI 只经 DocCore 写。
 */
export function createDocClient(type: DocType, room: string): DocClient {
  const doc = new Y.Doc();
  const provider = new WebsocketProvider(syncUrl(), room, doc, { connect: true });
  const core = new DocCore({ type, doc });

  return {
    core,
    doc,
    provider,
    room,
    destroy() {
      provider.destroy();
      core.destroy();
      doc.destroy();
    },
  };
}
