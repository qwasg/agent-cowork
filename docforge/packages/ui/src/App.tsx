import { useEffect, useMemo, useState } from "react";
import { createDocClient, type DocClient } from "./docClient.js";
import { WordEditor } from "./WordEditor.js";
import { PptEditor } from "./PptEditor.js";
import { useConnection } from "./hooks.js";
import { TauriPanel } from "./TauriPanel.js";

type Mode = "word" | "ppt";

export default function App() {
  const [mode, setMode] = useState<Mode>("word");
  return (
    <div className="app">
      <header className="app-header">
        <div className="brand">DocForge-Core</div>
        <nav className="mode-tabs">
          <button
            data-testid="tab-word"
            className={mode === "word" ? "active" : ""}
            onClick={() => setMode("word")}
          >
            Word
          </button>
          <button
            data-testid="tab-ppt"
            className={mode === "ppt" ? "active" : ""}
            onClick={() => setMode("ppt")}
          >
            PPT
          </button>
        </nav>
        <TauriPanel />
      </header>
      {/* 两种模式各自常驻一个 client,保持连接与本地状态 */}
      <Workspace mode={mode} />
    </div>
  );
}

function Workspace({ mode }: { mode: Mode }) {
  const wordClient = useMemo(() => createDocClient("word", "docforge-word"), []);
  const pptClient = useMemo(() => createDocClient("ppt", "docforge-ppt"), []);

  useEffect(() => {
    return () => {
      wordClient.destroy();
      pptClient.destroy();
    };
  }, [wordClient, pptClient]);

  const active: DocClient = mode === "word" ? wordClient : pptClient;

  return (
    <div className="workspace">
      <ConnBar client={active} />
      <main className="editor-pane">
        {mode === "word" ? <WordEditor client={wordClient} /> : <PptEditor client={pptClient} />}
      </main>
    </div>
  );
}

function ConnBar({ client }: { client: DocClient }) {
  const status = useConnection(client.provider);
  const label =
    status === "connected" ? "已连接" : status === "connecting" ? "连接中…" : "未连接";
  return (
    <div className="conn-bar" data-testid="conn-status" data-status={status}>
      <span className={`dot ${status}`} />
      <span>room: {client.room}</span>
      <span className="status-label">{label}</span>
    </div>
  );
}
