//! A window that draws its own titlebar (`App::titlebar`): a 44 point band that drags the
//! window, with a button in it that keeps its presses. macOS keeps its traffic lights at the
//! leading edge; Windows draws the caption buttons at the trailing edge, with snap layouts
//! over maximize. `Cx::chrome_insets` says what each takes. Press "Theme" to switch the app's
//! colours away from the system's: the caption buttons follow through `Cx::caption_color`.
use rune::*;

const HEIGHT: f32 = 44.0;

struct State { dark: Option<bool>, presses: u32 }

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let dark = s.dark.unwrap_or(cx.system_dark());
    let (bg, band, ink, faint) = if dark {
        (oklch(0.18, 0.0, 0.0), oklch(0.22, 0.0, 0.0), oklch(0.95, 0.0, 0.0), oklch(0.65, 0.0, 0.0))
    } else {
        (oklch(0.99, 0.0, 0.0), oklch(0.95, 0.0, 0.0), oklch(0.2, 0.0, 0.0), oklch(0.5, 0.0, 0.0))
    };
    cx.caption_color(if dark { Color::WHITE } else { Color::BLACK });
    let insets = cx.chrome_insets();
    let titlebar = row().key("titlebar").w_full().h(insets.height.max(HEIGHT)).fill(band).center_y().window_drag()
        .pl(insets.leading + 12.0).pr(insets.trailing + 12.0).gap(12.0)
        .child(text("Custom titlebar").color(ink))
        .child(el().grow(1.0))
        .child(row().key("theme").px(10.0).h(28.0).radius(6.0).center_y().fill(faint.alpha(0.2))
            .child(text("Theme").color(ink))
            .on_press(|s: &mut State| { s.dark = Some(!s.dark.unwrap_or(true)); s.presses += 1; }));
    let line = |t: String| text(t).color(faint);
    col().key("root").w_full().h_full().fill(bg)
        .child(titlebar)
        .child(col().padding(24.0).gap(8.0)
            .child(text("Drag the band to move the window; double-click it to maximize; right-click it for the window menu.").color(ink).wrap())
            .child(line(format!("chrome insets: leading {:.0}, trailing {:.0}, height {:.0}", insets.leading, insets.trailing, insets.height)))
            .child(line(format!("scale {:.2}, theme presses {}", cx.env().scale, s.presses))))
}

fn main() {
    App::new(AppSpec::new("Custom titlebar", (720.0, 420.0), || State { dark: None, presses: 0 }, view))
        .titlebar(Titlebar::new(HEIGHT))
        .run();
}
