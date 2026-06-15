//! Structured edit primitives: exact string replacement (`str_replace_edit`)
//! and a Codex-style multi-hunk patch format (`apply_patch`).
//!
//! These are pure text transforms; the runtime (agent-core) handles the
//! filesystem writes and the proposal recording around them.

/// Exact `old → new` replacement with uniqueness validation.
pub fn str_replace(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<String, String> {
    if old.is_empty() {
        return Err("old_string 不能为空".to_string());
    }
    if old == new {
        return Err("old_string 与 new_string 相同，无需修改".to_string());
    }
    let count = content.matches(old).count();
    if count == 0 {
        return Err(
            "old_string 在文件中不存在（请确保与原文完全一致，包括缩进与空白）".to_string(),
        );
    }
    if count > 1 && !replace_all {
        return Err(format!(
            "old_string 出现了 {count} 次，无法唯一定位；请扩大上下文使其唯一，或设置 replace_all=true"
        ));
    }
    Ok(if replace_all {
        content.replace(old, new)
    } else {
        content.replacen(old, new, 1)
    })
}

/// One contiguous patch hunk: `find` (context + removed lines) is located in
/// the file and replaced by `replace` (context + added lines).
#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    pub find: Vec<String>,
    pub replace: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FileOp {
    Add { path: String, content: String },
    Delete { path: String },
    Update { path: String, hunks: Vec<Hunk> },
}

impl FileOp {
    pub fn path(&self) -> &str {
        match self {
            FileOp::Add { path, .. } | FileOp::Delete { path } | FileOp::Update { path, .. } => {
                path
            }
        }
    }
}

/// Parse the Codex `apply_patch` envelope:
///
/// ```text
/// *** Begin Patch
/// *** Add File: a/new.txt
/// +line 1
/// *** Update File: b/exist.txt
/// @@ optional locator
///  context
/// -removed
/// +added
/// *** Delete File: c/old.txt
/// *** End Patch
/// ```
pub fn parse_patch(text: &str) -> Result<Vec<FileOp>, String> {
    let body = text.trim();
    let mut lines = body.lines().peekable();
    match lines.next() {
        Some(l) if l.trim() == "*** Begin Patch" => {}
        _ => return Err("patch 必须以 '*** Begin Patch' 开头".to_string()),
    }

    let mut ops: Vec<FileOp> = Vec::new();
    while let Some(line) = lines.next() {
        let line = line.trim_end();
        if line.trim() == "*** End Patch" {
            return if ops.is_empty() {
                Err("patch 不包含任何文件操作".to_string())
            } else {
                Ok(ops)
            };
        }
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            let mut content = String::new();
            while let Some(next) = lines.peek() {
                if next.starts_with("*** ") {
                    break;
                }
                let next = lines.next().unwrap();
                let added = next
                    .strip_prefix('+')
                    .ok_or_else(|| format!("Add File 段中的行必须以 '+' 开头: {next:?}"))?;
                content.push_str(added);
                content.push('\n');
            }
            ops.push(FileOp::Add {
                path: path.trim().to_string(),
                content,
            });
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            ops.push(FileOp::Delete {
                path: path.trim().to_string(),
            });
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            let mut hunks: Vec<Hunk> = Vec::new();
            let mut find: Vec<String> = Vec::new();
            let mut replace: Vec<String> = Vec::new();
            let mut in_hunk = false;
            while let Some(next) = lines.peek() {
                if next.starts_with("*** ") {
                    break;
                }
                let next = lines.next().unwrap();
                if next.starts_with("@@") {
                    if in_hunk && (!find.is_empty() || !replace.is_empty()) {
                        hunks.push(Hunk {
                            find: std::mem::take(&mut find),
                            replace: std::mem::take(&mut replace),
                        });
                    }
                    in_hunk = true;
                    continue;
                }
                in_hunk = true;
                if let Some(ctx) = next.strip_prefix(' ') {
                    find.push(ctx.to_string());
                    replace.push(ctx.to_string());
                } else if let Some(del) = next.strip_prefix('-') {
                    find.push(del.to_string());
                } else if let Some(add) = next.strip_prefix('+') {
                    replace.push(add.to_string());
                } else if next.is_empty() {
                    // Blank context line (some models omit the leading space).
                    find.push(String::new());
                    replace.push(String::new());
                } else {
                    return Err(format!(
                        "Update File 段中的行必须以 ' '、'-'、'+' 或 '@@' 开头: {next:?}"
                    ));
                }
            }
            if !find.is_empty() || !replace.is_empty() {
                hunks.push(Hunk { find, replace });
            }
            if hunks.is_empty() {
                return Err(format!("Update File {path} 不包含任何 hunk"));
            }
            ops.push(FileOp::Update {
                path: path.trim().to_string(),
                hunks,
            });
        } else if line.trim().is_empty() {
            continue;
        } else {
            return Err(format!("无法识别的 patch 指令: {line:?}"));
        }
    }
    Err("patch 缺少 '*** End Patch' 结尾".to_string())
}

/// Apply update hunks to file content. Each hunk's `find` block must occur
/// exactly once as contiguous lines.
pub fn apply_hunks(content: &str, hunks: &[Hunk]) -> Result<String, String> {
    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    let had_trailing_newline = content.ends_with('\n') || content.is_empty();

    for (i, hunk) in hunks.iter().enumerate() {
        if hunk.find.is_empty() {
            // Pure addition with no context: append at end.
            lines.extend(hunk.replace.iter().cloned());
            continue;
        }
        let positions: Vec<usize> = (0..=lines.len().saturating_sub(hunk.find.len()))
            .filter(|&start| {
                lines[start..start + hunk.find.len()]
                    .iter()
                    .zip(&hunk.find)
                    .all(|(a, b)| a == b)
            })
            .collect();
        match positions.len() {
            0 => {
                return Err(format!(
                    "第 {} 个 hunk 的上下文在文件中不存在（请确保 context/删除行与原文完全一致）",
                    i + 1
                ))
            }
            1 => {
                let start = positions[0];
                lines.splice(start..start + hunk.find.len(), hunk.replace.iter().cloned());
            }
            n => {
                return Err(format!(
                    "第 {} 个 hunk 的上下文出现了 {n} 次，无法唯一定位；请增加上下文行",
                    i + 1
                ))
            }
        }
    }

    let mut out = lines.join("\n");
    if had_trailing_newline && !out.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn str_replace_unique() {
        let out = str_replace(
            "let a = 1;\nlet b = 2;\n",
            "let b = 2;",
            "let b = 3;",
            false,
        )
        .unwrap();
        assert_eq!(out, "let a = 1;\nlet b = 3;\n");
    }

    #[test]
    fn str_replace_rejects_ambiguous_and_missing() {
        let err = str_replace("x\nx\n", "x", "y", false).unwrap_err();
        assert!(err.contains("2 次"));
        assert!(str_replace("a\n", "zzz", "y", false).is_err());
        // replace_all resolves ambiguity.
        assert_eq!(str_replace("x\nx\n", "x", "y", true).unwrap(), "y\ny\n");
    }

    #[test]
    fn parse_and_apply_full_patch() {
        let patch = r#"*** Begin Patch
*** Add File: docs/new.md
+# Title
+body
*** Update File: src/main.rs
@@
 fn main() {
-    println!("old");
+    println!("new");
 }
*** Delete File: tmp.txt
*** End Patch"#;
        let ops = parse_patch(patch).unwrap();
        assert_eq!(ops.len(), 3);
        assert_eq!(ops[0].path(), "docs/new.md");
        match &ops[0] {
            FileOp::Add { content, .. } => assert_eq!(content, "# Title\nbody\n"),
            _ => panic!("expected add"),
        }
        match &ops[1] {
            FileOp::Update { hunks, .. } => {
                let updated =
                    apply_hunks("fn main() {\n    println!(\"old\");\n}\n", hunks).unwrap();
                assert_eq!(updated, "fn main() {\n    println!(\"new\");\n}\n");
            }
            _ => panic!("expected update"),
        }
        assert_eq!(
            ops[2],
            FileOp::Delete {
                path: "tmp.txt".into()
            }
        );
    }

    #[test]
    fn apply_hunks_rejects_missing_context() {
        let hunks = vec![Hunk {
            find: vec!["not in file".into()],
            replace: vec!["whatever".into()],
        }];
        assert!(apply_hunks("line1\nline2\n", &hunks).is_err());
    }

    #[test]
    fn parse_rejects_malformed() {
        assert!(parse_patch("hello").is_err());
        assert!(parse_patch("*** Begin Patch\n*** End Patch").is_err());
        assert!(parse_patch("*** Begin Patch\n*** Update File: a.rs\n@@\n x").is_err());
    }
}
