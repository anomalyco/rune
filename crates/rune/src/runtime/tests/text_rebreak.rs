use super::*;
use parley::Layout;
use crate::text::Ink;

fn resize_view(s: &bool, _: &mut Cx<bool>) -> El {
    let para = "Paragraph text long enough to wrap over several lines as the window narrows.";
    col().w_full().children((0..3).filter(|&i| *s || i != 2).map(|i| crate::el::text(format!("{i}. {para}")).w_full().wrap().key(format!("t{i}"))))
}

fn layout_of<'a>(rt: &'a Runtime<bool>, key: &str) -> &'a Rc<Layout<Ink>> {
    rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some(key)).and_then(|n| n.text_layout.as_ref()?.single()).unwrap()
}

/// A commit lets go of the last commit's text layouts before measuring, so on a resize each
/// wrapped text re-breaks its own layout in place; a text that leaves keeps its layout to draw
/// its exit.
#[test]
fn resizing_rebreaks_kept_texts_in_place_and_leaving_ones_keep_their_layout() {
    let mut rt = Runtime::new(true, resize_view, (700.0, 900.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.set_viewport((600.0, 900.0), 2.0);
    rt.frame(1.0 / 120.0);
    let kept = Rc::as_ptr(layout_of(&rt, "t0"));
    rt.set_viewport((300.0, 900.0), 2.0);
    rt.frame(2.0 / 120.0);
    assert_eq!(Rc::as_ptr(layout_of(&rt, "t0")), kept, "re-broken in place");
    assert!(layout_of(&rt, "t0").len() > 1);
    let leaving = layout_of(&rt, "t2").clone();
    rt.update(|s, _| *s = false);
    rt.frame(3.0 / 120.0);
    assert!(rt.is_exiting(rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("t2")).unwrap().id));
    assert!(Rc::ptr_eq(layout_of(&rt, "t2"), &leaving), "the leaving text draws its exit from its own layout");
}

/// Measuring the content's height without committing leaves the drawn nodes' layouts alone.
#[test]
fn measuring_content_height_keeps_the_drawn_layouts() {
    let mut rt = Runtime::new(true, resize_view, (700.0, 900.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let l = layout_of(&rt, "t0").clone();
    rt.host.fit_height = true;
    rt.content_height();
    assert!(Rc::ptr_eq(layout_of(&rt, "t0"), &l));
}

fn typed(s: &String, _: &mut Cx<String>) -> El {
    col().w_full().child(crate::el::text(s.clone()).w_full().wrap().key("para")).child(crate::el::text("label").key("label"))
}

/// Typing into a paragraph keeps no stale copy of each edit's layout: the replaced text's
/// layouts go when the commit that replaced it ends.
#[test]
fn typing_keeps_no_layout_of_each_edit() {
    let para = "Paragraph text long enough to wrap over several lines as the window narrows. ".repeat(8);
    let mut rt = Runtime::new(para.clone(), typed, (700.0, 900.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let cached = rt.text.cached_layout_count();
    for i in 0..200 {
        rt.update(|s, _| s.push(char::from(b'a' + (i % 26) as u8)));
        rt.frame((i + 1) as f64 / 120.0);
    }
    assert!(rt.text.cached_layout_count() <= cached + 2, "{} layouts cached, {cached} at first", rt.text.cached_layout_count());
}

fn field(s: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
    crate::text_input(s, crate::TextStyle::new(14.0, 400.0)).key("field").w(600.0).on_edit(|s: &mut crate::TextEdit| s)
}

/// The same for a focused text field.
#[test]
fn typing_into_a_field_keeps_no_layout_of_each_edit() {
    let mut edit = crate::TextEdit::new("Paragraph text long enough to wrap over several lines as the window narrows. ".repeat(8));
    edit.focused = true;
    let mut rt = Runtime::new(edit, field, (700.0, 900.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let cached = rt.text.cached_layout_count();
    for i in 0..200 {
        rt.update(|s, _| s.insert("k"));
        rt.frame((i + 1) as f64 / 120.0);
    }
    assert!(rt.text.cached_layout_count() <= cached + 2, "{} layouts cached, {cached} at first", rt.text.cached_layout_count());
}

fn rows(s: &Vec<&'static str>, _: &mut Cx<Vec<&'static str>>) -> El {
    col().children(s.iter().map(|&t| crate::el::text(t)))
}

/// A text replaced by an unrelated one, as a reader showing another note, keeps its layout for
/// when it's shown again.
#[test]
fn a_text_shown_again_after_another_keeps_its_layout() {
    let mut rt = Runtime::new(vec!["The first note's opening paragraph"], rows, (400.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let shaped = rt.text.shaped;
    rt.update(|s, _| s[0] = "Something else entirely, from another note");
    for f in 1..4 { rt.frame(f as f64 / 120.0); }
    rt.update(|s, _| s[0] = "The first note's opening paragraph");
    for f in 4..8 { rt.frame(f as f64 / 120.0); }
    assert_eq!(rt.text.shaped, shaped + 1, "only the other note shaped");
}

/// An edited text's old layout stays while another node shows the same text.
#[test]
fn an_edited_text_shown_elsewhere_keeps_its_layout() {
    let mut rt = Runtime::new(vec!["same words", "same words"], rows, (400.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let shaped = rt.text.shaped;
    rt.update(|s, _| s[0] = "same wordz");
    for f in 1..4 { rt.frame(f as f64 / 120.0); }
    rt.update(|s, _| s[0] = "same words");
    for f in 4..8 { rt.frame(f as f64 / 120.0); }
    assert_eq!(rt.text.shaped, shaped + 1, "only the edit shaped");
}

#[test]
fn an_edit_shares_most_of_its_text_and_a_replacement_does_not() {
    use super::super::text_layout::edited;
    assert!(edited("hello world", "hello world!"));
    assert!(edited("hello world", "hello, world"));
    assert!(edited("12", "13"));
    assert!(!edited("same", "same"));
    assert!(!edited("Show", "Hide"));
    assert!(!edited("The first note", "Another one"));
}

/// Issue #1412: one layout span (an underline on the first line, or on an interior line) must not
/// disable deferred shaping for the plain ASCII lines in a wrapped monospace `by_line` log.
#[test]
fn by_line_rich_log_with_underline_defers_offscreen_plain_lines() {
    struct LogState {
        content: String,
        spans: Vec<crate::Span>,
    }
    fn log_view(s: &LogState, _: &mut Cx<LogState>) -> El {
        col()
            .key("viewport")
            .size(320.0, 320.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(
                crate::rich(s.content.clone(), s.spans.clone())
                    .key("log")
                    .font(crate::TextStyle::new(13.0, 400.0).mono())
                    .by_line()
                    .wrap()
                    .selectable(true)
                    .w_full(),
            )
    }

    let head = "A".repeat(1500);
    let tail: String = (0..1000).map(|i| format!("line {i}\n")).collect();
    let content = format!("{head}\n{tail}");
    let head_spans = vec![crate::Span::new(0, head.len()).underline(crate::Color::BLACK)];

    let target = "line 500";
    let line500_start = content.find(target).unwrap();
    let interior_spans = vec![crate::Span::new(line500_start, line500_start + target.len()).underline(crate::Color::BLACK)];

    // Bring bundled Inter + IBM Plex Mono so the test does not depend on host system monospace
    // fonts (which may be absent in minimal Linux environments, causing `ui-monospace` to fall back
    // to proportional Inter and intentionally disable `by_line` deferral).
    const INTER: &[u8] = include_bytes!("../../../assets/Inter-Regular.ttf");
    const PLEX: &[u8] = include_bytes!("../../../assets/IBMPlexMono-Regular.ttf");
    let _fonts = crate::text::with_fonts(&[INTER, PLEX], ("Inter", "IBM Plex Mono"));

    let env = Env::new(true, 2.0);
    let make = |spans| {
        Runtime::with_text(
            LogState { content: content.clone(), spans },
            log_view,
            (320.0, 320.0),
            env,
            crate::text::TextSystem::new(&[INTER, PLEX], ("Inter", "IBM Plex Mono")),
        )
    };
    let mut plain = make(Vec::new());
    let mut underlined = make(head_spans);
    let mut interior = make(interior_spans);

    plain.frame(0.0);
    underlined.frame(0.0);
    interior.frame(0.0);

    let log_layout = |rt: &Runtime<LogState>| {
        let n = rt.tree.nodes.iter().find(|n| n.key.as_deref() == Some("log")).unwrap();
        match n.text_layout.as_ref().unwrap() {
            crate::text::TextLayout::Paragraphs(p) => p.clone(),
            crate::text::TextLayout::Single(_) => panic!("expected ParagraphLayout"),
        }
    };

    let plain_pl = log_layout(&plain);
    let underlined_pl = log_layout(&underlined);
    let interior_pl = log_layout(&interior);

    assert!(plain_pl.waiting(), "unspanned baseline should leave off-screen plain lines waiting");
    assert!(underlined_pl.waiting(), "underlined head must keep off-screen plain lines waiting");
    assert!(interior_pl.waiting(), "underlined interior line must keep surrounding plain lines waiting");
    assert_eq!(
        underlined.stats.shaped, plain.stats.shaped,
        "underlined head should not shape more paragraphs than the unspanned baseline",
    );
    assert!(
        interior.stats.shaped <= plain.stats.shaped + 2,
        "one underlined interior line should only shape itself eagerly: {} vs baseline {}",
        interior.stats.shaped,
        plain.stats.shaped,
    );
    assert!(underlined.stats.shaped < 200, "shaped count stays bounded: {}", underlined.stats.shaped);
    let height_of = |pl: &Rc<crate::text::ParagraphLayout>| crate::text::TextFlow::Paragraphs(pl).height();
    assert_eq!(height_of(&underlined_pl), height_of(&plain_pl), "total log height matches unspanned baseline");
    assert_eq!(height_of(&interior_pl), height_of(&plain_pl), "interior-underlined log height matches baseline");

    // Visual underline decoration quads are emitted for the underlined head (more quads than plain).
    assert!(
        underlined.instances.len() > plain.instances.len(),
        "underlined head must still emit underline decoration quads",
    );

    // Accessibility tree builds cleanly over mixed shaped + waiting paragraphs.
    let tree = underlined.accessibility_tree();
    let text_runs = tree.nodes.iter().filter(|(_, n)| n.role() == accesskit::Role::TextRun).count();
    assert!(text_runs >= 1000, "a11y tree exposes runs across shaped and waiting paragraphs (got {text_runs})");

    // Once the visible margin fills, the idle window asks for no frames and shapes nothing.
    for _ in 0..20 {
        if !underlined.frame(1.0 / 60.0) {
            break;
        }
    }
    assert!(!underlined.frame(1.0 / 60.0), "idle window stops requesting frames");
    assert_eq!(underlined.stats.shaped, 0, "idle frame shapes nothing");
    assert!(log_layout(&underlined).waiting(), "far off-screen lines remain deferred while idle");
}
