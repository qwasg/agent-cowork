//! Workspace tree service (port of `domain/workspace_tree.py`): single-level
//! directory listings annotated with git status, branch detection, heavy-dir
//! filtering and a real `git checkout HEAD -- <path>` revert.
//!
//! Git operations shell out to the system `git` CLI (same behaviour as the
//! Python reference) and degrade gracefully on non-git workspaces.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

use agent_protocol::{ApiError, ApiResult};

/// Directories hidden from the tree unless `showHeavy=true`. `.git` is always
/// skipped.
const HEAVY_DIRS: &[&str] = &[
    "node_modules",
    "__pycache__",
    "target",
    "dist",
    "build",
    ".venv",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    "htmlcov",
];

fn status_priority(status: &str) -> i32 {
    match status {
        "M" => 5,
        "A" => 4,
        "D" | "R" | "C" => 3,
        "U" => 2,
        "?" => 1,
        _ => 0,
    }
}

/// `git status --porcelain=v1 -uall` → `{relPath -> singleCharStatus}`.
/// Empty map when git is unavailable or the root isn't a repo.
pub fn git_statuses(root: &Path) -> HashMap<String, String> {
    if !root.join(".git").exists() {
        return HashMap::new();
    }
    let output = Command::new("git")
        .args(["status", "--porcelain=v1", "-uall"])
        .current_dir(root)
        .output();
    match output {
        Ok(out) if out.status.success() => parse_porcelain(&String::from_utf8_lossy(&out.stdout)),
        _ => HashMap::new(),
    }
}

fn parse_porcelain(text: &str) -> HashMap<String, String> {
    let mut statuses = HashMap::new();
    for raw in text.lines() {
        if raw.len() < 4 {
            continue;
        }
        let bytes: Vec<char> = raw.chars().collect();
        let (x, y, sep) = (bytes[0], bytes[1], bytes[2]);
        if sep != ' ' {
            continue;
        }
        let mut path: &str = &raw[3..];
        if let Some((_, dest)) = path.split_once(" -> ") {
            path = dest;
        }
        let path = path.trim().trim_matches('"').replace('\\', "/");
        if path.is_empty() {
            continue;
        }
        let status = if x == '?' || y == '?' {
            "U".to_string()
        } else if x == '!' || y == '!' {
            "!".to_string()
        } else {
            let primary = if x != ' ' { x } else { y };
            match primary {
                'R' | 'C' => "M".to_string(),
                'M' | 'A' | 'D' => primary.to_string(),
                _ => "M".to_string(),
            }
        };
        statuses.insert(path, status);
    }
    statuses
}

/// Highest-priority status across files under `rel_dir/` (folder indicator).
fn aggregate_dir_status(rel_dir: &str, statuses: &HashMap<String, String>) -> Option<String> {
    if rel_dir.is_empty() {
        return None;
    }
    let prefix = format!("{rel_dir}/");
    let mut best: Option<&String> = None;
    let mut best_rank = -1;
    for (path, status) in statuses {
        if !path.starts_with(&prefix) {
            continue;
        }
        let rank = status_priority(status);
        if rank > best_rank {
            best = Some(status);
            best_rank = rank;
        }
    }
    best.cloned()
}

/// Current branch from `.git/HEAD` (no subprocess; mirrors `_read_branch`).
pub fn read_branch(root: &Path) -> Option<String> {
    let head = root.join(".git").join("HEAD");
    let content = std::fs::read_to_string(head).ok()?.trim().to_string();
    if let Some(rest) = content.strip_prefix("ref:") {
        let r = rest.trim();
        return Some(r.strip_prefix("refs/heads/").unwrap_or(r).to_string());
    }
    Some("detached".to_string())
}

/// Workspace info payload (`GET /workspace/info`), Python shape:
/// `{root, branch}`.
pub fn workspace_info(root: &Path) -> Value {
    json!({
        "root": root.to_string_lossy(),
        "branch": read_branch(root),
    })
}

/// Single-level listing with git annotation (`GET /workspace/tree`), Python
/// shape: `{root, relPath, entries: [{name, kind, relPath, size, modifiedAt,
/// gitStatus, hidden}], gitBranch}`.
pub fn list_tree(root: &Path, rel_path: &str, show_heavy: bool) -> ApiResult<Value> {
    let target = resolve_rel(root, rel_path)?;
    if !target.exists() {
        return Err(ApiError::path_not_found(rel_path));
    }
    if !target.is_dir() {
        return Err(ApiError::new("PATH_NOT_DIRECTORY", "not a directory"));
    }
    let statuses = git_statuses(root);
    let mut entries: Vec<Value> = Vec::new();
    let rd = std::fs::read_dir(&target).map_err(|_| ApiError::path_not_found(rel_path))?;
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".git" {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let is_dir = file_type.is_dir();
        let heavy = HEAVY_DIRS.contains(&name.as_str());
        if heavy && !show_heavy {
            continue;
        }
        let hidden = name.starts_with('.') || heavy;
        let meta = entry.metadata().ok();
        let rel_entry = if rel_path.trim().is_empty() {
            name.clone()
        } else {
            format!("{}/{}", rel_path.trim().trim_end_matches('/'), name)
        }
        .replace('\\', "/");
        let git_status = if is_dir {
            aggregate_dir_status(&rel_entry, &statuses)
        } else {
            statuses.get(&rel_entry).cloned()
        };
        let modified_at = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64());
        entries.push(json!({
            "name": name,
            "kind": if is_dir { "dir" } else { "file" },
            "relPath": rel_entry,
            "size": if is_dir { Value::Null } else { json!(meta.as_ref().map(|m| m.len())) },
            "modifiedAt": modified_at,
            "gitStatus": git_status,
            "hidden": hidden,
        }));
    }
    entries.sort_by(|a, b| {
        let dir_rank = |e: &Value| if e["kind"] == "dir" { 0 } else { 1 };
        dir_rank(a).cmp(&dir_rank(b)).then_with(|| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
        })
    });
    Ok(json!({
        "root": root.to_string_lossy(),
        "relPath": rel_path,
        "entries": entries,
        "gitBranch": read_branch(root),
    }))
}

/// Restore a single workspace file to HEAD via `git checkout HEAD -- <rel>`.
pub fn checkout_head(root: &Path, path: &str) -> ApiResult<Value> {
    let abs = agent_tools::resolve_in_root(&root.to_path_buf(), path)?;
    if abs.is_dir() {
        return Err(ApiError::new("PATH_IS_DIRECTORY", format!("{path} 是目录")));
    }
    if !root.join(".git").exists() {
        return Err(ApiError::new(
            "NOT_A_GIT_REPO",
            "workspace 不是 git 仓库，无法回滚到 HEAD",
        ));
    }
    let rel = abs
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.replace('\\', "/"));
    let output = Command::new("git")
        .args(["checkout", "HEAD", "--", &rel])
        .current_dir(root)
        .output()
        .map_err(|_| ApiError::new("GIT_ERROR", "未找到 git 可执行程序"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let msg = if stderr.is_empty() {
            "git checkout 失败".to_string()
        } else {
            stderr
        };
        return Err(ApiError::new("GIT_ERROR", msg));
    }
    Ok(json!({ "path": rel, "reverted": true }))
}

fn resolve_rel(root: &Path, rel_path: &str) -> ApiResult<PathBuf> {
    let cleaned = rel_path.trim();
    if cleaned.is_empty() {
        return Ok(root.to_path_buf());
    }
    if cleaned.starts_with('/') || cleaned.starts_with('\\') || PathBuf::from(cleaned).is_absolute()
    {
        return Err(ApiError::path_outside_root(rel_path));
    }
    agent_tools::resolve_in_root(&root.to_path_buf(), cleaned)
}

/// Directory browser for the workspace picker (`GET /workspace/browse`),
/// Python shape: `{path, parent, separator, drives, places, entries}`.
pub fn browse(path: &str) -> ApiResult<Value> {
    let cleaned = path.trim().to_string();
    let drives = windows_drives();
    let places = quick_places();

    if cleaned.is_empty() {
        if !drives.is_empty() {
            let entries: Vec<Value> = drives
                .iter()
                .map(|d| json!({ "name": d, "path": d, "hasChildren": true }))
                .collect();
            return Ok(json!({
                "path": "",
                "parent": Value::Null,
                "separator": std::path::MAIN_SEPARATOR.to_string(),
                "drives": drives,
                "places": places,
                "entries": entries,
            }));
        }
    }
    let cleaned = if cleaned.is_empty() {
        std::path::MAIN_SEPARATOR.to_string()
    } else {
        cleaned
    };
    let target = dunce::canonicalize(&cleaned).map_err(|_| ApiError::path_not_found(&cleaned))?;
    if !target.is_dir() {
        return Err(ApiError::new("PATH_NOT_DIRECTORY", "not a directory"));
    }
    let mut entries: Vec<Value> = Vec::new();
    let rd = std::fs::read_dir(&target)
        .map_err(|e| ApiError::new("FILESYSTEM_ERROR", format!("无法读取目录: {e}")))?;
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let child = entry.path();
        let has_children = std::fs::read_dir(&child)
            .map(|mut sub| {
                sub.any(|e| {
                    e.ok()
                        .map(|e| {
                            e.file_type().map(|t| t.is_dir()).unwrap_or(false)
                                && !e.file_name().to_string_lossy().starts_with('.')
                        })
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        entries.push(json!({
            "name": name,
            "path": child.to_string_lossy(),
            "hasChildren": has_children,
        }));
    }
    entries.sort_by_key(|e| e["name"].as_str().unwrap_or("").to_lowercase());

    let parent = target.parent();
    let parent_value = match parent {
        None => {
            if drives.is_empty() {
                Value::Null
            } else {
                json!("")
            }
        }
        Some(p) if p == target => Value::Null,
        Some(p) => json!(p.to_string_lossy()),
    };

    Ok(json!({
        "path": target.to_string_lossy(),
        "parent": parent_value,
        "separator": std::path::MAIN_SEPARATOR.to_string(),
        "drives": drives,
        "places": places,
        "entries": entries,
    }))
}

#[cfg(windows)]
fn windows_drives() -> Vec<String> {
    ('A'..='Z')
        .map(|l| format!("{l}:\\"))
        .filter(|root| Path::new(root).exists())
        .collect()
}

#[cfg(not(windows))]
fn windows_drives() -> Vec<String> {
    Vec::new()
}

fn quick_places() -> Vec<Value> {
    let mut places = Vec::new();
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        let home = PathBuf::from(home);
        for candidate in [home.join("Desktop"), home.join("OneDrive").join("Desktop")] {
            if candidate.is_dir() {
                places.push(json!({
                    "name": "桌面",
                    "path": candidate.to_string_lossy(),
                    "icon": "monitor",
                }));
                break;
            }
        }
    }
    places
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_statuses() {
        let text = " M src/a.rs\n?? new.txt\nA  staged.rs\nR  old.rs -> renamed.rs\n";
        let map = parse_porcelain(text);
        assert_eq!(map.get("src/a.rs").map(String::as_str), Some("M"));
        assert_eq!(map.get("new.txt").map(String::as_str), Some("U"));
        assert_eq!(map.get("staged.rs").map(String::as_str), Some("A"));
        assert_eq!(map.get("renamed.rs").map(String::as_str), Some("M"));
    }

    #[test]
    fn aggregates_directory_status_by_priority() {
        let mut map = HashMap::new();
        map.insert("src/a.rs".to_string(), "U".to_string());
        map.insert("src/b.rs".to_string(), "M".to_string());
        assert_eq!(aggregate_dir_status("src", &map), Some("M".to_string()));
        assert_eq!(aggregate_dir_status("other", &map), None);
    }

    #[test]
    fn tree_lists_and_filters_heavy_dirs() {
        let root =
            std::env::temp_dir().join(format!("agentd_ws_{}", agent_protocol::models::new_id("t")));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("node_modules")).unwrap();
        std::fs::write(root.join("a.txt"), "hi").unwrap();
        let tree = list_tree(&root, "", false).unwrap();
        let names: Vec<&str> = tree["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"src"));
        assert!(names.contains(&"a.txt"));
        assert!(
            !names.contains(&"node_modules"),
            "heavy dirs hidden by default"
        );
        let tree = list_tree(&root, "", true).unwrap();
        let names: Vec<&str> = tree["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"node_modules"));
    }

    #[test]
    fn checkout_head_requires_git_repo() {
        let root = std::env::temp_dir().join(format!(
            "agentd_nogit_{}",
            agent_protocol::models::new_id("t")
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("f.txt"), "x").unwrap();
        let err = checkout_head(&root, "f.txt").unwrap_err();
        assert_eq!(err.code, "NOT_A_GIT_REPO");
    }
}
