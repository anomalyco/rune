use super::{normalize_tags, parse_wide_multi_sz, posix_languages};

fn encode_multi_sz(entries: &[&str]) -> Vec<u16> {
    let mut buf = Vec::new();
    for entry in entries {
        buf.extend(entry.encode_utf16());
        buf.push(0);
    }
    buf.push(0);
    buf
}

#[test]
fn posix_languages_follows_priority_normalizes_and_deduplicates() {
    let vars = |name: &str| match name {
        "LANGUAGE" => Some("zh_CN:de_AT.UTF-8@euro:C::zh_CN".to_owned()),
        "LC_ALL" => Some("POSIX".to_owned()),
        "LC_MESSAGES" => Some("en_US.UTF-8".to_owned()),
        "LANG" => Some("de_AT.UTF-8".to_owned()),
        _ => None,
    };
    assert_eq!(posix_languages(vars), vec!["zh-CN", "de-AT", "en-US"]);
}

#[test]
fn posix_languages_returns_empty_for_c_posix_and_missing_vars() {
    assert!(posix_languages(|_| None).is_empty());
    assert!(
        posix_languages(|name| match name {
            "LANGUAGE" => Some("C:POSIX:".to_owned()),
            "LANG" => Some("C.UTF-8".to_owned()),
            _ => None,
        })
        .is_empty()
    );
}

#[test]
fn parse_wide_multi_sz_reads_double_nul_buffer_and_stops_at_sentinel() {
    let mut buf = encode_multi_sz(&["zh-Hans-CN", "  en-US  ", "zh-Hans-CN", "it-IT"]);
    // Append trailing junk after the double-NUL terminator; it must be ignored.
    buf.extend("fr-FR".encode_utf16());
    buf.push(0);
    buf.push(0);
    assert_eq!(parse_wide_multi_sz(&buf), vec!["zh-Hans-CN", "en-US", "it-IT"]);
}

#[test]
fn parse_wide_multi_sz_handles_empty_leading_nul_and_unterminated_buffers() {
    assert!(parse_wide_multi_sz(&[]).is_empty());
    assert!(parse_wide_multi_sz(&[0]).is_empty());
    assert!(parse_wide_multi_sz(&[0, 0]).is_empty());
    assert!(parse_wide_multi_sz(&[0, b'e' as u16, b'n' as u16, 0, 0]).is_empty());

    // Whitespace-only entries before the double-NUL are skipped without halting earlier valid entries.
    let spaced = encode_multi_sz(&["  ", "ja-JP", "\t", "en-GB"]);
    assert_eq!(parse_wide_multi_sz(&spaced), vec!["ja-JP", "en-GB"]);

    // Missing trailing NUL still decodes the single entry cleanly.
    let unterminated: Vec<u16> = "de-DE".encode_utf16().collect();
    assert_eq!(parse_wide_multi_sz(&unterminated), vec!["de-DE"]);
}

#[test]
fn normalize_tags_trims_and_deduplicates_preserving_order() {
    assert_eq!(
        normalize_tags(["  zh-CN ", "", "en-US", "zh-CN", "   ", "it-IT"]),
        vec!["zh-CN", "en-US", "it-IT"],
    );
}

#[cfg(target_os = "windows")]
#[test]
fn windows_preferred_languages_queries_profile_and_ui_apis() {
    let ui = super::windows_ui_languages();
    assert!(!ui.is_empty(), "GetUserPreferredUILanguages must return at least one language on Windows");
    for tag in &ui {
        assert!(!tag.is_empty());
        assert_eq!(tag.trim(), tag);
    }

    let profile = super::windows_globalization_languages();
    assert!(
        profile.as_ref().is_some_and(|langs| !langs.is_empty()),
        "GlobalizationPreferences.Languages must return Windows preferred languages",
    );
    let profile = profile.unwrap();
    for tag in &profile {
        assert!(!tag.is_empty());
        assert_eq!(tag.trim(), tag);
    }

    // Worker thread without explicit COM/WinRT initialization must also succeed.
    let worker = std::thread::spawn(super::windows_globalization_languages).join().unwrap();
    assert_eq!(worker, Some(profile.clone()));

    // system_languages() exposes the Windows preferred-languages list.
    assert_eq!(super::system_languages(), profile);
}
