//! The "Text/Positioned PDF" playground story: a generated two-column PDF-style page bitmap
//! (`Pixels`) beneath a transparent `rune::positioned_text` layer with drag selection,
//! double-click word selection, copy, caret browsing, and zero-rebuild `El::live_scale` zoom.

use rune::tok::{self, radius, type_};
use rune::{
    Color, Cx, El, Link, Live, Pixels, PositionedText, Story, TextHighlight, Tx, col, el, oklch,
    positioned_text, row, text,
};

const PAGE_W: u32 = 480;
const PAGE_H: u32 = 300;

pub(crate) struct PositionedPdfDemo {
    pub(crate) bitmap: Pixels,
    pub(crate) page: PositionedText,
    pub(crate) zoom: Live<f32>,
    pub(crate) zoom_label: f32,
    pub(crate) selected: String,
    pub(crate) link_clicked: String,
}

impl Default for PositionedPdfDemo {
    fn default() -> Self {
        let (bitmap, page) = generate_two_column_page();
        Self {
            bitmap,
            page,
            zoom: Live::new(1.0),
            zoom_label: 1.0,
            selected: String::new(),
            link_clicked: String::new(),
        }
    }
}

fn fill_rect(rgba: &mut [u8], x0: f32, y0: f32, w: f32, h: f32, col: [u8; 4]) {
    let ix0 = (x0.round() as i32).clamp(0, PAGE_W as i32) as u32;
    let iy0 = (y0.round() as i32).clamp(0, PAGE_H as i32) as u32;
    let ix1 = ((x0 + w).round() as i32).clamp(0, PAGE_W as i32) as u32;
    let iy1 = ((y0 + h).round() as i32).clamp(0, PAGE_H as i32) as u32;
    for y in iy0..iy1 {
        for x in ix0..ix1 {
            let off = ((y * PAGE_W + x) * 4) as usize;
            rgba[off..off + 4].copy_from_slice(&col);
        }
    }
}

fn push_line(
    rgba: &mut [u8],
    content: &mut String,
    rects: &mut Vec<[f32; 4]>,
    line: &str,
    origin: [f32; 2],
    cell: [f32; 2],
    ink: [u8; 4],
) {
    let ([mut cx, y], [cw, ch]) = (origin, cell);
    for ch_val in line.chars() {
        content.push(ch_val);
        rects.push([cx, y, cw, ch]);
        if !ch_val.is_whitespace() {
            let inset = if ch_val.is_uppercase() { 1.0 } else { 3.0 };
            fill_rect(rgba, cx + 0.5, y + inset, (cw - 1.5).max(1.0), (ch - inset - 2.0).max(2.0), ink);
        }
        cx += cw;
    }
    content.push('\n');
    rects.push([0.0; 4]);
}

/// Generates a crisp two-column page bitmap plus matching 1:1 `PositionedText` character boxes.
fn generate_two_column_page() -> (Pixels, PositionedText) {
    let mut rgba = vec![248u8; (PAGE_W * PAGE_H * 4) as usize];
    for p in rgba.as_chunks_mut::<4>().0 {
        p[0] = 250; p[1] = 249; p[2] = 246; p[3] = 255;
    }

    // Header rule and vertical column divider.
    fill_rect(&mut rgba, 24.0, 44.0, 432.0, 1.0, [205, 208, 214, 255]);
    fill_rect(&mut rgba, 239.0, 56.0, 1.0, 214.0, [224, 226, 232, 255]);

    let mut content = String::new();
    let mut rects = Vec::new();
    let ink = [52, 58, 68, 255];
    let link_ink = [30, 110, 210, 255];

    push_line(&mut rgba, &mut content, &mut rects, "RUNE PDF LAYER - TWO-COLUMN SPECIMEN", [24.0, 20.0], [7.0, 14.0], [36, 40, 48, 255]);

    // Column 1 (x = 24..224).
    push_line(&mut rgba, &mut content, &mut rects, "Column one shows selectable", [24.0, 62.0], [6.5, 12.0], ink);
    push_line(&mut rgba, &mut content, &mut rects, "glyph boxes over a bitmap.", [24.0, 80.0], [6.5, 12.0], ink);
    push_line(&mut rgba, &mut content, &mut rects, "Short end.", [24.0, 98.0], [6.5, 12.0], ink);
    // Paragraph gap in Column 1 (y = 110..146) opposite active Column 2 lines.
    push_line(&mut rgba, &mut content, &mut rects, "Second paragraph after gap", [24.0, 146.0], [6.5, 12.0], ink);
    // Ligature `"fi"` sharing one identical rect.
    let mut cx = 24.0f32;
    for ch_val in "Fine ".chars() {
        content.push(ch_val); rects.push([cx, 164.0, 6.5, 12.0]);
        if !ch_val.is_whitespace() { fill_rect(&mut rgba, cx + 0.5, 167.0, 5.0, 7.0, ink); }
        cx += 6.5;
    }
    let lig_rect = [cx, 164.0, 11.0, 12.0];
    for ch_val in "fi".chars() { content.push(ch_val); rects.push(lig_rect); }
    fill_rect(&mut rgba, cx + 0.5, 166.0, 9.5, 8.0, link_ink);
    cx += 11.0;
    for ch_val in "lter & zoom.".chars() {
        content.push(ch_val); rects.push([cx, 164.0, 6.5, 12.0]);
        if !ch_val.is_whitespace() { fill_rect(&mut rgba, cx + 0.5, 167.0, 5.0, 7.0, ink); }
        cx += 6.5;
    }
    content.push('\n'); rects.push([0.0; 4]);

    // Column 2 (x = 254..456).
    push_line(&mut rgba, &mut content, &mut rects, "Column two stays separate", [254.0, 62.0], [6.5, 12.0], ink);
    push_line(&mut rgba, &mut content, &mut rects, "because lines split only on", [254.0, 80.0], [6.5, 12.0], ink);
    push_line(&mut rgba, &mut content, &mut rects, "explicit newline markers.", [254.0, 98.0], [6.5, 12.0], ink);
    push_line(&mut rgba, &mut content, &mut rects, "Line beside col-1 para gap", [254.0, 122.0], [6.5, 12.0], ink);
    push_line(&mut rgba, &mut content, &mut rects, "See https://rune.dev docs.", [254.0, 146.0], [6.5, 12.0], link_ink);

    let pixels = Pixels::new();
    pixels.set(PAGE_W, PAGE_H, rgba);
    let page = PositionedText::new(content, rects).view_box(PAGE_W as f32, PAGE_H as f32);
    (pixels, page)
}

pub(crate) fn story() -> Story<PositionedPdfDemo> {
    Story::new("Text/Positioned PDF", PositionedPdfDemo::default, view)
        .trigger('z', "Cycle zoom (1x / 1.25x / 1.5x)", |s, _| {
            s.zoom_label = if s.zoom_label < 1.1 { 1.25 } else if s.zoom_label < 1.4 { 1.5 } else { 1.0 };
            s.zoom.set(s.zoom_label);
        })
        .variant("100% zoom", |_| {})
        .variant("125% live zoom", |s| { s.zoom_label = 1.25; s.zoom.set(1.25); })
}

pub(crate) fn view(s: &PositionedPdfDemo, cx: &mut Cx<PositionedPdfDemo>) -> El {
    let t = tok::theme(cx.system_dark());
    let accent = tok::accent(0, cx.system_dark());
    let sel_bg = oklch(0.65, 0.16, 250.0).alpha(0.35);
    let url_start = s.page.text().find("https://rune.dev").unwrap_or(0);
    let url_end = url_start + "https://rune.dev".len();

    let zoom_btn = |key: &'static str, label: &'static str, z: f32| {
        let on = (s.zoom_label - z).abs() < 0.05;
        row().key(key).h(26.0).px(10.0).center().radius(radius::SM)
            .fill(if on { accent } else { t.control_track })
            .hover(if on { accent } else { t.hover })
            .keeps_text_selection()
            .child(text(label).font(type_::LABEL).color(if on { Color::WHITE } else { t.text }))
            .on_press_tx(move |s: &mut PositionedPdfDemo, tx: &mut Tx| {
                s.zoom.set(z);
                tx.live_only();
            })
    };

    let status = if !s.selected.is_empty() {
        format!("Selected: {:?}", s.selected.chars().take(48).collect::<String>())
    } else if !s.link_clicked.is_empty() {
        format!("Clicked link: {}", s.link_clicked)
    } else {
        "Drag across columns, double-click a word, Cmd/Ctrl+C to copy, or zoom without rebuilds.".to_owned()
    };

    col().gap(10.0).px(12.0).py(12.0)
        .child(
            row().gap(6.0).center_y()
                .child(zoom_btn("z100", "100%", 1.0))
                .child(zoom_btn("z125", "125%", 1.25))
                .child(zoom_btn("z150", "150%", 1.5))
                .child(text(status).key("pdf-status").font(type_::LABEL).color(t.text_secondary).pl(8.0)),
        )
        .child(
            col().gap(8.0).selection_group(true).caret_browsing(true)
                .on_selection_change(|s: &mut PositionedPdfDemo, ch, _| {
                    s.selected = ch.selection().map_or_else(String::new, |sel| sel.text().to_owned());
                })
                .child(
                    el().key("pdf-viewport").size(PAGE_W as f32, PAGE_H as f32).clip_soft(0.0).radius(radius::MD).border(1.0, t.card_border).child(
                        el().key("pdf-page").size(PAGE_W as f32, PAGE_H as f32).origin(0.0, 0.0).live_scale(&s.zoom)
                            .pixels(&s.bitmap)
                            .child(
                                positioned_text(&s.page)
                                    .key("pdf-layer")
                                    .w_full()
                                    .h_full()
                                    .selection_color(sel_bg)
                                    .text_highlight(TextHighlight::new(0..4, oklch(0.9, 0.14, 95.0).alpha(0.45)))
                                    .link(Link::new(url_start..url_end, "https://rune.dev"))
                                    .on_link(|s: &mut PositionedPdfDemo, id, _| s.link_clicked = id.to_owned()),
                            ),
                    ),
                )
                .child(text("Caption: ordinary selectable text sharing the same selection_group.").key("pdf-caption").font(type_::LABEL).color(t.text_secondary).selectable(true)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn positioned_pdf_story_selects_and_live_zooms_without_rebuilds() {
        let mut rt = Runtime::new(PositionedPdfDemo::default(), view, (640.0, 420.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let page_rect = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("pdf-layer")).unwrap().rect;
        let (x0, y0) = (page_rect[0] + 25.0, page_rect[1] + 68.0);
        rt.pointer_move(x0, y0);
        rt.pointer_down(x0, y0);
        rt.frame(0.01);
        rt.pointer_move(x0 + 62.0, y0);
        rt.frame(0.01);
        rt.pointer_up(x0 + 62.0, y0);
        rt.frame(0.01);
        assert!(rt.selected_text().unwrap().starts_with("Column one"));

        // Live zoom button uses tx.live_only(): verified by live_only_events and zero debug warnings.
        let before_live_only = rt.live_only_events();
        let btn = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("z125")).unwrap().rect;
        let (bx, by) = (btn[0] + btn[2] * 0.5, btn[1] + btn[3] * 0.5);
        rt.pointer_move(bx, by);
        rt.pointer_down(bx, by);
        rt.frame(0.01);
        rt.pointer_up(bx, by);
        rt.frame(0.01);
        assert_eq!(rt.live_only_events(), before_live_only + 1);
        assert!(rt.live_only_warnings().is_empty(), "{:?}", rt.live_only_warnings());
        assert!((rt.state.zoom.get() - 1.25).abs() < 1e-4);
        assert!(rt.selected_text().unwrap().starts_with("Column one"), "zoom button keeps selection");
    }
}
