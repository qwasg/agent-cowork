import * as Y from "yjs";
import type { WebSocket } from "ws";
import {
  MESSAGE_SYNC,
  MESSAGE_AWARENESS,
  encoding,
  decoding,
  syncProtocol,
  awarenessProtocol,
  encodeSyncStep1,
  encodeUpdate,
  encodeAwareness,
} from "./protocol.js";

/**
 * 一个 doc 一个 room。维护权威 Y.Doc + awareness,
 * 把任意 conn 的更新广播给其余 conn(CRDT 收敛)。
 */
export class DocRoom {
  readonly name: string;
  readonly doc: Y.Doc;
  readonly awareness: awarenessProtocol.Awareness;
  readonly conns = new Map<WebSocket, Set<number>>();

  constructor(name: string) {
    this.name = name;
    this.doc = new Y.Doc();
    this.awareness = new awarenessProtocol.Awareness(this.doc);
    this.awareness.setLocalState(null);

    this.doc.on("update", this.onDocUpdate);
    this.awareness.on("update", this.onAwarenessUpdate);
  }

  private onDocUpdate = (update: Uint8Array, origin: unknown) => {
    const message = encodeUpdate(update);
    for (const conn of this.conns.keys()) {
      if (conn === origin) continue; // 不回送给来源
      this.sendRaw(conn, message);
    }
  };

  private onAwarenessUpdate = (
    changes: { added: number[]; updated: number[]; removed: number[] },
    origin: unknown,
  ) => {
    const { added, updated, removed } = changes;
    // 记录每个 conn 控制的 clientID,断连时清理
    const controlled = origin ? this.conns.get(origin as WebSocket) : undefined;
    if (controlled) {
      for (const id of added) controlled.add(id);
      for (const id of updated) controlled.add(id);
      for (const id of removed) controlled.delete(id);
    }
    const changed = [...added, ...updated, ...removed];
    const message = encodeAwareness(this.awareness, changed);
    for (const conn of this.conns.keys()) {
      this.sendRaw(conn, message);
    }
  };

  addConnection(conn: WebSocket): void {
    this.conns.set(conn, new Set());
    // 握手:发送 sync step1
    this.sendRaw(conn, encodeSyncStep1(this.doc));
    // 推送现有 awareness 状态
    const states = this.awareness.getStates();
    if (states.size > 0) {
      this.sendRaw(conn, encodeAwareness(this.awareness, [...states.keys()]));
    }
  }

  handleMessage(conn: WebSocket, data: Uint8Array): void {
    const decoder = decoding.createDecoder(data);
    const messageType = decoding.readVarUint(decoder);
    switch (messageType) {
      case MESSAGE_SYNC: {
        const encoder = encoding.createEncoder();
        encoding.writeVarUint(encoder, MESSAGE_SYNC);
        // origin = conn,避免把更新回送给自己
        syncProtocol.readSyncMessage(decoder, encoder, this.doc, conn);
        if (encoding.length(encoder) > 1) {
          this.sendRaw(conn, encoding.toUint8Array(encoder));
        }
        break;
      }
      case MESSAGE_AWARENESS: {
        const update = decoding.readVarUint8Array(decoder);
        awarenessProtocol.applyAwarenessUpdate(this.awareness, update, conn);
        break;
      }
      default:
        break;
    }
  }

  removeConnection(conn: WebSocket): void {
    const controlled = this.conns.get(conn);
    this.conns.delete(conn);
    if (controlled && controlled.size > 0) {
      awarenessProtocol.removeAwarenessStates(this.awareness, [...controlled], null);
    }
  }

  get isEmpty(): boolean {
    return this.conns.size === 0;
  }

  destroy(): void {
    this.doc.off("update", this.onDocUpdate);
    this.awareness.off("update", this.onAwarenessUpdate);
    this.awareness.destroy();
    this.doc.destroy();
  }

  private sendRaw(conn: WebSocket, message: Uint8Array): void {
    if (conn.readyState !== 1 /* OPEN */) return;
    try {
      conn.send(message);
    } catch {
      this.removeConnection(conn);
    }
  }
}
