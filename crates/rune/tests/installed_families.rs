//! Headless `Runtime` tests for `rune::text::installed_families()` and
//! `rune::text::InstalledFamily`: re-export, caching, and switching slots via
//! `Tx::set_font_families` to an installed system family.

use rune::text::{InstalledFamily, installed_families};
use rune::{Direction, El, Env, Runtime, TextStyle, col, row, text};

const STYLE: TextStyle = TextStyle::new(14.0, 400.0);

fn view(_: &(), _: &mut rune::Cx<()>) -> El {
    col().children([
        row().child(text("iiiiiiiiii 0123456789").key("sans").font(STYLE)),
        row().child(text("iiiiiiiiii 0123456789").key("mono").font(STYLE.mono())),
        row().child(text("שלום iiiiiiiiii").key("rtl").dir(Direction::Rtl).font(STYLE)),
    ])
}

fn width_of(rt: &Runtime<()>, key: &str) -> f32 {
    rt.elements()
        .iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing element {key}"))
        .target[2]
}

fn quote_css(name: &str) -> String {
    format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
}

#[test]
fn rune_text_installed_families_round_trip_with_tx_set_font_families() {
    let families: &'static [InstalledFamily] = installed_families();
    assert_eq!(families.as_ptr(), installed_families().as_ptr());

    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    {
        #[cfg(any(windows, target_os = "macos"))]
        assert!(!families.is_empty());
        let Some(mono) = families.iter().find(|f| f.monospace()) else { return };
        let Some(prop) = families.iter().find(|f| !f.monospace()) else { return };

        let mut rt = Runtime::new((), view, (600.0, 300.0), Env::new(true, 2.0));
        rt.frame(0.0);
        assert!(width_of(&rt, "sans") > 0.0);
        assert!(width_of(&rt, "mono") > 0.0);
        assert!(width_of(&rt, "rtl") > 0.0);

        let sans_css = format!("{}, Inter", quote_css(prop.name()));
        let mono_css = quote_css(mono.name());
        let stale = rt.update_if(|_, tx| {
            tx.set_font_families(sans_css.clone(), mono_css.clone());
            false
        });
        assert!(stale, "switching to installed system families must request a frame");
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.font_families(), (sans_css.as_str(), mono_css.as_str()));
        assert!(width_of(&rt, "sans") > 0.0);
        assert!(width_of(&rt, "mono") > 0.0);
        assert!(width_of(&rt, "rtl") > 0.0);
    }
}
