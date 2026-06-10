//! Append-only JSONL event log with an in-memory `(seq -> byte offset)` index.
//!
//! Optimization over the Python `JsonlEventStore`: replaying `fromSeq` seeks
//! straight to the matching byte offset instead of parsing the whole file from
//! the start, so incremental replay is O(events_after_seq) not O(total_events).

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::contracts::events::DebugEvent;

struct SessionIndex {
    /// Ascending `(seq, byte_offset_of_line_start)`.
    offsets: Vec<(i64, u64)>,
    next_offset: u64,
}

pub struct JsonlStore {
    dir: PathBuf,
    index: Mutex<HashMap<String, SessionIndex>>,
}

impl JsonlStore {
    pub fn new(dir: PathBuf) -> Self {
        let _ = fs::create_dir_all(&dir);
        JsonlStore {
            dir,
            index: Mutex::new(HashMap::new()),
        }
    }

    fn path(&self, session_id: &str) -> PathBuf {
        self.dir.join(format!("{session_id}.jsonl"))
    }

    fn ensure_index(&self, session_id: &str) {
        let mut guard = self.index.lock().unwrap();
        if guard.contains_key(session_id) {
            return;
        }
        let mut offsets = Vec::new();
        let mut next_offset = 0u64;
        if let Ok(file) = File::open(self.path(session_id)) {
            let mut reader = BufReader::new(file);
            let mut line = String::new();
            loop {
                let start = next_offset;
                line.clear();
                let n = match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break,
                };
                next_offset += n as u64;
                if let Ok(ev) = serde_json::from_str::<DebugEvent>(line.trim_end()) {
                    offsets.push((ev.seq, start));
                }
            }
        }
        guard.insert(
            session_id.to_string(),
            SessionIndex {
                offsets,
                next_offset,
            },
        );
    }

    pub fn append(&self, session_id: &str, event: &DebugEvent) {
        self.ensure_index(session_id);
        let line = match serde_json::to_string(event) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("jsonl: failed to serialize event for {session_id}: {e}");
                return;
            }
        };
        let bytes = format!("{line}\n");
        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path(session_id))
        {
            Ok(mut f) => {
                let mut guard = self.index.lock().unwrap();
                let idx = guard.entry(session_id.to_string()).or_insert(SessionIndex {
                    offsets: Vec::new(),
                    next_offset: 0,
                });
                let start = idx.next_offset;
                match f.write_all(bytes.as_bytes()) {
                    Ok(()) => {
                        idx.offsets.push((event.seq, start));
                        idx.next_offset += bytes.len() as u64;
                    }
                    Err(e) => {
                        tracing::warn!("jsonl: write failed for {session_id}: {e}");
                    }
                }
            }
            Err(e) => {
                tracing::warn!("jsonl: cannot open log for {session_id}: {e}");
            }
        }
    }

    pub fn read_session(&self, session_id: &str) -> Vec<DebugEvent> {
        let mut out = Vec::new();
        if let Ok(file) = File::open(self.path(session_id)) {
            let reader = BufReader::new(file);
            for line in reader.lines().map_while(Result::ok) {
                if let Ok(ev) = serde_json::from_str::<DebugEvent>(line.trim_end()) {
                    out.push(ev);
                }
            }
        }
        out
    }

    /// Replay events with `seq > from_seq`, seeking to the matching byte offset.
    pub fn replay_since(&self, session_id: &str, from_seq: i64) -> Vec<DebugEvent> {
        self.ensure_index(session_id);
        let seek_offset = {
            let guard = self.index.lock().unwrap();
            let Some(idx) = guard.get(session_id) else {
                return Vec::new();
            };
            // First offset whose seq > from_seq.
            idx.offsets
                .iter()
                .find(|(seq, _)| *seq > from_seq)
                .map(|(_, off)| *off)
        };
        let Some(offset) = seek_offset else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Ok(mut file) = File::open(self.path(session_id)) {
            if file.seek(SeekFrom::Start(offset)).is_ok() {
                let mut buf = String::new();
                if file.read_to_string(&mut buf).is_ok() {
                    for line in buf.lines() {
                        if let Ok(ev) = serde_json::from_str::<DebugEvent>(line.trim_end()) {
                            if ev.seq > from_seq {
                                out.push(ev);
                            }
                        }
                    }
                }
            }
        }
        out
    }

    pub fn delete_session(&self, session_id: &str) {
        let _ = fs::remove_file(self.path(session_id));
        self.index.lock().unwrap().remove(session_id);
    }
}
