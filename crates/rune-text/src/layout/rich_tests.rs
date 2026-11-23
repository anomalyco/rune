use super::*;

const TOKEN: &str = "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
const WORDS: &str = "a plain sentence that wraps at spaces across a few short lines";

fn lines(l: &Layout<Ink>) -> Vec<std::ops::Range<usize>> {
    l.lines().map(|line| line.text_range()).collect()
}

#[test]
fn wrap_anywhere_breaks_an_unbroken_word_inside_the_width() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);
    let (scale, max_w) = (2.0, 160.0 * 2.0);
    let clipped = ts.layout_rich(TOKEN, &[], style, scale, Some(max_w));
    assert_eq!(clipped.len(), 1, "without wrap-anywhere the token stays on one line");
    assert!(clipped.full_width() > max_w);

    let spans = [Span::new(0, 6).bold()];
    let opts = RichLayoutOptions::new().wrap_anywhere();
    let wrapped = ts.layout_rich_with(TOKEN, &spans, style, scale, Some(max_w), opts);
    assert!(wrapped.len() >= 3, "the token wraps over several lines (got {})", wrapped.len());
    for line in wrapped.lines() {
        assert!(line.metrics().advance <= max_w + 0.5, "line {:?} fits {max_w}", line.text_range());
    }
    let r = lines(&wrapped);
    assert_eq!((r[0].start, r.last().unwrap().end), (0, TOKEN.len()));
    assert!(r.windows(2).all(|w| w[0].end == w[1].start), "lines cover the token without gaps");
}

#[test]
fn wrap_anywhere_keeps_word_breaks_and_defaults_match_layout_rich() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);
    let plain = ts.layout_rich(WORDS, &[], style, 2.0, Some(240.0));
    let anywhere = ts.layout_rich_with(WORDS, &[], style, 2.0, Some(240.0), RichLayoutOptions::new().wrap_anywhere());
    assert!(plain.len() > 1);
    assert_eq!(lines(&plain), lines(&anywhere), "words that fit break at spaces either way");

    let default = ts.layout_rich_with(WORDS, &[], style, 2.0, Some(240.0), RichLayoutOptions::default());
    assert!(Rc::ptr_eq(&plain, &default), "default options share the layout_rich cache entry");
    let aligned = ts.layout_aligned(WORDS, &[], style, 2.0, Some(240.0), Alignment::Center);
    let with = ts.layout_rich_with(WORDS, &[], style, 2.0, Some(240.0), RichLayoutOptions::new().align(Alignment::Center));
    assert!(Rc::ptr_eq(&aligned, &with), "align matches layout_aligned");
}

fn reference_mono_tab_widening(
    ts: &mut TextSystem,
    tabs: &[(usize, u32)],
    spans: &[Span],
    style: TextStyle,
    scale: f32,
) -> Vec<(std::ops::Range<usize>, f32)> {
    let mono_style = matches!(style.family, Family::Mono | Family::SystemMono);
    let default_family = if mono_style { style.family } else { Family::Mono };
    let mut widen: Vec<(std::ops::Range<usize>, f32)> = Vec::new();
    for &(at, columns) in tabs {
        if columns <= 1 {
            continue;
        }
        let (mut sz, mut wt, mut it) = (style.size, style.weight, style.italic);
        for s in spans {
            if (s.start as usize) <= at && at < s.end as usize {
                if let Some(v) = s.size { sz = v; }
                if let Some(v) = s.weight { wt = v; }
                if s.italic { it = true; }
            }
        }
        let probe = TextStyle { family: default_family, size: sz, weight: wt, italic: it, ..style };
        let l = ts.layout_rich(" ", &[], probe, scale, None);
        let col_pt = parley::Cluster::from_byte_index(&l, 0).map_or(0.0, |c| c.advance() / scale);
        if col_pt <= 0.0 {
            continue;
        }
        let add = (columns - 1) as f32 * col_pt;
        match widen.last_mut() {
            Some((r, last_add)) if r.end == at && (*last_add - add).abs() < 1e-4 => r.end = at + 1,
            _ => widen.push((at..at + 1, add)),
        }
    }
    widen
}

#[test]
fn mono_tab_widening_scans_spans_linearly_and_preserves_overlap_precedence() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);
    let scale = 2.0;

    // 1. Many disjoint mono spans on a tabbed line must not scan all spans per tab.
    let n = 600usize;
    let text = "A\t".repeat(n);
    let spans: Vec<Span> = (0..n).map(|i| Span::new(i * 2, i * 2 + 2).mono().size(12.0)).collect();
    let tabs = mono_tab_stops(&text, &spans, false);
    assert_eq!(tabs.len(), n);
    let _ = super::super::span_map::take_span_scan_steps();
    let got = ts.mono_tab_widening(&tabs, &spans, style, scale);
    let steps = super::super::span_map::take_span_scan_steps();
    assert_eq!(got.len(), n);
    assert!(
        steps <= (tabs.len() + spans.len()) * 6,
        "mono_tab_widening should be linear, got {steps} span steps for {} tabs and {} spans",
        tabs.len(),
        spans.len()
    );

    // 2. Overlapping and unsorted spans preserve later-wins precedence per attribute.
    let overlap_text = "a\tb\tc\td\te\t";
    let overlap_spans = vec![
        Span::new(0, overlap_text.len()).mono().size(10.0).weight(700.0),
        Span::new(6, 12).mono().size(20.0),
        Span::new(2, 10).mono().italic(),
        Span::new(0, 6).mono().weight(400.0),
    ];
    let overlap_tabs = mono_tab_stops(overlap_text, &overlap_spans, false);
    let expected = reference_mono_tab_widening(&mut ts, &overlap_tabs, &overlap_spans, style, scale);
    let actual = ts.mono_tab_widening(&overlap_tabs, &overlap_spans, style, scale);
    assert_eq!(actual, expected, "overlapping/unsorted spans must match brute-force tab widening");
}

#[test]
fn layout_rich_scans_spans_linearly_for_routed_and_spacing() {
    let mut ts = TextSystem::default();
    let style = TextStyle::new(14.0, 400.0);
    let scale = 2.0;

    // Alternating Latin + emoji with per-chunk spans exercises both routed mono checks and spacing scans.
    let n = 350usize;
    let chunk = "A🦀";
    let text = chunk.repeat(n);
    let stride = chunk.len();
    let spans: Vec<Span> = (0..n)
        .map(|i| {
            let s = Span::new(i * stride, (i + 1) * stride);
            if i % 2 == 0 { s.mono().size(12.0) } else { s.size(16.0) }
        })
        .collect();
    let _ = super::super::span_map::take_span_scan_steps();
    let layout = ts.layout_rich(&text, &spans, style, scale, None);
    let steps = super::super::span_map::take_span_scan_steps();
    let runs: usize = layout.lines().map(|l| l.runs().count()).sum();
    assert!(runs >= n, "expected at least {n} runs, got {runs}");
    assert!(
        steps <= (spans.len() + runs) * 16,
        "layout_rich should scan spans linearly, got {steps} steps for {} spans and {runs} runs",
        spans.len()
    );

    // Tabbed mono spans through full layout_rich (tab widening + spacing + inline fit) also stay linear.
    let tab_n = 600usize;
    let tab_text = "A\t".repeat(tab_n);
    let tab_spans: Vec<Span> = (0..tab_n).map(|i| Span::new(i * 2, i * 2 + 2).mono()).collect();
    let _ = super::super::span_map::take_span_scan_steps();
    let tab_layout = ts.layout_rich(&tab_text, &tab_spans, style, scale, None);
    let tab_steps = super::super::span_map::take_span_scan_steps();
    let tab_runs: usize = tab_layout.lines().map(|l| l.runs().count()).sum();
    assert!(
        tab_steps <= (tab_spans.len() + tab_runs) * 16,
        "tabbed layout_rich should scan spans linearly, got {tab_steps} steps for {} spans and {tab_runs} runs",
        tab_spans.len()
    );

    // Overlapping and unsorted spans lay out identically to their flattened disjoint equivalent.
    let mixed = "A🦀\tB🦀\tC";
    let raw_spans = vec![
        Span::new(0, mixed.len()).size(12.0).underline(Color::BLACK),
        Span::new(mixed.find('B').unwrap(), mixed.len()).mono().weight(700.0),
        Span::new(0, mixed.find('B').unwrap()).italic(),
    ];
    let flat_spans = flatten_spans(&raw_spans);
    let l_raw = ts.layout_rich(mixed, &raw_spans, style, scale, None);
    let l_flat = ts.layout_rich(mixed, &flat_spans, style, scale, None);
    assert!((l_raw.full_width() - l_flat.full_width()).abs() < 1e-3);
    assert!((l_raw.height() - l_flat.height()).abs() < 1e-3);
    let line_metrics = |l: &Layout<Ink>| -> Vec<(usize, f32, f32)> {
        l.lines()
            .flat_map(|line| line.runs().map(|r| (r.text_range().start, r.font_size(), r.advance())).collect::<Vec<_>>())
            .collect()
    };
    assert_eq!(line_metrics(&l_raw), line_metrics(&l_flat));
}
