//! Append-only per-session conversation log (`{dir}/{id}/rollout.jsonl`).
//!
//! Source of truth for conversation history (redb keeps only session
//! metadata). Appends are O(1); fork is a file copy; revert is truncation at
//! a turn boundary; crash recovery is "parse what's there".

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use agent_protocol::rollout::RolloutItem;

pub struct RolloutStore {
    dir: PathBuf,
    /// Serializes append/rewrite so concurrent turns can't interleave lines
    /// (Windows has no O_APPEND atomicity guarantee across processes anyway).
    write_lock: Mutex<()>,
}

impl RolloutStore {
    pub fn new(dir: PathBuf) -> Self {
        if let Err(e) = fs::create_dir_all(&dir) {
            tracing::warn!("rollout: cannot create dir {}: {e}", dir.display());
        }
        RolloutStore {
            dir,
            write_lock: Mutex::new(()),
        }
    }

    fn session_dir(&self, session_id: &str) -> PathBuf {
        self.dir.join(session_id)
    }

    fn path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("rollout.jsonl")
    }

    pub fn exists(&self, session_id: &str) -> bool {
        self.path(session_id).exists()
    }

    pub fn append(&self, session_id: &str, item: &RolloutItem) -> anyhow::Result<()> {
        self.append_many(session_id, std::slice::from_ref(item))
    }

    pub fn append_many(&self, session_id: &str, items: &[RolloutItem]) -> anyhow::Result<()> {
        let result = self.append_many_inner(session_id, items);
        if let Err(e) = &result {
            tracing::warn!("rollout: append for {session_id} failed: {e}");
        }
        result
    }

    fn append_many_inner(&self, session_id: &str, items: &[RolloutItem]) -> anyhow::Result<()> {
        let mut buf = String::new();
        for item in items {
            buf.push_str(&serde_json::to_string(item)?);
            buf.push('\n');
        }
        let _guard = self.write_lock.lock().unwrap();
        fs::create_dir_all(self.session_dir(session_id))?;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path(session_id))?;
        f.write_all(buf.as_bytes())?;
        Ok(())
    }

    /// Parse every recoverable item; corrupt trailing lines (crash mid-write)
    /// are skipped with a warning instead of poisoning the whole session.
    pub fn read(&self, session_id: &str) -> Vec<RolloutItem> {
        let mut out = Vec::new();
        let Ok(file) = File::open(self.path(session_id)) else {
            return out;
        };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let line = line.trim_end();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<RolloutItem>(line) {
                Ok(item) => out.push(item),
                Err(e) => tracing::warn!("rollout: skipping bad line in {session_id}: {e}"),
            }
        }
        out
    }

    /// Session fork = file copy.
    pub fn fork(&self, from: &str, to: &str) -> anyhow::Result<()> {
        let src = self.path(from);
        if !src.exists() {
            return Ok(());
        }
        let _guard = self.write_lock.lock().unwrap();
        fs::create_dir_all(self.session_dir(to))?;
        fs::copy(&src, self.path(to))?;
        Ok(())
    }

    pub fn delete(&self, session_id: &str) {
        let _guard = self.write_lock.lock().unwrap();
        if let Err(e) = fs::remove_dir_all(self.session_dir(session_id)) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("rollout: delete for {session_id} failed: {e}");
            }
        }
    }

    /// Keep only items strictly before the `TurnBoundary` with `turn_id`
    /// (revert / checkpoint rewind). No-op if the boundary isn't found.
    pub fn truncate_before_turn(&self, session_id: &str, turn_id: &str) -> anyhow::Result<()> {
        let items = self.read(session_id);
        let Some(cut) = items.iter().position(
            |i| matches!(i, RolloutItem::TurnBoundary { turn_id: t, .. } if t == turn_id),
        ) else {
            return Ok(());
        };
        self.rewrite(session_id, &items[..cut])
    }

    /// Atomically replace the whole log (write temp, rename over).
    pub fn rewrite(&self, session_id: &str, items: &[RolloutItem]) -> anyhow::Result<()> {
        let _guard = self.write_lock.lock().unwrap();
        fs::create_dir_all(self.session_dir(session_id))?;
        let path = self.path(session_id);
        let tmp = self.session_dir(session_id).join("rollout.jsonl.tmp");
        {
            let mut f = File::create(&tmp)?;
            for item in items {
                f.write_all(serde_json::to_string(item)?.as_bytes())?;
                f.write_all(b"\n")?;
            }
            f.flush()?;
        }
        // Windows: rename over an existing file requires removing it first.
        let _ = fs::remove_file(&path);
        if let Err(e) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_protocol::models::ChatMessage;
    use agent_protocol::rollout::rebuild_messages;

    fn tmp_rollout(name: &str) -> RolloutStore {
        let dir = std::env::temp_dir()
            .join("agentd-rollout-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        RolloutStore::new(dir)
    }

    fn turn(id: &str) -> RolloutItem {
        RolloutItem::TurnBoundary {
            turn_id: id.to_string(),
            ts: String::new(),
        }
    }

    #[test]
    fn append_read_roundtrip_and_rebuild() {
        let r = tmp_rollout("roundtrip");
        r.append_many(
            "s1",
            &[
                turn("t1"),
                RolloutItem::message(ChatMessage::user("hi")),
                RolloutItem::message(ChatMessage::assistant("hello")),
            ],
        )
        .unwrap();
        let items = r.read("s1");
        assert_eq!(items.len(), 3);
        let msgs = rebuild_messages(&items);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].content, "hello");
    }

    #[test]
    fn fork_and_truncate() {
        let r = tmp_rollout("fork-trunc");
        r.append_many(
            "a",
            &[
                turn("t1"),
                RolloutItem::message(ChatMessage::user("q1")),
                RolloutItem::message(ChatMessage::assistant("a1")),
                turn("t2"),
                RolloutItem::message(ChatMessage::user("q2")),
                RolloutItem::message(ChatMessage::assistant("a2")),
            ],
        )
        .unwrap();

        r.fork("a", "b").unwrap();
        assert_eq!(r.read("b").len(), 6);

        r.truncate_before_turn("a", "t2").unwrap();
        let msgs = rebuild_messages(&r.read("a"));
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1].content, "a1");
        // Fork is unaffected.
        assert_eq!(r.read("b").len(), 6);
    }

    #[test]
    fn compaction_replaces_prefix() {
        let r = tmp_rollout("compaction");
        r.append_many(
            "s",
            &[
                turn("t1"),
                RolloutItem::message(ChatMessage::user("old question")),
                RolloutItem::Compaction {
                    summary: "user asked about X".to_string(),
                    ts: String::new(),
                },
                turn("t2"),
                RolloutItem::message(ChatMessage::user("new question")),
            ],
        )
        .unwrap();
        let msgs = rebuild_messages(&r.read("s"));
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "system");
        assert!(msgs[0].content.contains("user asked about X"));
        assert_eq!(msgs[1].content, "new question");
    }

    #[test]
    fn corrupt_trailing_line_is_skipped() {
        let r = tmp_rollout("corrupt");
        r.append("s", &RolloutItem::message(ChatMessage::user("ok")))
            .unwrap();
        // Simulate a crash mid-append.
        let path = r.path("s");
        let mut f = OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(b"{\"type\":\"message\",\"mess").unwrap();
        drop(f);
        let items = r.read("s");
        assert_eq!(items.len(), 1);
    }
}
