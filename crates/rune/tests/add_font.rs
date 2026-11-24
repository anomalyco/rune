//! Headless `Runtime` tests for runtime font registration via `Tx::add_font` and
//! `Runtime::add_font`: immediate layout invalidation, memo rebuilds, `measure`/`font_epoch`
//! propagation, multi-runtime follow, new windows, RTL text, and duplicate/invalid no-ops.

use std::cell::Cell;

use rune::text::{TextSystem, font_epoch, measure};
use rune::{App, AppSpec, Direction, El, Env, Family, Runtime, TextStyle, col, memo, row, text};

const INTER: &[u8] = include_bytes!("../assets/Inter-Regular.ttf");
const PLEX: &[u8] = include_bytes!("../assets/IBMPlexMono-Regular.ttf");
const GEIST_MONO: &[u8] = include_bytes!("../../../www/public/fonts/geist-mono.ttf");
static ONLY_INTER: &[&[u8]] = &[INTER];

thread_local! {
    static MEMO_BUILDS: Cell<u32> = const { Cell::new(0) };
}

const STYLE: TextStyle = TextStyle::new(14.0, 400.0);

fn view(_: &(), _: &mut rune::Cx<()>) -> El {
    col().children([
        row().child(text("iiiiiiiiii 0123456789").key("sans").font(STYLE)),
        row().child(memo("card", 0, || {
            MEMO_BUILDS.set(MEMO_BUILDS.get() + 1);
            row().child(text("Memoized iiiiiiiiii").key("memo").font(STYLE))
        })),
        row().child(text("iiiiiiiiii named").key("named").font(STYLE.family(Family::Named("Geist Mono")))),
        row().child(text("שלום iiiiiiiiii").key("rtl").dir(Direction::Rtl).font(STYLE)),
    ])
}

fn launch(fonts: &[&'static [u8]], families: (&str, &str)) -> Runtime<()> {
    let ts = TextSystem::new(fonts, families);
    let mut rt = Runtime::with_text((), view, (600.0, 400.0), Env::new(true, 2.0), ts);
    rt.frame(0.0);
    rt
}

fn width_of(rt: &Runtime<()>, key: &str) -> f32 {
    rt.elements()
        .iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing element {key}"))
        .target[2]
}

#[test]
fn tx_and_runtime_add_font_register_invalidate_deduplicate_and_propagate() {
    // Baseline: an app with only Inter does not change when idle or given invalid font bytes.
    let mut rt = launch(&[INTER], ("Inter", "Inter"));
    let mut peer = launch(&[INTER], ("Inter", "Inter"));
    rt.frame(1.0 / 120.0);
    let sans_before = width_of(&rt, "sans");
    let memo_before = width_of(&rt, "memo");
    let named_before = width_of(&rt, "named");
    let rtl_before = width_of(&rt, "rtl");
    let epoch0 = font_epoch();
    let builds0 = MEMO_BUILDS.get();
    let commits0 = rt.stats.commits;

    // Empty and invalid bytes do nothing and never ask for a frame.
    let stale_empty = rt.update_if(|_, tx| {
        assert!(tx.add_font(&[][..]).is_empty());
        assert!(tx.add_font(b"corrupt font bytes".to_vec()).is_empty());
        false
    });
    assert!(!stale_empty, "invalid font bytes must not mark runtime stale");
    assert!(rt.add_font(&[][..]).is_empty());
    assert!(rt.add_font(b"also invalid".as_slice()).is_empty());
    assert_eq!(font_epoch(), epoch0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.stats.commits, commits0, "no commit on invalid add_font");
    assert_eq!(MEMO_BUILDS.get(), builds0, "no memo rebuild on invalid add_font");

    // 1) `Tx::add_font` + `Tx::set_font_families` in the same handler.
    let fresh_plex = launch(&[INTER, PLEX], ("IBM Plex Mono", "IBM Plex Mono"));
    let want_sans = width_of(&fresh_plex, "sans");
    let want_memo = width_of(&fresh_plex, "memo");

    let mut returned = Vec::new();
    let stale_add = rt.update_if(|_, tx| {
        returned = tx.add_font(PLEX);
        if let Some(family) = returned.first() {
            tx.set_font_families(family.clone(), family.clone());
        }
        false
    });
    assert_eq!(returned, vec!["IBM Plex Mono"]);
    assert!(stale_add, "adding a font and switching slots asks for a frame");
    let epoch1 = font_epoch();
    assert_ne!(epoch1, epoch0);

    rt.frame(1.0 / 120.0);
    assert_eq!(rt.font_families(), ("IBM Plex Mono", "IBM Plex Mono"));
    assert_eq!(width_of(&rt, "sans"), want_sans, "matches fresh launch width");
    assert_eq!(width_of(&rt, "memo"), want_memo, "memoized subtree rebuilt in new font");
    assert!((width_of(&rt, "sans") - sans_before).abs() > 10.0);
    assert!((width_of(&rt, "memo") - memo_before).abs() > 5.0);
    assert!((width_of(&rt, "rtl") - rtl_before).abs() > 5.0, "RTL line relayouts in new font");
    assert!(MEMO_BUILDS.get() > builds0);

    let measured_sans = measure("iiiiiiiiii 0123456789", &[], STYLE, None).0;
    assert!((measured_sans - want_sans).abs() < 0.51, "measure matches runtime: {measured_sans} vs {want_sans}");

    // Peer runtime on the same thread follows on its next frame.
    peer.frame(1.0 / 120.0);
    assert_eq!(width_of(&peer, "sans"), want_sans);

    // Duplicate `Tx::add_font` returns family names but asks for no frame and rebuilds nothing.
    let builds_dup = MEMO_BUILDS.get();
    let commits_dup = rt.stats.commits;
    let mut dup_names = Vec::new();
    let stale_dup = rt.update_if(|_, tx| {
        dup_names = tx.add_font(PLEX.to_vec());
        false
    });
    assert_eq!(dup_names, vec!["IBM Plex Mono"]);
    assert!(!stale_dup, "duplicate font bytes must not mark runtime stale");
    assert_eq!(font_epoch(), epoch1);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.stats.commits, commits_dup);
    assert_eq!(MEMO_BUILDS.get(), builds_dup);

    // 2) `Tx::add_font` alone (without `set_font_families`) updates elements already using `Family::Named`.
    let mut geist_names = Vec::new();
    let stale_named = rt.update_if(|_, tx| {
        geist_names = tx.add_font(GEIST_MONO);
        false
    });
    assert_eq!(geist_names, vec!["Geist Mono"]);
    assert!(stale_named, "new runtime font marks runtime stale even without set_font_families");
    let epoch2 = font_epoch();
    assert_ne!(epoch2, epoch1);
    rt.frame(1.0 / 120.0);
    let named_after = width_of(&rt, "named");
    assert!((named_after - named_before).abs() > 10.0, "Family::Named updated: {named_before} -> {named_after}");

    // 3) `Runtime::add_font` outside a handler returns existing families on duplicate without invalidating.
    let commits_rt_dup = rt.stats.commits;
    assert_eq!(rt.add_font(GEIST_MONO), vec!["Geist Mono"]);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.stats.commits, commits_rt_dup);

    // 4) A new window opened after `add_font` starts with the runtime-registered fonts available.
    let mut spec = AppSpec::new("runtime-fonts", (600.0, 400.0), || (), view);
    spec.fonts = ONLY_INTER;
    spec.families = ("Inter", "Inter");
    let mut new_win = App::new(spec).headless_runtime((600.0, 400.0), Env::new(true, 2.0));
    new_win.frame(0.0);
    assert_eq!(width_of(&new_win, "sans"), want_sans);
    assert_eq!(width_of(&new_win, "named"), named_after);
}
