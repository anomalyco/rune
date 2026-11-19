//! Linux and the BSDs: system font files found through fontconfig, in process (fontique's
//! system collection over the dlopened `libfontconfig`, which reads fontconfig's own cache), as
//! DirectWrite finds them on Windows (`direct_write`): no font folder is walked and no
//! `fc-match` or `fc-list` is started. A character the app's fonts lack goes to fontconfig's
//! match for its script and language, the font a GTK or Qt app falls back to.

use std::path::PathBuf;

use parley::fontique::{Collection, CollectionOptions, FallbackKey, FamilyId, GenericFamily, Language, Script, SourceKind};

thread_local! {
    static SYSTEM: std::cell::RefCell<Option<Collection>> = const { std::cell::RefCell::new(None) };
}

fn with_system<R>(f: impl FnOnce(&mut Collection) -> R) -> R {
    SYSTEM.with_borrow_mut(|system| f(system.get_or_insert_with(|| Collection::new(CollectionOptions { shared: false, system_fonts: true }))))
}

/// Every file of family `id` in the system collection.
fn files_of(system: &mut Collection, id: FamilyId, out: &mut Vec<PathBuf>) {
    let Some(family) = system.family(id) else { return };
    for font in family.fonts() {
        if let SourceKind::Path(path) = &font.source().kind
            && !out.iter().any(|f| f.as_path() == path.as_ref())
        {
            out.push(path.to_path_buf());
        }
    }
}

/// The files of the installed families `names` (any of their names, case aside), in order.
pub(super) fn family_files(names: &[&str]) -> Vec<PathBuf> {
    with_system(|system| {
        let mut out = Vec::new();
        for name in names {
            if let Some(id) = system.family_id(name) { files_of(system, id, &mut out); }
        }
        out
    })
}

/// The family fontconfig resolves `monospace` to, and its files.
pub(super) fn monospace() -> Option<(String, Vec<PathBuf>)> {
    with_system(|system| {
        let id = system.generic_families(GenericFamily::Monospace).next()?;
        let name = system.family_name(id)?.to_owned();
        let mut files = Vec::new();
        files_of(system, id, &mut files);
        Some((name, files))
    })
}

/// The files of the fonts fontconfig matches for the scripts of `chars` in `locale`, each
/// family whole (its bold and light faces too). A character in no script of its own (symbols,
/// punctuation) or that no installed font covers adds nothing.
pub(super) fn fallback_family_files(chars: &[char], locale: Option<&str>) -> Vec<PathBuf> {
    use icu_properties::{CodePointMapData, PropertyNamesShort, props::Script as UScript};
    let language = locale.and_then(|tag| Language::parse(tag).ok());
    let names = PropertyNamesShort::<UScript>::new();
    let mut scripts: Vec<[u8; 4]> = Vec::new();
    for &ch in chars {
        let script = CodePointMapData::<UScript>::new().get(ch);
        if matches!(script, UScript::Common | UScript::Inherited | UScript::Unknown) { continue; }
        let Some(tag) = names.get_locale_script(script) else { continue };
        let Ok(bytes) = <[u8; 4]>::try_from(tag.as_str().as_bytes()) else { continue };
        if !scripts.contains(&bytes) { scripts.push(bytes); }
    }
    with_system(|system| {
        let mut out = Vec::new();
        for script in scripts {
            let ids: Vec<FamilyId> = system.fallback_families(FallbackKey::new(Script::from_bytes(script), language.as_ref())).take(2).collect();
            for id in ids { files_of(system, id, &mut out); }
        }
        out
    })
}
