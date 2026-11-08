//! [`Data`]: plain values that cross a VM boundary: from a worker thread into a script, from a
//! host into a [signal](crate::Script::fire), and from the old VM to the new one on a hot reload.

use std::collections::BTreeMap;

use mlua::{FromLua, IntoLua, Lua, Value};

/// A plain, owned value: what a script table holds, without functions, threads or userdata.
/// It is `Send`, so a worker thread can return one.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Data {
    #[default]
    Nil,
    Bool(bool),
    /// A whole number: Luau keeps every number as a double, and a whole one comes across as
    /// this, so it decodes into an integer field.
    Integer(i64),
    Number(f64),
    String(String),
    /// A Luau array: keys `1..=n`.
    List(Vec<Data>),
    /// A Luau table with string keys.
    Map(BTreeMap<String, Data>),
}

/// How deep [`Data`] follows nested tables; a deeper or cyclic table stops there as `nil`.
const MAX_DEPTH: usize = 64;

/// The largest whole number a double holds exactly (2^53 - 1).
const MAX_SAFE: f64 = 9_007_199_254_740_991.0;

impl Data {
    /// The value as a number, if it is one.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Data::Number(n) => Some(*n),
            Data::Integer(i) => Some(*i as f64),
            _ => None,
        }
    }

    /// The value as an integer, if it is a whole number.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Data::Integer(i) => Some(*i),
            _ => None,
        }
    }

    /// The value as a string, if it is one.
    pub fn as_str(&self) -> Option<&str> {
        if let Data::String(s) = self { Some(s) } else { None }
    }

    /// The field `key` of a map, or [`Data::Nil`].
    pub fn get(&self, key: &str) -> &Data {
        const NIL: Data = Data::Nil;
        match self {
            Data::Map(m) => m.get(key).unwrap_or(&NIL),
            _ => &NIL,
        }
    }

    /// Decodes into a host record through serde. Whole numbers decode into integer fields on
    /// every target: mlua's own serde path hands wasm32 hosts a float for any whole number past
    /// 32 bits, which an `i64` field refuses.
    #[cfg(feature = "serde")]
    pub fn decode<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.to_json())
    }

    /// The value as JSON: lists as arrays, maps as objects, whole numbers as integers.
    #[cfg(feature = "serde")]
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::Value as J;
        match self {
            Data::Nil => J::Null,
            Data::Bool(b) => J::Bool(*b),
            Data::Integer(i) => J::from(*i),
            Data::Number(n) => serde_json::Number::from_f64(*n).map_or(J::Null, J::Number),
            Data::String(s) => J::String(s.clone()),
            Data::List(items) => J::Array(items.iter().map(Data::to_json).collect()),
            Data::Map(fields) => J::Object(fields.iter().map(|(k, v)| (k.clone(), v.to_json())).collect()),
        }
    }

    fn from_value(value: &Value, depth: usize) -> Data {
        match value {
            Value::Boolean(b) => Data::Bool(*b),
            // `i32` on wasm32, `i64` elsewhere.
            #[allow(clippy::useless_conversion)]
            Value::Integer(i) => Data::Integer(i64::from(*i)),
            // mlua's integers are isize: on wasm32 a whole number past 32 bits arrives as a
            // float, so whole numbers are recognised here too.
            Value::Number(n) if n.fract() == 0.0 && n.abs() <= MAX_SAFE => Data::Integer(*n as i64),
            Value::Number(n) => Data::Number(*n),
            Value::String(s) => Data::String(s.to_string_lossy()),
            Value::Table(t) if depth < MAX_DEPTH => {
                let len = t.raw_len();
                if len > 0 {
                    return Data::List((1..=len).map(|i| Data::from_value(&t.raw_get(i).unwrap_or(Value::Nil), depth + 1)).collect());
                }
                Data::Map(
                    t.pairs::<Value, Value>()
                        .flatten()
                        .filter_map(|(k, v)| {
                            let key = match &k {
                                Value::String(s) => s.to_string_lossy(),
                                Value::Integer(i) => i.to_string(),
                                Value::Number(n) => n.to_string(),
                                _ => return None,
                            };
                            Some((key, Data::from_value(&v, depth + 1)))
                        })
                        .collect(),
                )
            }
            _ => Data::Nil,
        }
    }
}

impl From<bool> for Data {
    fn from(v: bool) -> Self {
        Data::Bool(v)
    }
}

impl From<f64> for Data {
    fn from(v: f64) -> Self {
        Data::Number(v)
    }
}

impl From<i64> for Data {
    fn from(v: i64) -> Self {
        Data::Integer(v)
    }
}

impl From<&str> for Data {
    fn from(v: &str) -> Self {
        Data::String(v.to_owned())
    }
}

impl From<String> for Data {
    fn from(v: String) -> Self {
        Data::String(v)
    }
}

impl<T: Into<Data>> From<Vec<T>> for Data {
    fn from(v: Vec<T>) -> Self {
        Data::List(v.into_iter().map(Into::into).collect())
    }
}

impl IntoLua for Data {
    fn into_lua(self, lua: &Lua) -> mlua::Result<Value> {
        Ok(match self {
            Data::Nil => Value::Nil,
            Data::Bool(b) => Value::Boolean(b),
            Data::Integer(i) => Value::Number(i as f64),
            Data::Number(n) => Value::Number(n),
            Data::String(s) => Value::String(lua.create_string(s)?),
            Data::List(items) => {
                let t = lua.create_table_with_capacity(items.len(), 0)?;
                for (i, item) in items.into_iter().enumerate() {
                    t.raw_set(i + 1, item)?;
                }
                Value::Table(t)
            }
            Data::Map(fields) => {
                let t = lua.create_table_with_capacity(0, fields.len())?;
                for (k, v) in fields {
                    t.raw_set(k, v)?;
                }
                Value::Table(t)
            }
        })
    }
}

impl FromLua for Data {
    fn from_lua(value: Value, _: &Lua) -> mlua::Result<Self> {
        Ok(Data::from_value(&value, 0))
    }
}
