//! Build-time generation of a [`crate::Catalog`] from JSON (feature `codegen`).
//!
//! The folder holds `locales.json` and one subfolder per layer:
//!
//! ```json
//! { "layers": ["upstream", "app"],
//!   "locales": [{ "id": "en", "tag": "en", "label": "English" },
//!               { "id": "ar", "tag": "ar", "label": "العربية", "rtl": true }] }
//! ```
//!
//! `<layer>/<id>.json` is a flat object of key to message; a later layer's message replaces an
//! earlier one's, and a missing file is a locale with no messages in that layer. The first locale
//! is the source: its keys are the catalog's keys. A base whose source has `base.one` and
//! `base.other` is a plural, and every locale's `base.<category>` messages join the catalog.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::{fs, io};

use serde_json::Value;

use crate::PluralCategory;

/// Reads the catalogs in `dir` and writes `i18n.rs` to `out_dir`, with each locale's text blob
/// and span table beside it in `out_dir/i18n/`. Include the result with
/// `include!(concat!(env!("OUT_DIR"), "/i18n.rs"))`; it defines `KEYS`, `PLURALS` and `CATALOG`.
/// Prints `cargo:rerun-if-changed` for `dir`, so a new or edited catalog rebuilds.
pub fn generate(dir: &Path, out_dir: &Path) -> io::Result<()> {
    println!("cargo:rerun-if-changed={}", dir.display());
    let manifest = read_json(&dir.join("locales.json"))?;
    let layers: Vec<&str> = manifest["layers"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let locales = locales(&manifest)?;

    let messages: Vec<BTreeMap<String, String>> = locales
        .iter()
        .map(|locale| {
            let mut merged = BTreeMap::new();
            for layer in &layers {
                let path = dir.join(layer).join(format!("{}.json", locale.id));
                if !path.exists() {
                    continue;
                }
                let Value::Object(map) = read_json(&path)? else {
                    return Err(invalid(format!("{} is not a JSON object", path.display())));
                };
                merged.extend(map.into_iter().filter_map(|(key, value)| Some((key, value.as_str()?.to_owned()))));
            }
            Ok(merged)
        })
        .collect::<io::Result<_>>()?;

    let source = &messages[0];
    let mut plurals: Vec<&str> = source
        .keys()
        .filter_map(|key| key.strip_suffix(".other"))
        .filter(|base| source.contains_key(&format!("{base}.one")))
        .collect();
    plurals.sort_unstable();
    let mut keys: BTreeSet<&str> = source.keys().map(String::as_str).collect();
    for locale in &messages {
        keys.extend(locale.keys().map(String::as_str).filter(|key| {
            key.rsplit_once('.').is_some_and(|(base, category)| {
                plurals.binary_search(&base).is_ok() && PluralCategory::ALL.iter().any(|c| c.suffix() == category)
            })
        }));
    }
    let keys: Vec<&str> = keys.into_iter().collect();

    let blobs = out_dir.join("i18n");
    fs::create_dir_all(&blobs)?;
    for (locale, map) in locales.iter().zip(&messages) {
        // A right-to-left locale's missing message is the source's left-to-right one, isolated, so an
        // untranslated English sentence keeps its words and its full stop in order in an Arabic row.
        let isolate = (locale.rtl && !locales[0].rtl).then_some(source);
        let (text, spans) = blob(&keys, map, isolate);
        fs::write(blobs.join(format!("{}.txt", locale.id)), text)?;
        fs::write(blobs.join(format!("{}.spans", locale.id)), spans)?;
    }
    fs::write(out_dir.join("i18n.rs"), source_file(&keys, &plurals, &locales))
}

struct Locale {
    id: String,
    tag: String,
    label: String,
    rtl: bool,
}

fn locales(manifest: &Value) -> io::Result<Vec<Locale>> {
    let list = manifest["locales"].as_array().filter(|list| !list.is_empty());
    let list = list.ok_or_else(|| invalid("locales.json lists no locales".into()))?;
    list.iter()
        .map(|entry| {
            let field = |name: &str| entry[name].as_str().map(str::to_owned);
            let id = field("id").filter(|id| id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
            let id = id.ok_or_else(|| invalid(format!("locale entry {entry} needs an id of letters, digits, - or _")))?;
            Ok(Locale {
                tag: field("tag").unwrap_or_else(|| id.clone()),
                label: field("label").unwrap_or_else(|| id.clone()),
                rtl: entry["rtl"].as_bool().unwrap_or(false),
                id,
            })
        })
        .collect()
}

fn blob<'a>(keys: &[&str], map: &BTreeMap<String, String>, isolate: Option<&'a BTreeMap<String, String>>) -> (String, Vec<u8>) {
    let source = |key: &str| isolate.and_then(|source| source.get(key)).map(String::as_str);
    crate::catalog::blob(keys, |key| map.get(key).map(String::as_str), isolate.map(|_| &source as &dyn Fn(&str) -> Option<&'a str>))
}

macro_rules! ln {
    ($($arg:tt)*) => {
        format!("{}\n", format_args!($($arg)*))
    };
}

fn source_file(keys: &[&str], plurals: &[&str], locales: &[Locale]) -> String {
    let mut out = String::from("// Generated by rune_i18n::codegen. Do not edit.\n\n");
    out += &ln!("pub const KEYS: &[&str] = &[");
    for key in keys {
        out += &ln!("    {key:?},");
    }
    out += &ln!("];\n\npub const PLURALS: &[&str] = &[");
    for base in plurals {
        out += &ln!("    {base:?},");
    }
    out += &ln!("];\n\nstatic PLURAL_FORMS: [[u32; 6]; {}] = [", plurals.len());
    for base in plurals {
        let forms = PluralCategory::ALL.map(|category| {
            let key = format!("{base}.{}", category.suffix());
            keys.binary_search(&key.as_str()).map_or(u32::MAX, |index| index as u32)
        });
        out += &ln!("    {forms:?},");
    }
    out += &ln!("];\n\nstatic LOCALES: [::rune_i18n::LocaleInfo; {}] = [", locales.len());
    for locale in locales {
        out += &ln!("    ::rune_i18n::LocaleInfo::new({:?}, {:?}, {:?}, {}),",
            locale.id, locale.tag, locale.label, locale.rtl
        );
    }
    out += &ln!("];\n\nstatic MESSAGES: [::rune_i18n::Messages; {}] = [", locales.len());
    // A web build keeps the source locale's messages in its module and loads the one a page shows
    // at run time (`Catalog::load_json`).
    let web = std::env::var("CARGO_CFG_TARGET_ARCH").is_ok_and(|arch| arch == "wasm32");
    for (i, locale) in locales.iter().enumerate() {
        if web && i > 0 {
            out += &ln!("    ::rune_i18n::Messages::later(),");
            continue;
        }
        out += &ln!("    ::rune_i18n::Messages::new(include_str!(concat!(env!(\"OUT_DIR\"), \"/i18n/{0}.txt\")), \
             include_bytes!(concat!(env!(\"OUT_DIR\"), \"/i18n/{0}.spans\"))),",
            locale.id
        );
    }
    out += &ln!("];\n\npub static CATALOG: ::rune_i18n::Catalog = \
         ::rune_i18n::Catalog::new(KEYS, PLURALS, &PLURAL_FORMS, &LOCALES, &MESSAGES);"
    );
    out
}

fn read_json(path: &Path) -> io::Result<Value> {
    let text = fs::read_to_string(path).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
    serde_json::from_str(&text).map_err(|e| invalid(format!("{}: {e}", path.display())))
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
