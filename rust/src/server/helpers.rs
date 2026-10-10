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
