//! Integration tests for opt-in `App::emoji_family`, `Tx::set_emoji_family`,
//! `Runtime::set_emoji_family`, `Runtime::emoji_family`, memo invalidation, multi-window
//! synchronization, and `rune::text::measure` / `font_epoch`.

use std::cell::Cell;

use rune::text::TextSystem;
use rune::{App, AppSpec, El, Env, Runtime, TextStyle, col, memo, row, text};

const INTER: &[u8] = include_bytes!("../assets/Inter-Regular.ttf");
const PLEX: &[u8] = include_bytes!("../assets/IBMPlexMono-Regular.ttf");
const STYLE: TextStyle = TextStyle::new(16.0, 400.0);

thread_local! {
    static MEMO_BUILDS: Cell<u32> = const { Cell::new(0) };
}

/// Builds a minimal valid SFNT COLR font based on bundled IBM Plex Mono with a custom family name,
/// a Format 12 `cmap` covering `codepoints`, and uniform advance width `advance_units`.
fn make_emoji_font(family: &str, codepoints: &[char], advance_units: u16) -> Vec<u8> {
    let num = u16::from_be_bytes([PLEX[4], PLEX[5]]) as usize;
    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::with_capacity(num + 1);
    for rec in PLEX[12..12 + num * 16].as_chunks::<16>().0 {
        let tag: [u8; 4] = [rec[0], rec[1], rec[2], rec[3]];
        let off = u32::from_be_bytes([rec[8], rec[9], rec[10], rec[11]]) as usize;
        let len = u32::from_be_bytes([rec[12], rec[13], rec[14], rec[15]]) as usize;
        tables.push((tag, PLEX[off..off + len].to_vec()));
    }

    let hhea_metrics = tables
        .iter()
        .find(|(t, _)| t == b"hhea")
        .map(|(_, d)| u16::from_be_bytes([d[34], d[35]]) as usize)
        .unwrap_or(1);
    if let Some((_, hmtx)) = tables.iter_mut().find(|(t, _)| t == b"hmtx") {
        for i in 0..hhea_metrics {
            let at = i * 4;
            if at + 2 <= hmtx.len() {
                hmtx[at..at + 2].copy_from_slice(&advance_units.to_be_bytes());
            }
        }
    }

    let mut cps: Vec<u32> = codepoints.iter().map(|&c| c as u32).collect();
    cps.sort_unstable();
    cps.dedup();
    let sub_len = (16 + 12 * cps.len()) as u32;
    let mut cmap = Vec::with_capacity(12 + sub_len as usize);
    cmap.extend_from_slice(&0u16.to_be_bytes());
    cmap.extend_from_slice(&1u16.to_be_bytes());
    cmap.extend_from_slice(&3u16.to_be_bytes());
    cmap.extend_from_slice(&10u16.to_be_bytes());
    cmap.extend_from_slice(&12u32.to_be_bytes());
    cmap.extend_from_slice(&12u16.to_be_bytes());
    cmap.extend_from_slice(&0u16.to_be_bytes());
    cmap.extend_from_slice(&sub_len.to_be_bytes());
    cmap.extend_from_slice(&0u32.to_be_bytes());
    cmap.extend_from_slice(&(cps.len() as u32).to_be_bytes());
    for (i, cp) in cps.into_iter().enumerate() {
        cmap.extend_from_slice(&cp.to_be_bytes());
        cmap.extend_from_slice(&cp.to_be_bytes());
        cmap.extend_from_slice(&((i as u32) + 1).to_be_bytes());
    }

    let ps = family.replace(' ', "");
    let entries: [(u16, &str); 5] = [
        (1, family),
        (2, "Regular"),
        (4, family),
        (6, &ps),
        (16, family),
    ];
    let mut str_data = Vec::new();
    let mut records = Vec::new();
    for (name_id, val) in entries {
        let offset = str_data.len() as u16;
        for u in val.encode_utf16() {
            str_data.extend_from_slice(&u.to_be_bytes());
        }
        let len = (str_data.len() as u16) - offset;
        records.push((name_id, len, offset));
    }
    let storage_off = (6 + records.len() * 12) as u16;
    let mut name = Vec::with_capacity(storage_off as usize + str_data.len());
    name.extend_from_slice(&0u16.to_be_bytes());
    name.extend_from_slice(&(records.len() as u16).to_be_bytes());
    name.extend_from_slice(&storage_off.to_be_bytes());
    for (name_id, len, offset) in records {
        name.extend_from_slice(&3u16.to_be_bytes());
        name.extend_from_slice(&1u16.to_be_bytes());
        name.extend_from_slice(&0x0409u16.to_be_bytes());
        name.extend_from_slice(&name_id.to_be_bytes());
        name.extend_from_slice(&len.to_be_bytes());
        name.extend_from_slice(&offset.to_be_bytes());
    }
    name.extend_from_slice(&str_data);

    tables.retain(|(tag, _)| !matches!(tag, b"cmap" | b"name" | b"COLR"));
    tables.push((*b"cmap", cmap));
    tables.push((*b"name", name));
    tables.push((*b"COLR", vec![0u8; 14]));
    tables.sort_by_key(|(tag, _)| *tag);

    let count = tables.len() as u16;
    let mut entry_sel = 0u16;
    while (1u16 << (entry_sel + 1)) <= count {
        entry_sel += 1;
    }
    let search_range = (1u16 << entry_sel) * 16;
    let range_shift = count * 16 - search_range;

    let mut out = Vec::new();
    out.extend_from_slice(&[0, 1, 0, 0]);
    out.extend_from_slice(&count.to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_sel.to_be_bytes());
    out.extend_from_slice(&range_shift.to_be_bytes());

    let mut offset = 12 + tables.len() * 16;
    for (tag, data) in &tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        offset += (data.len() + 3) & !3;
    }
    for (_, data) in tables {
        out.extend_from_slice(&data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    out
}

fn view(_: &(), _: &mut rune::Cx<()>) -> El {
    col().children([
        row().child(text("Launch 🚀 🇧🇷").key("emoji-label").font(STYLE)),
        row().child(memo("emoji-card", 0, || {
            MEMO_BUILDS.set(MEMO_BUILDS.get() + 1);
            row().child(text("Heart ❤️").key("memo-emoji").font(STYLE))
        })),
        row().child(text("Plain text only").key("plain-label").font(STYLE)),
    ])
}

fn width_of(rt: &Runtime<()>, key: &str) -> f32 {
    rt.elements()
        .iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing element {key}"))
        .target[2]
}

#[test]
fn app_emoji_family_normalizes_empty_and_configures_runtime() {
    let _guard = rune::text::with_families("Inter", "ui-monospace");
    let wide = make_emoji_font("App Wide Emoji", &['\u{1F680}', '\u{1F1E7}', '\u{1F1F7}', '\u{2764}'], 2400);
    let narrow = make_emoji_font("App Narrow Emoji", &['\u{1F680}', '\u{1F1E7}', '\u{1F1F7}', '\u{2764}'], 400);
    assert_eq!(rune::text::add_font(wide), vec!["App Wide Emoji"]);
    assert_eq!(rune::text::add_font(narrow), vec!["App Narrow Emoji"]);

    // Empty / whitespace string normalizes to None.
    let spec_empty = AppSpec::new("empty", (400.0, 200.0), || (), view);
    let rt_empty = App::new(spec_empty)
        .emoji_family("   ")
        .headless_runtime((400.0, 200.0), Env::new(true, 1.0));
    assert_eq!(rt_empty.emoji_family(), None);

    // Explicit App::emoji_family applies immediately at launch.
    let mut rt_narrow = App::new(AppSpec::new("narrow", (400.0, 200.0), || (), view))
        .emoji_family("App Narrow Emoji")
        .headless_runtime((400.0, 200.0), Env::new(true, 1.0));
    rt_narrow.frame(0.0);
    assert_eq!(rt_narrow.emoji_family(), Some("App Narrow Emoji"));
    let narrow_w = width_of(&rt_narrow, "emoji-label");
    let plain_narrow = width_of(&rt_narrow, "plain-label");

    let mut rt_wide = App::new(AppSpec::new("wide", (400.0, 200.0), || (), view))
        .emoji_family("App Wide Emoji")
        .headless_runtime((400.0, 200.0), Env::new(true, 1.0));
    rt_wide.frame(0.0);
    assert_eq!(rt_wide.emoji_family(), Some("App Wide Emoji"));
    let wide_w = width_of(&rt_wide, "emoji-label");
    let plain_wide = width_of(&rt_wide, "plain-label");

    assert!(wide_w > narrow_w + 20.0, "emoji width must follow App::emoji_family: narrow={narrow_w}, wide={wide_w}");
    assert_eq!(plain_narrow, plain_wide, "non-emoji text width must be unaffected by emoji_family");

    rt_wide.set_emoji_family(None);
}

#[test]
fn tx_and_runtime_set_emoji_family_rebuild_memos_sync_windows_and_update_measure() {
    let _guard = rune::text::with_families("Inter", "ui-monospace");
    let narrow = make_emoji_font("Tx Narrow Emoji", &['\u{1F680}', '\u{1F1E7}', '\u{1F1F7}', '\u{2764}'], 450);
    let wide = make_emoji_font("Tx Wide Emoji", &['\u{1F680}', '\u{1F1E7}', '\u{1F1F7}', '\u{2764}'], 2200);
    assert_eq!(rune::text::add_font(narrow), vec!["Tx Narrow Emoji"]);
    assert_eq!(rune::text::add_font(wide), vec!["Tx Wide Emoji"]);

    let make_rt = || {
        let text = TextSystem::new(&[INTER, PLEX], ("Inter", "IBM Plex Mono"));
        let mut rt = Runtime::with_text((), view, (500.0, 300.0), Env::new(true, 2.0), text);
        rt.frame(0.0);
        rt
    };

    let mut a = make_rt();
    let mut b = make_rt();
    assert_eq!(a.emoji_family(), None);
    assert_eq!(b.emoji_family(), None);

    // Repeating None when already None asks for no frame and rebuilds nothing.
    a.frame(1.0 / 60.0);
    let (builds0, commits0) = (MEMO_BUILDS.get(), a.stats.commits);
    let stale_noop = a.update_if(|_, tx| {
        tx.set_emoji_family(None);
        false
    });
    a.frame(2.0 / 60.0);
    assert!(!stale_noop, "setting None when already None must not mark stale");
    assert_eq!(a.stats.commits, commits0, "no commit on no-op");
    assert_eq!(MEMO_BUILDS.get(), builds0, "no memo rebuild on no-op");
    assert_eq!(a.stats.shaped, 0, "no shaping on no-op");

    // Switch via Tx::set_emoji_family to narrow emoji font.
    let ep0 = rune::text::font_epoch();
    let stale_narrow = a.update_if(|_, tx| {
        tx.set_emoji_family(Some("Tx Narrow Emoji"));
        false
    });
    assert!(stale_narrow, "switching emoji_family must mark runtime stale");
    a.frame(3.0 / 60.0);
    assert_eq!(a.emoji_family(), Some("Tx Narrow Emoji"));
    assert_eq!(MEMO_BUILDS.get(), builds0 + 1, "memoized subtree must rebuild on emoji_family switch");
    let ep1 = rune::text::font_epoch();
    assert_ne!(ep0, ep1, "font_epoch must advance on emoji_family switch");
    let narrow_a = width_of(&a, "emoji-label");
    let narrow_measured = rune::text::measure("Launch 🚀 🇧🇷", &[], STYLE, None).0;
    assert!((narrow_measured - narrow_a).abs() < 0.6, "measure follows: {narrow_measured} vs {narrow_a}");

    // Another runtime on the same thread follows on its next frame.
    b.frame(3.0 / 60.0);
    assert_eq!(b.emoji_family(), Some("Tx Narrow Emoji"));
    assert_eq!(width_of(&b, "emoji-label"), narrow_a);

    // A new window opened after the switch starts in the chosen emoji_family.
    let mut c = App::new(AppSpec::new("later", (500.0, 300.0), || (), view))
        .headless_runtime((500.0, 300.0), Env::new(true, 2.0));
    assert_eq!(c.emoji_family(), Some("Tx Narrow Emoji"));
    c.frame(0.0);
    assert_eq!(width_of(&c, "emoji-label"), narrow_a);

    // Repeating the same family via Tx::set_emoji_family is a no-op.
    let (builds1, commits1) = (MEMO_BUILDS.get(), a.stats.commits);
    let stale_same = a.update_if(|_, tx| {
        tx.set_emoji_family(Some("  Tx Narrow Emoji  "));
        false
    });
    a.frame(4.0 / 60.0);
    assert!(!stale_same, "repeating active emoji_family must not mark stale");
    assert_eq!(a.stats.commits, commits1);
    assert_eq!(MEMO_BUILDS.get(), builds1);

    // Switch via Runtime::set_emoji_family to wide emoji font.
    a.set_emoji_family(Some("Tx Wide Emoji"));
    a.frame(5.0 / 60.0);
    assert_eq!(a.emoji_family(), Some("Tx Wide Emoji"));
    let wide_a = width_of(&a, "emoji-label");
    assert!(wide_a > narrow_a + 20.0, "wide emoji font must widen emoji label: {narrow_a} -> {wide_a}");
    let wide_measured = rune::text::measure("Launch 🚀 🇧🇷", &[], STYLE, None).0;
    assert!((wide_measured - wide_a).abs() < 0.6, "measure follows wide switch: {wide_measured} vs {wide_a}");

    // Clear back to None via Runtime::set_emoji_family(None).
    a.set_emoji_family(None);
    a.frame(6.0 / 60.0);
    assert_eq!(a.emoji_family(), None);
}
