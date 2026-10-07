use serde_json::Value;

pub fn get_str(args: Option<&serde_json::Map<String, Value>>, key: &str) -> Option<String> {
    args?
        .get(key)?
        .as_str()
        .map(std::string::ToString::to_string)
}

pub fn get_bool(args: Option<&serde_json::Map<String, Value>>, key: &str) -> Option<bool> {
    args?.get(key)?.as_bool()
}

pub fn hash_fast(s: &str) -> String {
    const THRESHOLD: usize = 16 * 1024;
    if s.len() <= THRESHOLD {
        crate::core::hasher::hash_str(s)
    } else {
        let prefix = &s[..s.floor_char_boundary(4096)];
        let suffix = &s[s.ceil_char_boundary(s.len().saturating_sub(4096))..];
        let mut hasher = blake3::Hasher::new();
        hasher.update(prefix.as_bytes());
        hasher.update(s.len().to_string().as_bytes());
        hasher.update(suffix.as_bytes());
        hasher.finalize().to_hex().to_string()
    }
}

pub fn canonicalize_json(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for k in keys {
                if let Some(val) = map.get(k) {
                    out.insert(k.clone(), canonicalize_json(val));
                }
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(canonicalize_json).collect()),
        other => other.clone(),
    }
}

pub fn canonical_args_string(args: Option<&serde_json::Map<String, Value>>) -> String {
    let v = args.map_or(Value::Null, |m| Value::Object(m.clone()));
    let canon = canonicalize_json(&v);
    serde_json::to_string(&canon).unwrap_or_default()
}
