//! The worker's event log: one JSON object per line on the console.
//! Fields carry diagnostics (object kinds, sizes, event names) and never
//! credentials, key material, or request bodies. Serialization goes
//! through serde_json because some values derive from request bytes, and
//! hand-built log lines would be an injection surface.

use worker::console_log;

/// Emit one event line at the given level.
pub fn event(level: &str, name: &str, fields: &[(&str, String)]) {
    let mut document = serde_json::Map::new();
    document.insert("level".to_string(), level.into());
    document.insert("event".to_string(), name.into());
    for (key, value) in fields {
        document.insert((*key).to_string(), value.clone().into());
    }
    console_log!("{}", serde_json::Value::Object(document));
}

/// Emit one alert line: a condition the platform must never produce under
/// correct operation (corrupt internal state, violated expectations).
pub fn alert(name: &str) {
    event("alert", name, &[]);
}
