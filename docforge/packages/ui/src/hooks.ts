import { useEffect, useState } from "react";
import type { DocCore, DocIR } from "@docforge/doc-core";
import type { WebsocketProvider } from "y-websocket";

/** 订阅 doc-core,任意 mutation(本地/远端)后返回最新 IR。 */
export function useDocIR(core: DocCore): DocIR {
  const [ir, setIr] = useState<DocIR>(() => core.read_document());
  useEffect(() => {
    setIr(core.read_document());
    const off = core.observe(() => setIr(core.read_document()));
    return off;
  }, [core]);
  return ir;
}

export type ConnStatus = "connecting" | "connected" | "disconnected";

export function useConnection(provider: WebsocketProvider): ConnStatus {
  const [status, setStatus] = useState<ConnStatus>("connecting");
  useEffect(() => {
    const handler = (e: { status: ConnStatus }) => setStatus(e.status);
    provider.on("status", handler);
    return () => provider.off("status", handler);
  }, [provider]);
  return status;
}
