use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::hunspell;
use super::linux::{self, LinuxBackend};
use super::{default_languages, resolve_languages};

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("rune-spell-test-{}-{}-{n}", std::process::id(), label));
        std::fs::create_dir_all(&dir).expect("create temp test dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, name: &str, bytes: &[u8]) {
        std::fs::write(self.0.join(name), bytes).expect("write fixture file");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

const EN_AFF: &[u8] = b"SET UTF-8\nTRY esianrtolcdugmphbyfvkwz'\nSFX S Y 1\nSFX S 0 s .\n";
const EN_DIC: &[u8] = b"5\nhello\nworld\ncat/S\ndon't\nspelling\n";

const DE_AFF: &[u8] = b"SET UTF-8\nTRY enisratdhulcgmobwfkz\n";
const DE_DIC: &[u8] = b"3\nhallo\nwelt\nkatze\n";

const AR_AFF: &[u8] = "SET UTF-8\nTRY ابلتثجحخدذرزسشصضطظعغفقكلمنهوي\n".as_bytes();
const AR_DIC: &[u8] = "2\nمرحبا\nعالم\n".as_bytes();

#[test]
fn normalize_tag_handles_bcp47_and_rejects_invalid() {
    assert_eq!(linux::normalize_tag("en_US").as_deref(), Some("en-US"));
    assert_eq!(linux::normalize_tag("en_us").as_deref(), Some("en-US"));
    assert_eq!(linux::normalize_tag("DE_at").as_deref(), Some("de-AT"));
    assert_eq!(linux::normalize_tag("zh_hans").as_deref(), Some("zh-Hans"));
    assert_eq!(linux::normalize_tag("zh_hans_cn").as_deref(), Some("zh-Hans-CN"));
    assert_eq!(linux::normalize_tag("es_419").as_deref(), Some("es-419"));
    assert_eq!(linux::normalize_tag("fr").as_deref(), Some("fr"));
    assert_eq!(linux::normalize_tag("de_DE_frami").as_deref(), Some("de-DE-frami"));

    assert_eq!(linux::normalize_tag(""), None);
    assert_eq!(linux::normalize_tag("   "), None);
    assert_eq!(linux::normalize_tag("a"), None);
    assert_eq!(linux::normalize_tag("123_US"), None);
    assert_eq!(linux::normalize_tag("en__US"), None);
    assert_eq!(linux::normalize_tag("-en"), None);
    assert_eq!(linux::normalize_tag("en.US"), None);
}

#[test]
fn preferred_from_follows_posix_precedence_and_skips_c_posix() {
    let vars = |k: &str| -> Option<String> {
        match k {
            "LANGUAGE" => Some("de_AT:C:fr_FR.UTF-8@euro:POSIX:de-AT".into()),
            "LC_ALL" => Some("es_MX.UTF-8".into()),
            "LC_MESSAGES" => Some("fr_FR.UTF-8".into()),
            "LANG" => Some("en_US.UTF-8".into()),
            _ => None,
        }
    };
    assert_eq!(
        linux::preferred_from(vars),
        vec!["de-AT", "fr-FR", "es-MX", "en-US"]
    );

    let only_c = |k: &str| -> Option<String> {
        match k {
            "LC_ALL" => Some("C.UTF-8".into()),
            "LANG" => Some("POSIX".into()),
            _ => None,
        }
    };
    assert!(linux::preferred_from(only_c).is_empty());

    let empty = |_: &str| -> Option<String> { None };
    assert!(linux::preferred_from(empty).is_empty());
}

#[test]
fn search_dirs_from_follows_dicpath_xdg_and_system_order() {
    let dicpath = std::env::join_paths([Path::new("/custom/one"), Path::new("/custom/two")]).unwrap();
    let xdg_dirs = std::env::join_paths([Path::new("/opt/share"), Path::new("relative/ignored")]).unwrap();
    let dirs = linux::search_dirs_from(|k| match k {
        "DICPATH" => Some(dicpath.clone()),
        "XDG_DATA_HOME" => Some(OsString::from("/xdg/data")),
        "HOME" => Some(OsString::from("/home/user")),
        "XDG_DATA_DIRS" => Some(xdg_dirs.clone()),
        _ => None,
    });
    assert_eq!(dirs[0], PathBuf::from("/custom/one"));
    assert_eq!(dirs[1], PathBuf::from("/custom/two"));
    assert_eq!(dirs[2], PathBuf::from("/xdg/data/hunspell"));
    assert_eq!(dirs[3], PathBuf::from("/opt/share/hunspell"));
    assert!(!dirs.iter().any(|p| p == Path::new("relative/ignored/hunspell")));
    assert!(dirs.contains(&PathBuf::from("/usr/share/hunspell")));
    assert!(dirs.contains(&PathBuf::from("/usr/share/myspell/dicts")));
    assert!(dirs.contains(&PathBuf::from("/usr/share/myspell")));
    assert!(dirs.contains(&PathBuf::from("/usr/local/share/hunspell")));

    // Relative XDG_DATA_HOME falls back to $HOME/.local/share/hunspell per XDG spec.
    let fallback = linux::search_dirs_from(|k| match k {
        "XDG_DATA_HOME" => Some(OsString::from("relative/data")),
        "HOME" => Some(OsString::from("/home/user")),
        _ => None,
    });
    assert_eq!(fallback[0], PathBuf::from("/home/user").join(".local").join("share").join("hunspell"));
}

#[test]
fn hunspell_decoding_handles_utf8_bom_iso8859_1_and_iso8859_15() {
    // UTF-8 with BOM on both files.
    let mut bom_aff = vec![0xEF, 0xBB, 0xBF];
    bom_aff.extend_from_slice(EN_AFF);
    let mut bom_dic = vec![0xEF, 0xBB, 0xBF];
    bom_dic.extend_from_slice(EN_DIC);
    let dict = hunspell::parse(&bom_aff, &bom_dic).expect("parse UTF-8 BOM pair");
    assert!(dict.check("hello"));
    assert!(dict.check("cats"));

    // ISO-8859-1: "café" where é = 0xE9.
    let latin1_aff = b"SET ISO8859-1\n";
    let latin1_dic = b"1\ncaf\xE9\n";
    let dict = hunspell::parse(latin1_aff, latin1_dic).expect("parse ISO-8859-1 pair");
    assert!(dict.check("caf\u{00E9}"));
    assert!(!dict.check("cafez"));

    // ISO-8859-15: "cœur" where œ = 0xBD.
    let latin9_aff = b"SET ISO8859-15\n";
    let latin9_dic = b"1\nc\xBDur\n";
    let dict = hunspell::parse(latin9_aff, latin9_dic).expect("parse ISO-8859-15 pair");
    assert!(dict.check("c\u{0153}ur"));

    // Unsupported encoding with non-UTF-8 bytes returns None cleanly.
    let bad_aff = b"SET KOI8-R\n";
    let bad_dic = b"1\n\xFF\xFE\n";
    assert!(hunspell::parse(bad_aff, bad_dic).is_none());
}

#[test]
fn hunspell_word_filtering_skips_code_urls_acronyms_and_checks_rtl() {
    let en = hunspell::parse(EN_AFF, EN_DIC).expect("en dict");
    let ar = hunspell::parse(AR_AFF, AR_DIC).expect("ar dict");

    // Empty and whitespace.
    assert!(hunspell::misspelled(&[&en], "").is_empty());
    assert!(hunspell::misspelled(&[&en], "   \n\t ").is_empty());

    // Prose vs code/URLs/acronyms/digits/single letters.
    let text = "hello helo https://example.com foo/bar std::vec a_b HTTP camelCase v2 a don't";
    let bad = hunspell::misspelled(&[&en], text);
    let words: Vec<&str> = bad.into_iter().map(|(a, b)| &text[a..b]).collect();
    assert_eq!(words, vec!["helo"]);

    // RTL Arabic prose checks valid vs misspelled words with exact byte ranges.
    let rtl = "مرحبا خطااا عالم";
    let bad_rtl = hunspell::misspelled(&[&ar], rtl);
    let rtl_words: Vec<&str> = bad_rtl.into_iter().map(|(a, b)| &rtl[a..b]).collect();
    assert_eq!(rtl_words, vec!["خطااا"]);
}

#[test]
fn discover_in_pairs_dicts_overrides_by_priority_and_ignores_orphans() {
    let hi = TempDir::new("hi");
    let lo = TempDir::new("lo");

    // Higher-priority dir has en_US (custom) and an orphan hyphenation .dic.
    hi.write("en_US.aff", EN_AFF);
    hi.write("en_US.dic", b"1\noverrideword\n");
    hi.write("hyph_en_US.dic", b"1\nhyphen\n");
    hi.write("orphan.aff", EN_AFF);
    hi.write("README_en_US.txt", b"readme");

    // Lower-priority dir has duplicate en-US (must lose to hi) and de_DE.
    lo.write("en-US.aff", EN_AFF);
    lo.write("en-US.dic", EN_DIC);
    lo.write("de_DE.aff", DE_AFF);
    lo.write("de_DE.dic", DE_DIC);

    let missing = hi.path().join("does-not-exist");
    let catalog = linux::discover_in(&[missing, hi.path().to_path_buf(), lo.path().to_path_buf()]);
    let tags: Vec<&str> = catalog.iter().map(|e| e.tag.as_str()).collect();
    assert_eq!(tags, vec!["en-US", "de-DE"]);
    assert_eq!(catalog[0].dic, hi.path().join("en_US.dic"), "earlier dir wins on duplicate tag");
}

#[test]
fn linux_backend_lifecycle_check_suggest_learn_multilingual_and_reload() {
    let dir = TempDir::new("lifecycle");
    dir.write("en_US.aff", EN_AFF);
    dir.write("en_US.dic", EN_DIC);
    dir.write("de_DE.aff", DE_AFF);
    dir.write("de_DE.dic", DE_DIC);

    let backend = LinuxBackend::with_dirs(vec![dir.path().to_path_buf()], vec!["de-AT".into()]);
    let available = backend.available();
    assert_eq!(available, vec!["de-DE", "en-US"]);

    // Default languages for de-AT resolve to de-DE plus en-US fallback.
    let defaults = default_languages(&available, &backend.preferred());
    assert_eq!(defaults, vec!["de-DE", "en-US"]);

    // Multilingual check: valid in either language passes, invalid in both is flagged.
    let text = "hello hallo cats helo";
    let bad = backend.misspelled(1, &defaults, text, || 1);
    let bad_words: Vec<&str> = bad.into_iter().map(|(a, b)| &text[a..b]).collect();
    assert_eq!(bad_words, vec!["helo"]);
    assert_eq!(backend.loaded_tags(), vec!["de-DE", "en-US"]);

    // Suggestions include close corrections from loaded dictionaries.
    let suggestions = backend.suggest(1, &defaults, "helo", || 1);
    assert!(suggestions.iter().any(|s| s == "hello"), "expected 'hello' in suggestions: {suggestions:?}");

    // Learn adds the word in memory immediately for active dictionaries.
    backend.learn("helo");
    assert!(backend.misspelled(1, &defaults, "hello helo", || 1).is_empty());

    // Switching languages (generation bump) drops deselected dicts and re-applies learned words.
    let only_de = resolve_languages(&available, &["de-DE".into()], &backend.preferred());
    assert_eq!(only_de, vec!["de-DE"]);
    let bad_de = backend.misspelled(2, &only_de, "hello hallo helo", || 2);
    let bad_de_words: Vec<&str> = bad_de.into_iter().map(|(a, b)| &"hello hallo helo"[a..b]).collect();
    assert_eq!(bad_de_words, vec!["hello"], "en-US unloaded; learned 'helo' retained");
    assert_eq!(backend.loaded_tags(), vec!["de-DE"]);

    // Adding en-US back reuses de-DE and reloads en-US with learned words applied.
    let both = resolve_languages(&available, &["de-DE".into(), "en-US".into()], &backend.preferred());
    assert!(backend.misspelled(3, &both, "hello hallo helo", || 3).is_empty());
    assert_eq!(backend.loaded_tags(), vec!["de-DE", "en-US"]);
}

#[test]
fn linux_backend_handles_missing_and_corrupt_dictionaries_gracefully() {
    let empty_dir = TempDir::new("empty");
    let empty = LinuxBackend::with_dirs(vec![empty_dir.path().to_path_buf()], vec!["en-US".into()]);
    assert!(empty.available().is_empty());
    assert!(empty.misspelled(0, &[], "helo wrld", || 0).is_empty());
    assert!(empty.suggest(0, &[], "helo", || 0).is_empty());
    empty.learn("customword");

    let corrupt_dir = TempDir::new("corrupt");
    corrupt_dir.write("en_US.aff", EN_AFF);
    corrupt_dir.write("en_US.dic", b"not-a-count\nhello\n");
    corrupt_dir.write("de_DE.aff", DE_AFF);
    corrupt_dir.write("de_DE.dic", DE_DIC);

    let mixed = LinuxBackend::with_dirs(vec![corrupt_dir.path().to_path_buf()], vec!["en-US".into(), "de-DE".into()]);
    let active = vec!["en-US".to_owned(), "de-DE".to_owned()];
    // Corrupt en-US is skipped without panicking; valid de-DE still checks words.
    let bad = mixed.misspelled(1, &active, "hallo helo", || 1);
    let bad_words: Vec<&str> = bad.into_iter().map(|(a, b)| &"hallo helo"[a..b]).collect();
    assert_eq!(bad_words, vec!["helo"]);
    assert_eq!(mixed.loaded_tags(), vec!["de-DE"]);
}

#[cfg(all(target_os = "linux", feature = "hunspell"))]
#[test]
fn linux_platform_dispatches_off_thread() {
    assert!(super::platform::OFF_THREAD);
    // Smoke check against the process environment without panicking when no dicts are installed.
    let _ = super::available_languages();
    let _ = super::languages();
    let _ = super::misspelled("hello world", None);
}

#[cfg(all(target_os = "linux", not(feature = "hunspell")))]
#[test]
fn linux_no_default_features_falls_back_to_noop() {
    assert!(!super::platform::OFF_THREAD);
    assert!(super::available_languages().is_empty());
    assert!(super::languages().is_empty());
    assert!(super::misspelled("helo wrld", None).is_empty());
    assert!(super::suggest("helo").is_empty());
    super::learn("helo");
}

#[test]
fn escape_dic_word_handles_plain_and_slash_words() {
    assert_eq!(hunspell::escape_dic_word("hello").as_ref(), "hello");
    assert_eq!(hunspell::escape_dic_word("a/b").as_ref(), r"a\/b");
    assert_eq!(hunspell::escape_dic_word(r"a\b").as_ref(), r"a\\b");
}

#[cfg(unix)]
#[test]
fn discover_in_follows_symlinks_like_distro_hunspell_dirs() {
    use std::os::unix::fs::symlink;
    let dir = TempDir::new("symlinks");
    dir.write("en_US.aff", EN_AFF);
    dir.write("en_US.dic", EN_DIC);
    symlink(dir.path().join("en_US.aff"), dir.path().join("en_CA.aff")).expect("symlink aff");
    symlink(dir.path().join("en_US.dic"), dir.path().join("en_CA.dic")).expect("symlink dic");

    let catalog = linux::discover_in(&[dir.path().to_path_buf()]);
    let tags: Vec<&str> = catalog.iter().map(|e| e.tag.as_str()).collect();
    assert_eq!(tags, vec!["en-CA", "en-US"]);

    let backend = LinuxBackend::with_dirs(vec![dir.path().to_path_buf()], vec!["en-CA".into()]);
    let active = vec!["en-CA".to_owned()];
    assert!(backend.misspelled(1, &active, "hello world", || 1).is_empty());
    assert_eq!(backend.misspelled(1, &active, "helo", || 1), vec![(0, 4)]);
}

#[test]
fn manifest_gates_spellbook_behind_hunspell_feature() {
    let spell_manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("read crates/rune-spell/Cargo.toml");
    assert!(
        spell_manifest.contains("default = [\"hunspell\"]"),
        "rune-spell must enable hunspell by default"
    );
    assert!(
        spell_manifest.contains("\"dep:spellbook\"") && spell_manifest.contains("\"dep:unicode-segmentation\""),
        "rune-spell hunspell feature must gate optional spellbook and unicode-segmentation"
    );
    assert!(
        spell_manifest.contains("spellbook = { version = \"0.4\", optional = true }"),
        "rune-spell spellbook target dependency must be optional"
    );

    let rune_manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../rune/Cargo.toml"))
        .expect("read crates/rune/Cargo.toml");
    assert!(
        rune_manifest.contains("hunspell = [\"rune-spell/hunspell\"]"),
        "rune must forward hunspell feature to rune-spell/hunspell"
    );
    assert!(
        rune_manifest.contains("rune-spell = { workspace = true, default-features = false }"),
        "rune must disable rune-spell default features so --no-default-features omits spellbook on Linux and wasm32"
    );
    assert!(
        !rune_manifest.contains("rune-spell = { workspace = true, features = [\"hunspell\"] }"),
        "rune must not force rune-spell/hunspell unconditionally on wasm32 when default-features = false"
    );
}
