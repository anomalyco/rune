//! `Script::try_render_fn`: one render's element or error, without the error card.

use mlua::Function;

use crate::{Script, Sources};

const SRC: &str = r#"
local ui = require("@rune/ui")
return {
    render = function()
        return ui.text("main")
    end,
    good = function(view, label)
        return ui.text(label)
    end,
    bad = function(view)
        error("boom")
    end,
}
"#;

fn export(script: &Script, name: &str) -> Function {
    script.module().expect("running").get::<Function>(name).expect("exported")
}

/// A render that succeeds returns its element; one that throws returns the error, logged,
/// instead of the error card.
#[test]
fn try_render_fn_returns_element_or_error() {
    let script = Script::new("try_render", Sources::single(SRC));
    assert!(script.error().is_none(), "start: {:?}", script.error());

    let good = export(&script, "good");
    assert!(script.try_render_fn::<()>(false, &good, "hello").is_ok());

    let bad = export(&script, "bad");
    let err = script.try_render_fn::<()>(false, &bad, ()).err().expect("render threw");
    assert!(err.message.contains("boom"), "message: {}", err.message);
    assert!(script.log().iter().any(|l| l.message.contains("boom")), "logged");
}

/// A script that failed to start returns its stored error, the one `Script::error` gives,
/// without entering any VM.
#[test]
fn try_render_fn_returns_stored_failure() {
    let good = Script::new("try_render_good", Sources::single(SRC));
    let f = export(&good, "good");

    let failed = Script::new("try_render_failed", Sources::single(r#"error("start failed")"#));
    let stored = failed.error().expect("start failed");
    let err = failed.try_render_fn::<()>(false, &f, "hello").err().expect("not running");
    assert_eq!(err, stored);
    assert_eq!(failed.metrics().renders, 0, "no render ran");
}
