use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
};
use tauri::{Manager, State};

struct SidecarState {
    sync_child: Mutex<Option<Child>>,
}

#[derive(Serialize)]
struct ShellStatus {
    sync_running: bool,
    soffice_path: Option<String>,
}

fn workspace_root(app: &tauri::AppHandle) -> PathBuf {
    let exe = std::env::current_exe().ok();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // dev: packages/shell/src-tauri -> docforge
    if cwd.ends_with(Path::new("packages/shell/src-tauri")) {
        return cwd
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or(cwd);
    }
    // packaged/dev fallback: repository root from app resource dir if possible.
    if let Ok(resource) = app.path().resource_dir() {
        if resource.exists() {
            return resource;
        }
    }
    exe.and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or(cwd)
}

fn find_soffice() -> Option<String> {
    let candidates = [
        r"C:\Program Files\LibreOffice\program\soffice.exe",
        r"C:\Program Files (x86)\LibreOffice\program\soffice.exe",
    ];
    for path in candidates {
        if Path::new(path).exists() {
            return Some(path.to_string());
        }
    }
    None
}

fn spawn_pnpm(root: &Path, args: &[&str]) -> Result<Child, String> {
    Command::new("pnpm")
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn pnpm {:?}: {}", args, e))
}

#[tauri::command]
fn shell_status(state: State<'_, SidecarState>) -> ShellStatus {
    ShellStatus {
        sync_running: state.sync_child.lock().map(|g| g.is_some()).unwrap_or(false),
        soffice_path: find_soffice(),
    }
}

#[tauri::command]
fn start_sync_server(app: tauri::AppHandle, state: State<'_, SidecarState>) -> Result<(), String> {
    let mut guard = state
        .sync_child
        .lock()
        .map_err(|_| "sync state poisoned".to_string())?;
    if guard.is_some() {
        return Ok(());
    }
    let root = workspace_root(&app);
    let child = spawn_pnpm(
        &root,
        &["--filter", "@docforge/sync-server", "start"],
    )?;
    *guard = Some(child);
    Ok(())
}

#[tauri::command]
fn stop_sync_server(state: State<'_, SidecarState>) -> Result<(), String> {
    let mut guard = state
        .sync_child
        .lock()
        .map_err(|_| "sync state poisoned".to_string())?;
    if let Some(mut child) = guard.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    Ok(())
}

#[tauri::command]
fn export_demo(app: tauri::AppHandle) -> Result<String, String> {
    let root = workspace_root(&app);
    let output = Command::new("pnpm")
        .args(["--filter", "@docforge/compile-engine", "demo"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("failed to run compile demo: {}", e))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn run() {
    tauri::Builder::default()
        .manage(SidecarState {
            sync_child: Mutex::new(None),
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let state = app.state::<SidecarState>();
            let _ = start_sync_server(handle, state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            shell_status,
            start_sync_server,
            stop_sync_server,
            export_demo
        ])
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                if let Some(state) = window.try_state::<SidecarState>() {
                    let _ = stop_sync_server(state);
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running docforge shell");
}
