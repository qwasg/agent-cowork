import { WebSocketServer, type WebSocket } from "ws";
import type { IncomingMessage } from "node:http";
import { DocRoom } from "./room.js";

export interface SyncServerOptions {
  port?: number;
  host?: string;
  /** room 空闲(无连接)多久后回收,毫秒。默认 30s。 */
  roomGcMs?: number;
}

export interface SyncServerHandle {
  port: number;
  host: string;
  rooms: Map<string, DocRoom>;
  close: () => Promise<void>;
}

/** 从 ws 升级请求里解析 room 名(URL path)。 */
function roomNameFromRequest(req: IncomingMessage): string {
  const url = req.url ?? "/";
  const name = url.split("?")[0].replace(/^\/+/, "");
  return name || "default";
}

/**
 * 启动 localhost y-websocket room 服务。一个 doc 一个 room(按连接 URL path)。
 */
export function createSyncServer(options: SyncServerOptions = {}): Promise<SyncServerHandle> {
  const host = options.host ?? "127.0.0.1";
  const roomGcMs = options.roomGcMs ?? 30_000;
  const rooms = new Map<string, DocRoom>();
  const gcTimers = new Map<string, NodeJS.Timeout>();

  const wss = new WebSocketServer({ host, port: options.port ?? 0 });

  function getRoom(name: string): DocRoom {
    let room = rooms.get(name);
    if (!room) {
      room = new DocRoom(name);
      rooms.set(name, room);
    }
    const t = gcTimers.get(name);
    if (t) {
      clearTimeout(t);
      gcTimers.delete(name);
    }
    return room;
  }

  function scheduleGc(name: string): void {
    const room = rooms.get(name);
    if (!room || !room.isEmpty) return;
    const timer = setTimeout(() => {
      const r = rooms.get(name);
      if (r && r.isEmpty) {
        r.destroy();
        rooms.delete(name);
      }
      gcTimers.delete(name);
    }, roomGcMs);
    gcTimers.set(name, timer);
  }

  wss.on("connection", (conn: WebSocket, req: IncomingMessage) => {
    const name = roomNameFromRequest(req);
    const room = getRoom(name);
    conn.binaryType = "arraybuffer";
    room.addConnection(conn);

    conn.on("message", (data: ArrayBuffer | Buffer) => {
      const bytes =
        data instanceof ArrayBuffer
          ? new Uint8Array(data)
          : new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
      room.handleMessage(conn, bytes);
    });

    conn.on("close", () => {
      room.removeConnection(conn);
      scheduleGc(name);
    });
    conn.on("error", () => {
      room.removeConnection(conn);
      scheduleGc(name);
    });
  });

  return new Promise((resolve, reject) => {
    wss.on("error", reject);
    wss.on("listening", () => {
      const addr = wss.address();
      const port = typeof addr === "object" && addr ? addr.port : (options.port ?? 0);
      resolve({
        port,
        host,
        rooms,
        close: () =>
          new Promise<void>((res) => {
            for (const t of gcTimers.values()) clearTimeout(t);
            gcTimers.clear();
            for (const room of rooms.values()) room.destroy();
            rooms.clear();
            wss.close(() => res());
            for (const client of wss.clients) client.terminate();
          }),
      });
    });
  });
}
