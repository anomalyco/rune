use std::collections::HashSet;
use std::path::PathBuf;

use parley::fontique::{Collection, CollectionOptions};

use super::*;
use crate::fonts::system_fonts::installed::{finalize, reader_sfnt_meta};

const SAMPLE_ANDROID_14_FONTS_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<!-- Realistic Android 14 /system/etc/fonts.xml excerpt -->
<familyset version="23">
    <family name="sans-serif">
        <font weight="100" style="normal">Roboto-Thin.ttf</font>
        <font weight="100" style="italic">Roboto-ThinItalic.ttf</font>
        <font weight="300" style="normal">Roboto-Light.ttf</font>
        <font weight="300" style="italic">Roboto-LightItalic.ttf</font>
        <font weight="400" style="normal">Roboto-Regular.ttf</font>
        <font weight="400" style="italic">Roboto-Italic.ttf</font>
        <font weight="500" style="normal">Roboto-Medium.ttf</font>
        <font weight="500" style="italic">Roboto-MediumItalic.ttf</font>
        <font weight="700" style="normal">Roboto-Bold.ttf</font>
        <font weight="700" style="italic">Roboto-BoldItalic.ttf</font>
        <font weight="900" style="normal">Roboto-Black.ttf</font>
        <font weight="900" style="italic">Roboto-BlackItalic.ttf</font>
    </family>

    <alias name="sans-serif-thin" to="sans-serif" weight="100" />
    <alias name="sans-serif-light" to="sans-serif" weight="300" />
    <alias name="sans-serif-medium" to="sans-serif" weight="500" />
    <alias name="sans-serif-black" to="sans-serif" weight="900" />
    <alias name="arial" to="sans-serif" />
    <alias name="helvetica" to="arial" />
    <alias name="roboto" to="sans-serif" />

    <family name="serif">
        <font weight="400" style="normal">NotoSerif-Regular.ttf</font>
        <font weight="700" style="normal">NotoSerif-Bold.ttf</font>
        <font weight="400" style="italic">NotoSerif-Italic.ttf</font>
        <font weight="700" style="italic">NotoSerif-BoldItalic.ttf</font>
    </family>
    <alias name="times new roman" to="serif" />

    <family name="monospace">
        <font weight="400" style="normal">DroidSansMono.ttf</font>
    </family>
    <alias name="courier new" to="monospace" />
    <alias name="sans-serif-monospace" to="monospace" />

    <family lang="und-Arab" variant="elegant">
        <font weight="400" style="normal">NotoNaskhArabic-Regular.ttf</font>
        <font weight="700" style="normal">NotoNaskhArabic-Bold.ttf</font>
    </family>
    <family lang="und-Arab" variant="compact">
        <font weight="400" style="normal">NotoNaskhArabicUI-Regular.ttf</font>
        <font weight="700" style="normal">NotoNaskhArabicUI-Bold.ttf</font>
    </family>
    <family lang="ur-Arab">
        <font weight="400" style="normal">NotoNastaliqUrdu-Regular.ttf</font>
    </family>
    <family lang="und-Hebr">
        <font weight="400" style="normal">NotoSansHebrew-Regular.ttf</font>
        <font weight="700" style="normal">NotoSansHebrew-Bold.ttf</font>
    </family>
    <family lang="und-Deva" variant="compact">
        <font weight="400" style="normal">NotoSansDevanagariUI-Regular.ttf</font>
        <font weight="700" style="normal">NotoSansDevanagariUI-Bold.ttf</font>
    </family>
    <family lang="und-Thai">
        <font weight="400" style="normal">NotoSansThai-Regular.ttf</font>
    </family>
    <family lang="und-Ethi">
        <font weight="400" style="normal">NotoSansEthiopic-Regular.ttf</font>
    </family>

    <family lang="zh-Hans">
        <font weight="400" style="normal">NotoSansSC-Regular.otf</font>
        <font weight="400" style="normal" fallbackFor="serif">NotoSerifSC-Regular.otf</font>
    </family>
    <family lang="zh-Hant,zh-Bopo">
        <font weight="400" style="normal">NotoSansTC-Regular.otf</font>
        <font weight="400" style="normal" fallbackFor="serif">NotoSerifTC-Regular.otf</font>
    </family>
    <family lang="zh-Hant-HK">
        <font weight="400" style="normal">NotoSansHK-Regular.otf</font>
    </family>
    <family lang="ja">
        <font weight="400" style="normal">NotoSansJP-Regular.otf</font>
        <font weight="400" style="normal" fallbackFor="serif">NotoSerifJP-Regular.otf</font>
    </family>
    <family lang="ko">
        <font weight="400" style="normal">NotoSansKR-Regular.otf</font>
        <font weight="400" style="normal" fallbackFor="serif">NotoSerifKR-Regular.otf</font>
    </family>

    <family lang="und-Zmth">
        <font weight="400" style="normal">NotoSansMath-Regular.ttf</font>
    </family>
    <family lang="und-Zsym">
        <font weight="400" style="normal">NotoSansSymbols-Regular-Subsetted.ttf</font>
        <font weight="400" style="normal">NotoSansSymbols-Regular-Subsetted2.ttf</font>
    </family>
    <family lang="und-Zsye">
        <font weight="400" style="normal">NotoColorEmoji.ttf</font>
    </family>
    <family>
        <font weight="400" style="normal">DroidSansFallback.ttf</font>
    </family>
</familyset>
"#;

const SAMPLE_ANDROID_15_FALLBACK_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<familyset version="23">
    <family name="sans-serif">
        <font weight="400" style="normal" supportedAxes="wght">
            Roboto-Regular.ttf
            <axis tag="wght" stylevalue="400" />
        </font>
        <font weight="400" style="italic" supportedAxes="wght">
            Roboto-Italic.ttf
            <axis tag="wght" stylevalue="400" />
        </font>
        <font weight="700" style="normal" supportedAxes="wght">
            Roboto-Regular.ttf
            <axis tag="wght" stylevalue="700" />
        </font>
        <font weight="700" style="italic" supportedAxes="wght">
            Roboto-Italic.ttf
            <axis tag="wght" stylevalue="700" />
        </font>
    </family>
    <family name="ignored-oem" ignore="true">
        <font weight="400" style="normal">IgnoredFont.ttf</font>
    </family>
    <family name="monospace">
        <font weight="400" style="normal">RobotoMono-Regular.ttf</font>
    </family>
    <family lang="ja">
        <font weight="400" style="normal" index="0">NotoSansCJK-Regular.ttc</font>
        <font weight="400" style="normal" index="0" fallbackFor="serif">NotoSerifCJK-Regular.ttc</font>
    </family>
    <family lang="ko">
        <font weight="400" style="normal" index="1">NotoSansCJK-Regular.ttc</font>
    </family>
    <family lang="zh-Hans">
        <font weight="400" style="normal" index="2">NotoSansCJK-Regular.ttc</font>
    </family>
    <family lang="zh-Hant,zh-Bopo">
        <font weight="400" style="normal" index="3">NotoSansCJK-Regular.ttc</font>
    </family>
    <family lang="und-Zsye">
        <font weight="400" style="normal">NotoColorEmoji.ttf</font>
    </family>
</familyset>
"#;

fn mock_resolve_all(file: &str) -> Option<PathBuf> {
    Some(PathBuf::from("/system/fonts").join(file))
}

fn names_of(paths: Vec<PathBuf>) -> Vec<String> {
    paths
        .into_iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn parses_android_14_weights_italics_and_aliases() {
    let cfg = FontConfig::parse(SAMPLE_ANDROID_14_FONTS_XML, mock_resolve_all);

    let sans = cfg
        .families
        .iter()
        .find(|f| f.name.as_deref() == Some("sans-serif"))
        .expect("sans-serif family");
    assert_eq!(sans.fonts.len(), 12);
    assert!(sans.fonts.iter().any(|f| f.weight == 500 && !f.italic && f.file_name == "Roboto-Medium.ttf"));
    assert!(sans.fonts.iter().any(|f| f.weight == 500 && f.italic && f.file_name == "Roboto-MediumItalic.ttf"));
    assert!(sans.fonts.iter().any(|f| f.weight == 900 && f.italic && f.file_name == "Roboto-BlackItalic.ttf"));

    // Chained alias helvetica -> arial -> sans-serif.
    let helv = names_of(cfg.family_files(&["Helvetica"]));
    assert_eq!(helv.len(), 12);
    assert_eq!(helv[0], "Roboto-Thin.ttf");
    assert!(helv.contains(&"Roboto-BoldItalic.ttf".to_owned()));

    // Weighted alias sans-serif-medium -> sans-serif weight 500 narrows to weight 500 faces.
    let med = names_of(cfg.family_files(&["sans-serif-medium"]));
    assert_eq!(med, vec!["Roboto-Medium.ttf", "Roboto-MediumItalic.ttf"]);

    // Case-insensitive alias and monospace lookup.
    assert_eq!(names_of(cfg.family_files(&["Courier New"])), vec!["DroidSansMono.ttf"]);
    assert_eq!(names_of(cfg.monospace_files()), vec!["DroidSansMono.ttf"]);
    assert_eq!(
        names_of(cfg.family_files(&["Times New Roman"])),
        vec![
            "NotoSerif-Regular.ttf",
            "NotoSerif-Bold.ttf",
            "NotoSerif-Italic.ttf",
            "NotoSerif-BoldItalic.ttf",
        ]
    );
}

#[test]
fn parses_android_15_variable_axes_deduplicates_files_and_skips_ignored() {
    let cfg = FontConfig::parse(SAMPLE_ANDROID_15_FALLBACK_XML, mock_resolve_all);

    // Variable font entries sharing Roboto-Regular.ttf / Roboto-Italic.ttf deduplicate paths.
    let sans_files = names_of(cfg.family_files(&["sans-serif"]));
    assert_eq!(sans_files, vec!["Roboto-Regular.ttf", "Roboto-Italic.ttf"]);

    let sans = cfg
        .families
        .iter()
        .find(|f| f.name.as_deref() == Some("sans-serif"))
        .expect("sans-serif");
    assert_eq!(sans.fonts[0].wght_axis_values, vec![400]);
    assert_eq!(sans.fonts[2].wght_axis_values, vec![700]);
    assert!(sans.fonts[1].italic);

    // ignore="true" family is excluded.
    assert!(cfg.family_files(&["ignored-oem"]).is_empty());
    assert!(cfg.family_files(&["IgnoredFont.ttf"]).is_empty());

    // Monospace from Android 15 fallback XML.
    assert_eq!(names_of(cfg.monospace_files()), vec!["RobotoMono-Regular.ttf"]);

    // CJK TTC entries parse index and deduplicate to the single TTC file.
    let jp = cfg.families.iter().find(|f| f.langs.iter().any(|l| l.lang == "ja")).unwrap();
    assert_eq!(jp.fonts[0].index, 0);
    let hans = cfg.families.iter().find(|f| f.langs.iter().any(|l| l.script.as_deref() == Some("hans"))).unwrap();
    assert_eq!(hans.fonts[0].index, 2);
    assert_eq!(
        names_of(cfg.fallback_family_files(&['\u{6F22}'], Some("ja"))),
        vec!["NotoSansCJK-Regular.ttc"]
    );
}

#[test]
fn family_lookup_by_file_name_and_base_stem_avoids_prefix_collisions() {
    let cfg = FontConfig::parse(SAMPLE_ANDROID_14_FONTS_XML, mock_resolve_all);

    // Exact filename lookup.
    assert_eq!(names_of(cfg.family_files(&["Roboto-Regular.ttf"])), vec!["Roboto-Regular.ttf"]);

    // Base-stem lookup for unnamed fallback fonts.
    assert_eq!(names_of(cfg.family_files(&["Noto Color Emoji"])), vec!["NotoColorEmoji.ttf"]);
    assert_eq!(
        names_of(cfg.family_files(&["Noto Sans Symbols"])),
        vec![
            "NotoSansSymbols-Regular-Subsetted.ttf",
            "NotoSansSymbols-Regular-Subsetted2.ttf",
        ]
    );
    assert_eq!(names_of(cfg.family_files(&["Noto Sans Math"])), vec!["NotoSansMath-Regular.ttf"]);
    assert_eq!(names_of(cfg.family_files(&["Droid Sans Mono"])), vec!["DroidSansMono.ttf"]);

    // "Noto Sans" must NOT falsely match NotoSansArabic, NotoSansHebrew, NotoSansSC, etc.
    assert!(cfg.family_files(&["Noto Sans"]).is_empty());
}

#[test]
fn cjk_fallbacks_respect_locale_and_script_priority() {
    let cfg = FontConfig::parse(SAMPLE_ANDROID_14_FONTS_XML, mock_resolve_all);

    // Han with Japanese locale puts JP first and excludes fallbackFor="serif".
    let ja = names_of(cfg.fallback_family_files(&['\u{6F22}'], Some("ja")));
    assert_eq!(ja[0], "NotoSansJP-Regular.otf");
    assert!(!ja.iter().any(|n| n.contains("Serif")));

    // Han with Korean locale puts KR first.
    let ko = names_of(cfg.fallback_family_files(&['\u{6F22}'], Some("ko-KR")));
    assert_eq!(ko[0], "NotoSansKR-Regular.otf");

    // Han with Traditional Chinese (TW) puts TC first.
    let tw = names_of(cfg.fallback_family_files(&['\u{6F22}'], Some("zh-TW")));
    assert_eq!(tw[0], "NotoSansTC-Regular.otf");

    // Han with Hong Kong locale puts HK first, then TC.
    let hk = names_of(cfg.fallback_family_files(&['\u{6F22}'], Some("zh-HK")));
    assert_eq!(hk[0], "NotoSansHK-Regular.otf");
    assert_eq!(hk[1], "NotoSansTC-Regular.otf");

    // Han with no locale or English defaults to Simplified Chinese first.
    let default_han = names_of(cfg.fallback_family_files(&['\u{6F22}'], None));
    assert_eq!(default_han[0], "NotoSansSC-Regular.otf");
    let en_han = names_of(cfg.fallback_family_files(&['\u{6F22}'], Some("en-US")));
    assert_eq!(en_han[0], "NotoSansSC-Regular.otf");

    // Kana always picks JP; Hangul always picks KR; Bopomofo picks TC.
    assert_eq!(names_of(cfg.fallback_family_files(&['\u{3042}'], Some("zh-CN"))), vec!["NotoSansJP-Regular.otf"]);
    assert_eq!(names_of(cfg.fallback_family_files(&['\u{D55C}'], Some("ja"))), vec!["NotoSansKR-Regular.otf"]);
    assert_eq!(names_of(cfg.fallback_family_files(&['\u{3105}'], None)), vec!["NotoSansTC-Regular.otf"]);
}

#[test]
fn script_variant_locale_emoji_symbols_and_math_fallbacks() {
    let cfg = FontConfig::parse(SAMPLE_ANDROID_14_FONTS_XML, mock_resolve_all);

    // Generic Arabic prefers compact before elegant, and does NOT hijack with Urdu Nastaliq.
    let ar = names_of(cfg.fallback_family_files(&['\u{0639}'], None));
    assert_eq!(
        ar,
        vec![
            "NotoNaskhArabicUI-Regular.ttf",
            "NotoNaskhArabicUI-Bold.ttf",
            "NotoNaskhArabic-Regular.ttf",
            "NotoNaskhArabic-Bold.ttf",
        ]
    );

    // Urdu locale puts ur-Arab first, followed by generic Arabic.
    let ur = names_of(cfg.fallback_family_files(&['\u{0639}'], Some("ur-PK")));
    assert_eq!(ur[0], "NotoNastaliqUrdu-Regular.ttf");
    assert_eq!(ur[1], "NotoNaskhArabicUI-Regular.ttf");

    // Other scripts: Hebrew, Devanagari, Thai, Ethiopic.
    assert_eq!(
        names_of(cfg.fallback_family_files(&['\u{05D0}'], None)),
        vec!["NotoSansHebrew-Regular.ttf", "NotoSansHebrew-Bold.ttf"]
    );
    assert_eq!(
        names_of(cfg.fallback_family_files(&['\u{0915}'], None)),
        vec!["NotoSansDevanagariUI-Regular.ttf", "NotoSansDevanagariUI-Bold.ttf"]
    );
    assert_eq!(names_of(cfg.fallback_family_files(&['\u{0E01}'], None)), vec!["NotoSansThai-Regular.ttf"]);
    assert_eq!(names_of(cfg.fallback_family_files(&['\u{1200}'], None)), vec!["NotoSansEthiopic-Regular.ttf"]);

    // Emoji, symbols, and math.
    assert_eq!(names_of(cfg.fallback_family_files(&['\u{1F600}'], None)), vec!["NotoColorEmoji.ttf"]);
    assert_eq!(names_of(cfg.emoji_files()), vec!["NotoColorEmoji.ttf"]);
    assert_eq!(
        names_of(cfg.fallback_family_files(&['\u{2318}'], None)),
        vec![
            "NotoSansSymbols-Regular-Subsetted.ttf",
            "NotoSansSymbols-Regular-Subsetted2.ttf",
        ]
    );
    assert_eq!(names_of(cfg.fallback_family_files(&['\u{211D}'], None)), vec!["NotoSansMath-Regular.ttf"]);

    // Unlisted script falls back to unnamed untagged fallback (DroidSansFallback.ttf).
    assert_eq!(names_of(cfg.fallback_family_files(&['\u{0F40}'], None)), vec!["DroidSansFallback.ttf"]);
}

#[test]
fn missing_files_are_skipped_cleanly() {
    let present: HashSet<&str> = ["Roboto-Regular.ttf", "DroidSansMono.ttf"].into_iter().collect();
    let cfg = FontConfig::parse(SAMPLE_ANDROID_14_FONTS_XML, |file| {
        present.contains(file).then(|| PathBuf::from("/system/fonts").join(file))
    });

    assert_eq!(names_of(cfg.family_files(&["sans-serif"])), vec!["Roboto-Regular.ttf"]);
    assert_eq!(names_of(cfg.monospace_files()), vec!["DroidSansMono.ttf"]);
    // Families whose files are all missing are omitted completely.
    assert!(cfg.family_files(&["serif"]).is_empty());
    assert!(cfg.fallback_family_files(&['\u{0639}'], None).is_empty());
    assert!(cfg.emoji_files().is_empty());
    // Weighted alias whose target weight file is missing falls back to available target faces.
    assert_eq!(names_of(cfg.family_files(&["sans-serif-medium"])), vec!["Roboto-Regular.ttf"]);
}

#[test]
fn malformed_xml_cycles_entities_and_non_familyset_documents() {
    // Non-familyset XML returns empty.
    let html = "<html><body><family name=\"sans-serif\"><font>Roboto-Regular.ttf</font></family></body></html>";
    assert!(FontConfig::parse(html, mock_resolve_all).families.is_empty());

    // Malformed / truncated / garbage inputs must never panic or loop.
    let hostile = [
        "",
        "   \n\t  ",
        "not xml at all",
        "<<<>>>",
        "<!-- unclosed comment",
        "<familyset><family name=\"sans-serif\"><font>Roboto.ttf",
        "<familyset><family name=\"broken><font>Roboto.ttf</font></family></familyset>",
        "<familyset><family name=\"a\"><font><![CDATA[unclosed",
        "<familyset><?xml unclosed",
        "<familyset><!DOCTYPE unclosed",
        "<familyset></familyset></familyset>",
        "<familyset><family name=\"a\"><font weight=\"abc\" index=\"-5\">   </font></family></familyset>",
    ];
    for sample in hostile {
        let _ = FontConfig::parse(sample, mock_resolve_all);
    }

    // Cyclic aliases terminate cleanly without panicking or infinite-looping.
    let cyclic = r#"<familyset>
        <alias name="loop-a" to="loop-b" />
        <alias name="loop-b" to="loop-a" />
        <alias name="self-loop" to="self-loop" />
    </familyset>"#;
    let cyclic_cfg = FontConfig::parse(cyclic, mock_resolve_all);
    assert!(cyclic_cfg.family_files(&["loop-a", "self-loop"]).is_empty());

    // BOM, CDATA, single quotes, and XML entity unescaping all work.
    let entities = "\u{FEFF}<?xml version='1.0'?>\
        <familyset version='23'>\
        <family name='custom &amp; sans'>\
            <font weight='600' style='oblique'><![CDATA[Custom-SemiBold.ttf]]></font>\
        </family>\
        <alias name='custom&#x20;alias' to='custom &amp; sans' />\
        </familyset>";
    let cfg = FontConfig::parse(entities, mock_resolve_all);
    assert_eq!(names_of(cfg.family_files(&["custom & sans"])), vec!["Custom-SemiBold.ttf"]);
    assert_eq!(names_of(cfg.family_files(&["custom alias"])), vec!["Custom-SemiBold.ttf"]);
    assert_eq!(cfg.families[0].fonts[0].weight, 600);
    assert!(cfg.families[0].fonts[0].italic);
}

#[test]
fn load_from_paths_prefers_font_fallback_xml_and_rejects_parent_traversal() {
    let tmp = std::env::temp_dir().join(format!("rune-android-fonts-test-{}", std::process::id()));
    let fonts_dir = tmp.join("fonts");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&fonts_dir).unwrap();

    let bundled = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../rune/assets");
    for file in ["Inter-Regular.ttf", "Inter-Medium.ttf", "Inter-SemiBold.ttf", "IBMPlexMono-Regular.ttf"] {
        std::fs::copy(bundled.join(file), fonts_dir.join(file)).unwrap();
    }

    let fallback_xml = tmp.join("font_fallback.xml");
    let legacy_xml = tmp.join("fonts.xml");
    std::fs::write(
        &legacy_xml,
        r#"<familyset version="23">
            <family name="sans-serif">
                <font weight="400" style="normal">Inter-Regular.ttf</font>
            </family>
        </familyset>"#,
    )
    .unwrap();
    std::fs::write(
        &fallback_xml,
        r#"<familyset version="23">
            <family name="sans-serif">
                <font weight="400" style="normal">Inter-Regular.ttf</font>
                <font weight="500" style="normal">Inter-Medium.ttf</font>
                <font weight="600" style="normal">Inter-SemiBold.ttf</font>
                <font weight="400" style="normal">MissingFace.ttf</font>
                <font weight="400" style="normal">../escape.ttf</font>
            </family>
            <family name="monospace">
                <font weight="400" style="normal">IBMPlexMono-Regular.ttf</font>
            </family>
        </familyset>"#,
    )
    .unwrap();

    // Prefers font_fallback.xml over fonts.xml.
    let cfg = FontConfig::load_from_paths(
        &[fallback_xml.as_path(), legacy_xml.as_path()],
        &[fonts_dir.as_path()],
    );
    assert_eq!(
        names_of(cfg.family_files(&["sans-serif"])),
        vec!["Inter-Regular.ttf", "Inter-Medium.ttf", "Inter-SemiBold.ttf"]
    );
    assert_eq!(names_of(cfg.monospace_files()), vec!["IBMPlexMono-Regular.ttf"]);

    // Falls back to fonts.xml when font_fallback.xml is absent or invalid.
    std::fs::write(&fallback_xml, "<html>not a familyset</html>").unwrap();
    let legacy_cfg = FontConfig::load_from_paths(
        &[fallback_xml.as_path(), legacy_xml.as_path()],
        &[fonts_dir.as_path()],
    );
    assert_eq!(names_of(legacy_cfg.family_files(&["sans-serif"])), vec!["Inter-Regular.ttf"]);

    // installed_families() reads real SFNT family names, weights, and monospace flags,
    // and every returned name round-trips through family_files() and fontique::Collection.
    let installed = finalize(cfg.installed_families());
    let installed_names: Vec<&str> = installed.iter().map(|f| f.name()).collect();
    assert_eq!(installed_names, vec!["IBM Plex Mono", "Inter"]);

    let plex = &installed[0];
    assert!(plex.monospace());
    assert_eq!(plex.weights(), &[400]);

    let inter = &installed[1];
    assert!(!inter.monospace());
    assert_eq!(inter.weights(), &[400, 500, 600]);

    let mut collection = Collection::new(CollectionOptions { shared: false, system_fonts: false });
    for fam in &installed {
        let files = cfg.family_files(&[fam.name()]);
        assert!(!files.is_empty(), "installed family {} must resolve via family_files", fam.name());
        collection.load_fonts_from_paths(files);
        assert!(
            collection.family_id(fam.name()).is_some(),
            "installed family {} must register in fontique::Collection under the same name",
            fam.name()
        );
    }

    // OEM font whose filename does not match its SFNT family name still round-trips via sfnt_index.
    let oem_path = fonts_dir.join("OEMCustomFont.ttf");
    std::fs::copy(bundled.join("IBMPlexMono-Regular.ttf"), &oem_path).unwrap();
    let oem_xml = r#"<familyset version="23">
        <family name="monospace">
            <font weight="400" style="normal">OEMCustomFont.ttf</font>
        </family>
    </familyset>"#;
    let oem_cfg = FontConfig::parse(oem_xml, |f| {
        let p = fonts_dir.join(f);
        p.is_file().then_some(p)
    });
    let oem_installed = finalize(oem_cfg.installed_families());
    assert_eq!(oem_installed.len(), 1);
    assert_eq!(oem_installed[0].name(), "IBM Plex Mono");
    assert!(oem_installed[0].monospace());
    assert_eq!(names_of(oem_cfg.family_files(&["IBM Plex Mono"])), vec!["OEMCustomFont.ttf"]);

    // Verify reader_sfnt_meta directly on bundled fonts.
    let mut inter_file = std::fs::File::open(fonts_dir.join("Inter-SemiBold.ttf")).unwrap();
    let inter_meta = reader_sfnt_meta(&mut inter_file, 0).expect("Inter SFNT metadata");
    assert_eq!(inter_meta.family, "Inter");
    assert!(!inter_meta.monospace);
    assert_eq!(inter_meta.weight_class, Some(600));

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn env_switch_disables_system_fonts_for_off_values() {
    use crate::fonts::system_fonts::switch::from_env;
    assert!(from_env(Some("off")));
    assert!(from_env(Some("OFF")));
    assert!(from_env(Some("0")));
    assert!(from_env(Some("false")));
    assert!(from_env(Some("no")));
    assert!(!from_env(Some("on")));
    assert!(!from_env(None));
}
