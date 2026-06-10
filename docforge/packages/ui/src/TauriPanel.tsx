import { useEffect, useState } from "react";

interface ShellStatus {
  sync_running: boolean;
  soffice_path: string | null;
}

async function invokeShell<T>(cmd: string): Promise<T> {
  const mod = await import("@tauri-apps/api/core");
  return mod.invoke<T>(cmd);
}

function isTauri(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

/** Shell 状态面板:仅在 Tauri 中显示。 */
export function TauriPanel() {
  const [enabled, setEnabled] = useState(false);
  const [status, setStatus] = useState<ShellStatus | null>(null);
  const [message, setMessage] = useState("");

  useEffect(() => {
    if (!isTauri()) return;
    setEnabled(true);
    invokeShell<ShellStatus>("shell_status")
      .then(setStatus)
      .catch((e) => setMessage(String(e)));
  }, []);

  if (!enabled) return null;

  async function refresh() {
    setStatus(await invokeShell<ShellStatus>("shell_status"));
  }

  async function startSync() {
    await invokeShell<void>("start_sync_server");
    await refresh();
  }

  async function exportDemo() {
    const out = await invokeShell<string>("export_demo");
    setMessage(out);
  }

  return (
    <div className="tauri-panel" data-testid="tauri-panel">
      <span>Shell</span>
      <span>sync: {status?.sync_running ? "running" : "stopped"}</span>
      <span>soffice: {status?.soffice_path ? "ok" : "missing"}</span>
      <button onClick={startSync}>启动 sync</button>
      <button onClick={exportDemo}>导出演示</button>
      {message && <code title={message}>{message.split("\n")[0]}</code>}
    </div>
  );
}
