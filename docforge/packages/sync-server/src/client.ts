import * as Y from "yjs";
import WebSocket from "ws";
import {
  MESSAGE_SYNC,
  MESSAGE_AWARENESS,
  encoding,
  decoding,
  syncProtocol,
  awarenessProtocol,
  encodeSyncStep1,
  encodeUpdate,
} from "./protocol.js";

const SYNC_STEP2 = 1; // y-protocols/sync messageYjsSyncStep2

/**
 * Node 端 y-websocket 客户端(供 sidecar / 测试使用)。
 * 浏览器端请用 y-websocket 的 WebsocketProvider —— 协议一致。
 */
export class SyncClient {
  readonly doc: Y.Doc;
  readonly awareness: awarenessProtocol.Awareness;
  private ws?: WebSocket;
  private synced = false;
  private resolveSynced!: () => void;
  readonly whenSynced: Promise<void>;

  constructor(
    private readonly url: string,
    private readonly room: string,
    doc?: Y.Doc,
  ) {
    this.doc = doc ?? new Y.Doc();
    this.awareness = new awarenessProtocol.Awareness(this.doc);
    this.whenSynced = new Promise((res) => (this.resolveSynced = res));
    this.doc.on("update", this.onLocalUpdate);
  }

  connect(): this {
    const full = `${this.url.replace(/\/$/, "")}/${this.room}`;
    const ws = new WebSocket(full);
    ws.binaryType = "arraybuffer";
    this.ws = ws;
    ws.on("open", () => ws.send(encodeSyncStep1(this.doc)));
    ws.on("message", (data: ArrayBuffer | Buffer) => {
      const bytes =
        data instanceof ArrayBuffer
          ? new Uint8Array(data)
          : new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
      this.onMessage(bytes);
    });
    return this;
  }

  private onLocalUpdate = (update: Uint8Array, origin: unknown) => {
    // origin === this 表示来自远端 applyUpdate,不再回发
    if (origin === this) return;
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(encodeUpdate(update));
    }
  };

  private onMessage(data: Uint8Array): void {
    const decoder = decoding.createDecoder(data);
    const messageType = decoding.readVarUint(decoder);
    if (messageType === MESSAGE_SYNC) {
      const encoder = encoding.createEncoder();
      encoding.writeVarUint(encoder, MESSAGE_SYNC);
      const syncType = syncProtocol.readSyncMessage(decoder, encoder, this.doc, this);
      if (encoding.length(encoder) > 1 && this.ws?.readyState === WebSocket.OPEN) {
        this.ws.send(encoding.toUint8Array(encoder));
      }
      if (syncType === SYNC_STEP2 && !this.synced) {
        this.synced = true;
        this.resolveSynced();
      }
    } else if (messageType === MESSAGE_AWARENESS) {
      awarenessProtocol.applyAwarenessUpdate(
        this.awareness,
        decoding.readVarUint8Array(decoder),
        this,
      );
    }
  }

  destroy(): void {
    this.doc.off("update", this.onLocalUpdate);
    this.ws?.close();
    this.awareness.destroy();
  }
}
