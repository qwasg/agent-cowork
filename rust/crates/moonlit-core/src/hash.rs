//! Stable content hashing that is byte-for-byte compatible with the JS
//! implementation in `docforge/packages/doc-core/src/hash.ts`.
//!
//! Used as the compile/preview cache key: identical IR -> identical hash ->
//! cache hit. Parity with the JS hash is therefore a hard requirement; the
//! parity tests in `moonlit-doccore` depend on it.

use serde_json::Value;

/// Stable serialization: object keys are sorted so that semantically equal
/// values produce identical strings. Mirrors `stableStringify` in hash.ts.
///
/// Note: JS `JSON.stringify` renders integral numbers without a trailing
/// `.0` (e.g. `1`, not `1.0`). We replicate that here so the FNV input
/// matches the JS string exactly.
pub fn stable_stringify(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => {
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Value::Number(n) => format_number(n),
        Value::String(_) => {
            // serde_json escapes strings the same way as JS JSON.stringify
            // for the BMP/standard cases used by the IR.
            serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
        }
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(stable_stringify).collect();
            format!("[{}]", parts.join(","))
        }
        Value::Object(map) => {
            // JS only filters `undefined`; serde_json has no `undefined`, so we
            // keep every present key (including explicit null).
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    let key = serde_json::to_string(&Value::String(k.clone()))
                        .unwrap_or_else(|_| format!("\"{k}\""));
                    format!("{}:{}", key, stable_stringify(&map[k]))
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
    }
}

/// Render a JSON number the way `JSON.stringify` would in JS: integral floats
/// drop the fractional part.
fn format_number(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    if let Some(u) = n.as_u64() {
        return u.to_string();
    }
    if let Some(f) = n.as_f64() {
        if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 {
            return format!("{}", f as i64);
        }
        // Shortest round-trippable representation, matching V8 closely enough
        // for the coordinate magnitudes used by the IR.
        let s = format!("{f}");
        return s;
    }
    // NaN / Infinity are serialized as null by JSON.stringify.
    "null".to_string()
}

/// FNV-1a 64-bit over the UTF-16 code units of `s`, returning a zero-padded
/// 16-char hex string. Mirrors `fnv1a64` in hash.ts (which iterates over
/// `charCodeAt`, i.e. UTF-16 code units, low byte then optional high byte).
pub fn fnv1a64(s: &str) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash: u64 = FNV_OFFSET;
    for unit in s.encode_utf16() {
        let lo = (unit & 0xff) as u64;
        hash ^= lo;
        hash = hash.wrapping_mul(FNV_PRIME);
        let hi = unit >> 8;
        if hi != 0 {
            hash ^= hi as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    }
    format!("{hash:016x}")
}

/// Stable content hash over any JSON value.
pub fn content_hash(value: &Value) -> String {
    fnv1a64(&stable_stringify(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stable_stringify_sorts_keys() {
        let v = json!({ "b": 1, "a": 2 });
        assert_eq!(stable_stringify(&v), "{\"a\":2,\"b\":1}");
    }

    #[test]
    fn stable_stringify_integral_numbers_have_no_decimal() {
        let v = json!({ "x": 1.0, "w": 8.0 });
        assert_eq!(stable_stringify(&v), "{\"w\":8,\"x\":1}");
    }

    #[test]
    fn stable_stringify_fractional_numbers_kept() {
        let v = json!({ "h": 5.625 });
        assert_eq!(stable_stringify(&v), "{\"h\":5.625}");
    }

    #[test]
    fn fnv_matches_known_vectors() {
        // Empty string -> FNV offset basis (16 hex).
        assert_eq!(fnv1a64(""), "cbf29ce484222325");
        // "a": offset ^ 0x61 then * prime.
        let expected = {
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            h ^= 0x61;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
            format!("{h:016x}")
        };
        assert_eq!(fnv1a64("a"), expected);
    }

    #[test]
    fn content_hash_is_stable_across_key_order() {
        let a = json!({ "type": "word", "blocks": [] });
        let b = json!({ "blocks": [], "type": "word" });
        assert_eq!(content_hash(&a), content_hash(&b));
    }
}
