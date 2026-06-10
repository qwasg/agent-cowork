/**
 * y-websocket 兼容协议封装(基于 y-protocols)。
 * 消息格式:[varUint messageType][payload],与浏览器端 WebsocketProvider 一致。
 */
import * as encoding from "lib0/encoding";
import * as decoding from "lib0/decoding";
import * as syncProtocol from "y-protocols/sync";
import * as awarenessProtocol from "y-protocols/awareness";

export const MESSAGE_SYNC = 0;
export const MESSAGE_AWARENESS = 1;

export { encoding, decoding, syncProtocol, awarenessProtocol };

/** 构造 sync step1(用于握手)。 */
export function encodeSyncStep1(doc: import("yjs").Doc): Uint8Array {
  const encoder = encoding.createEncoder();
  encoding.writeVarUint(encoder, MESSAGE_SYNC);
  syncProtocol.writeSyncStep1(encoder, doc);
  return encoding.toUint8Array(encoder);
}

/** 构造一条 update 广播消息。 */
export function encodeUpdate(update: Uint8Array): Uint8Array {
  const encoder = encoding.createEncoder();
  encoding.writeVarUint(encoder, MESSAGE_SYNC);
  syncProtocol.writeUpdate(encoder, update);
  return encoding.toUint8Array(encoder);
}

/** 构造 awareness 广播消息。 */
export function encodeAwareness(
  awareness: awarenessProtocol.Awareness,
  clients: number[],
): Uint8Array {
  const encoder = encoding.createEncoder();
  encoding.writeVarUint(encoder, MESSAGE_AWARENESS);
  encoding.writeVarUint8Array(
    encoder,
    awarenessProtocol.encodeAwarenessUpdate(awareness, clients),
  );
  return encoding.toUint8Array(encoder);
}
