use super::*;

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
        "LANGUAGE" => Some("ar_EG:de_AT.UTF-8@euro:C::ar_EG".to_owned()),
        "LC_ALL" => Some("POSIX".to_owned()),
        "LC_MESSAGES" => Some("en_US.UTF-8".to_owned()),
        "LANG" => Some("de_AT.UTF-8".to_owned()),
        _ => None,
    };
    assert_eq!(posix_languages(vars), vec!["ar-EG", "de-AT", "en-US"]);
}

#[test]
fn posix_languages_returns_empty_for_c_posix_and_missing_vars() {
    assert!(posix_languages(|_| None).is_empty());
    assert!(posix_languages(|name| match name {
        "LANGUAGE" => Some("C:POSIX:".to_owned()),
        "LANG" => Some("C.UTF-8".to_owned()),
        _ => None,
    }).is_empty());
}

#[test]
fn normalize_trims_skips_empty_and_preserves_first_occurrence() {
    assert_eq!(
        normalize(["  ar-EG  ", "", "   ", "en-US", "ar-EG", "he-IL"]),
        vec!["ar-EG", "en-US", "he-IL"],
    );
    assert!(normalize(["", " \t "]).is_empty());
}

#[test]
fn parse_wide_multi_sz_reads_double_nul_buffer_and_stops_at_sentinel() {
    let mut buf = encode_multi_sz(&["zh-Hans-CN", "  en-US ", "zh-Hans-CN", "it-IT"]);
    // Trailing units past the double-NUL terminator must be ignored.
    buf.extend("fr-FR".encode_utf16());
    buf.push(0);
    buf.push(0);
    assert_eq!(parse_wide_multi_sz(&buf), vec!["zh-Hans-CN", "en-US", "it-IT"]);

    assert!(parse_wide_multi_sz(&[]).is_empty());
    assert!(parse_wide_multi_sz(&[0, 0]).is_empty());
    assert!(parse_wide_multi_sz(&[0, b'e' as u16, b'n' as u16, 0, 0]).is_empty());
}

#[test]
fn locale_setting_change_area_matches_intl_case_insensitively() {
    assert!(is_locale_setting_change_area(Some("intl")));
    assert!(is_locale_setting_change_area(Some("Intl")));
    assert!(is_locale_setting_change_area(Some("INTL")));
    assert!(!is_locale_setting_change_area(None));
    assert!(!is_locale_setting_change_area(Some("")));
    assert!(!is_locale_setting_change_area(Some("ImmersiveColorSet")));
    assert!(!is_locale_setting_change_area(Some("ConvertibleSlateMode")));
}

#[test]
fn cached_platform_languages_contains_only_non_empty_trimmed_tags() {
    let langs = cached();
    for tag in langs {
        assert!(!tag.is_empty());
        assert_eq!(tag.trim(), tag);
    }
}

#[cfg(target_os = "windows")]
#[test]
fn windows_preferred_languages_queries_profile_and_ui_apis() {
    let ui = windows_ui_languages();
    assert!(!ui.is_empty(), "GetUserPreferredUILanguages must return at least one language on Windows");

    let profile = windows_globalization_languages();
    assert!(
        profile.as_ref().is_some_and(|langs| !langs.is_empty()),
        "GlobalizationPreferences.Languages must return Windows preferred languages",
    );
    let profile = profile.unwrap();
    let worker = std::thread::spawn(windows_globalization_languages).join().unwrap();
    assert_eq!(worker, Some(profile.clone()));
    assert_eq!(current(), profile);
    assert_eq!(cached(), profile.as_slice());
}
