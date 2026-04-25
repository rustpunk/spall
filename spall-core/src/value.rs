use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Closed JSON-shaped value. Replaces `serde_json::Value` in the IR so that
/// postcard (non-self-describing) can serialize/deserialize it without hitting
/// `deserialize_any`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SpallValue {
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    Str(String),
    Array(Vec<SpallValue>),
    #[serde(with = "indexmap::map::serde_seq")]
    Object(IndexMap<String, SpallValue>),
}

impl From<&serde_json::Value> for SpallValue {
    fn from(v: &serde_json::Value) -> Self {
        match v {
            serde_json::Value::Null => SpallValue::Null,
            serde_json::Value::Bool(b) => SpallValue::Bool(*b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    SpallValue::I64(i)
                } else if let Some(u) = n.as_u64() {
                    SpallValue::U64(u)
                } else if let Some(f) = n.as_f64() {
                    SpallValue::F64(f)
                } else {
                    SpallValue::Null
                }
            }
            serde_json::Value::String(s) => SpallValue::Str(s.clone()),
            serde_json::Value::Array(a) => {
                SpallValue::Array(a.iter().map(SpallValue::from).collect())
            }
            serde_json::Value::Object(o) => SpallValue::Object(
                o.iter()
                    .map(|(k, v)| (k.clone(), SpallValue::from(v)))
                    .collect(),
            ),
        }
    }
}

impl From<&SpallValue> for serde_json::Value {
    fn from(v: &SpallValue) -> Self {
        match v {
            SpallValue::Null => serde_json::Value::Null,
            SpallValue::Bool(b) => serde_json::Value::Bool(*b),
            SpallValue::I64(i) => serde_json::Value::from(*i),
            SpallValue::U64(u) => serde_json::Value::from(*u),
            SpallValue::F64(f) => serde_json::Value::from(*f),
            SpallValue::Str(s) => serde_json::Value::String(s.clone()),
            SpallValue::Array(a) => {
                serde_json::Value::Array(a.iter().map(serde_json::Value::from).collect())
            }
            SpallValue::Object(o) => {
                let mut m = serde_json::Map::with_capacity(o.len());
                for (k, v) in o {
                    m.insert(k.clone(), serde_json::Value::from(v));
                }
                serde_json::Value::Object(m)
            }
        }
    }
}

impl fmt::Display for SpallValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = serde_json::Value::from(self);
        write!(f, "{}", v)
    }
}

impl SpallValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            SpallValue::Str(s) => Some(s),
            _ => None,
        }
    }
}
