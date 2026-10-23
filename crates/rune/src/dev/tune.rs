use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use crate::{Color, Spring};

thread_local! {
    static ACTIVE: RefCell<Option<Registry>> = const { RefCell::new(None) };
}

pub(crate) type Registry = Arc<Mutex<BTreeMap<String, Entry>>>;

pub(crate) fn scope<R>(registry: Registry, f: impl FnOnce() -> R) -> R {
    struct Guard(Option<Registry>);
    impl Drop for Guard {
        fn drop(&mut self) { ACTIVE.with(|a| { a.replace(self.0.take()); }); }
    }
    let _guard = Guard(ACTIVE.with(|a| a.replace(Some(registry))));
    f()
}

/// Values supported by the knobs remote protocol.
pub trait Tunable: Copy {
    const KIND: &'static str;
    fn encode(self) -> Value;
    fn decode(value: &Value) -> Option<Self>;
}

impl Tunable for f32 {
    const KIND: &'static str = "number";
    fn encode(self) -> Value { json!(self) }
    fn decode(value: &Value) -> Option<Self> {
        let n = value.as_f64()? as f32;
        n.is_finite().then_some(n)
    }
}

impl Tunable for Spring {
    const KIND: &'static str = "spring";
    fn encode(self) -> Value { json!({"duration":self.duration,"bounce":self.bounce}) }
    fn decode(value: &Value) -> Option<Self> {
        let duration = f32::decode(&value["duration"])?;
        let bounce = f32::decode(&value["bounce"])?;
        (duration > 0.0 && duration < 1e4 && bounce > -1.0 && bounce < 1.0)
            .then(|| Spring::new(duration, bounce))
    }
}

impl Tunable for Color {
    const KIND: &'static str = "color";
    fn encode(self) -> Value { json!(self.hex()) }
    fn decode(value: &Value) -> Option<Self> {
        let css: csscolorparser::Color = value.as_str()?.parse().ok()?;
        let (r, g, b, a) = (css.r, css.g, css.b, css.a);
        Some(Color {
            r: crate::color::srgb_decode(r as f32),
            g: crate::color::srgb_decode(g as f32),
            b: crate::color::srgb_decode(b as f32),
            a: a as f32,
        })
    }
}

pub(crate) struct Entry {
    pub descriptor: Value,
    pub value: Value,
    normalize: fn(&Value) -> Option<Value>,
}

impl Entry {
    pub fn validate(&self, value: &Value) -> Result<Value, String> {
        if value.is_null() { return Ok(self.descriptor["default"].clone()); }
        let value = (self.normalize)(value).ok_or("invalid tunable value")?;
        if let Some(n) = value.as_f64() {
            if self.descriptor["min"].as_f64().is_some_and(|min| n < min)
                || self.descriptor["max"].as_f64().is_some_and(|max| n > max) {
                return Err("value is outside the declared range".into());
            }
        }
        Ok(value)
    }
}

/// Used by `tune!`. The inactive path never creates a registry or allocates a descriptor.
pub fn tune<T: Tunable>(key: &str, default: T, bounds: Option<[f32; 3]>) -> T {
    ACTIVE.with(|active| {
        let active = active.borrow();
        let Some(registry) = active.as_ref() else { return default };
        assert!(!key.starts_with("rune."), "rune.* is reserved for dev controls");
        let mut entries = registry.lock().unwrap();
        let entry = entries.entry(key.to_owned()).or_insert_with(|| {
            let value = default.encode();
            assert!(T::decode(&value).is_some(), "invalid default for {key}");
            let (group, label) = key.rsplit_once('.').unwrap_or(("Tunables", key));
            let mut descriptor = json!({"key":key,"kind":T::KIND,"default":value,"group":group,"label":label});
            if T::KIND == "number" {
                let n = value.as_f64().unwrap() as f32;
                let [min, max, step] = bounds.unwrap_or([0.0_f32.min(n * 2.0), 1.0_f32.max(n * 2.0), 0.1]);
                assert!(min.is_finite() && max.is_finite() && step.is_finite() && min < max && step > 0.0 && n >= min && n <= max);
                descriptor["min"] = json!(min);
                descriptor["max"] = json!(max);
                descriptor["step"] = json!(step);
            }
            Entry { descriptor, value, normalize: |v| T::decode(v).map(T::encode) }
        });
        assert_eq!(entry.descriptor["kind"], T::KIND, "tunable {key} changed type");
        if entry.value == entry.descriptor["default"] { return default; }
        T::decode(&entry.value).unwrap_or(default)
    })
}
