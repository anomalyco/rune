//! Minimal native Android Rune app demonstrating `rune::android_main!`, edge-to-edge
//! `Cx::safe_area()`, separate `Cx::keyboard_inset()`, dark mode, and Back-to-Escape focus blur.
//!
//! Build an APK with `scripts/android-run.sh --build-only`.

#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]

use rune::tok::{self, motion, type_};
use rune::{App, AppSpec, Cx, El, TextEdit, col, el, text, text_input};

struct State {
    on: bool,
    note: TextEdit,
}

impl Default for State {
    fn default() -> Self {
        Self {
            on: false,
            note: TextEdit::new(""),
        }
    }
}

fn view(s: &State, cx: &mut Cx<State>) -> El {
    let dark = cx.system_dark();
    let t = tok::theme(dark);
    let [top, right, bottom, left] = cx.safe_area();
    let bottom_pad = bottom.max(cx.keyboard_inset());
    let label = text(if s.on { "Lights on" } else { "Lights off" })
        .font(type_::TITLE)
        .color(t.text);
    let knob = el()
        .key("knob")
        .size(22.0, 22.0)
        .radius(11.0)
        .fill(t.knob)
        .absolute(if s.on { 24.0 } else { 3.0 }, 3.0);
    let switch = el()
        .key("switch")
        .role("switch")
        .label("Lights")
        .checked(s.on)
        .focusable(true)
        .size(49.0, 28.0)
        .radius(14.0)
        .fill(if s.on { tok::accent(0, dark) } else { t.control_off })
        .motion(motion::SNAPPY)
        .on_press(|s: &mut State| s.on = !s.on)
        .child(knob);
    let field = text_input(&s.note, type_::BODY)
        .key("note")
        .label("Note")
        .placeholder("Type a note", t.text_secondary)
        .color(t.text)
        .w(240.0)
        .h(44.0)
        .padding(12.0)
        .radius(10.0)
        .fill(t.card)
        .on_edit(|s: &mut State| &mut s.note);
    col()
        .w_full()
        .h_full()
        .pt(16.0 + top)
        .pr(16.0 + right)
        .pb(16.0 + bottom_pad)
        .pl(16.0 + left)
        .center()
        .gap(16.0)
        .fill(t.bg)
        .child(label)
        .child(switch)
        .child(field)
}

pub fn start() {
    App::new(AppSpec::new("Hello Android", (360.0, 800.0), State::default, view)).run();
}

rune::android_main!(start);

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Key, KeyEvent, Mods, Runtime};

    #[test]
    fn android_hello_renders_separates_keyboard_inset_and_blurs_on_escape() {
        let mut rt = Runtime::new(State::default(), view, (360.0, 800.0), Env::new(true, 3.0));
        rt.set_safe_area([32.0, 0.0, 24.0, 0.0]);
        rt.set_keyboard_inset(280.0);
        rt.frame(0.0);

        let (sx, sy) = rt.find("switch").expect("switch present");
        rt.pointer_down(sx, sy);
        rt.pointer_up(sx, sy);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.on);

        // Focus the text input, then verify Escape (mapped from Android Back) blurs focus.
        let (fx, fy) = rt.find("note").expect("note field present");
        rt.pointer_down(fx, fy);
        rt.pointer_up(fx, fy);
        rt.frame(1.0 / 60.0);
        assert!(rt.has_focus());

        rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
        rt.frame(1.0 / 60.0);
        assert!(!rt.has_focus(), "Back mapped to Escape blurs focused text input");
    }

    #[test]
    fn android_hello_manifest_hides_action_bar() {
        let manifest = include_str!("../AndroidManifest.xml");
        assert!(
            manifest.contains(r#"android:theme="@android:style/Theme.DeviceDefault.NoActionBar""#),
            "android-hello manifest must set a NoActionBar theme so no system title bar covers Rune",
        );
    }
}
