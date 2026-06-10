//! Id generation. Mirrors `docforge/packages/doc-core/src/ids.ts`.
//!
//! A pluggable factory lets tests use deterministic ids while production uses
//! a time/counter/random scheme that is unique enough across sessions.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// `(prefix) -> id`.
pub trait IdFactory: Send + Sync {
    fn next(&self, prefix: &str) -> String;
}

impl<F> IdFactory for F
where
    F: Fn(&str) -> String + Send + Sync,
{
    fn next(&self, prefix: &str) -> String {
        self(prefix)
    }
}

/// Default factory: `prefix_<seq base36><6 random base36>`.
#[derive(Clone, Default)]
pub struct DefaultIdFactory {
    counter: Arc<AtomicU64>,
}

impl DefaultIdFactory {
    pub fn new() -> Self {
        Self::default()
    }
}

impl IdFactory for DefaultIdFactory {
    fn next(&self, prefix: &str) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed) % 0xff_ffff + 1;
        let seq = to_base36(n);
        let rand = random_base36(6);
        format!("{prefix}_{seq}{rand}")
    }
}

/// Deterministic factory for tests: `prefix_<n>`.
#[derive(Clone, Default)]
pub struct SeqIdFactory {
    counter: Arc<AtomicU64>,
}

impl SeqIdFactory {
    pub fn new() -> Self {
        Self::default()
    }
}

impl IdFactory for SeqIdFactory {
    fn next(&self, prefix: &str) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        format!("{prefix}_{n}")
    }
}

fn to_base36(mut n: u64) -> String {
    if n == 0 {
        return "0".to_string();
    }
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

fn random_base36(len: usize) -> String {
    // Cheap xorshift seeded from time; ids only need low collision odds, not
    // cryptographic randomness (matches the JS `Math.random` usage).
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut state = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e37_79b9_7f4a_7c15)
        | 1;
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push(DIGITS[(state % 36) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seq_factory_is_deterministic() {
        let f = SeqIdFactory::new();
        assert_eq!(f.next("blk"), "blk_1");
        assert_eq!(f.next("blk"), "blk_2");
        assert_eq!(f.next("el"), "el_3");
    }

    #[test]
    fn default_factory_has_prefix_and_is_unique() {
        let f = DefaultIdFactory::new();
        let a = f.next("sld");
        let b = f.next("sld");
        assert!(a.starts_with("sld_"));
        assert_ne!(a, b);
    }
}
