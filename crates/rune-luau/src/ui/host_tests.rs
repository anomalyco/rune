//! An app-defined node that lists its props (`HostNode::props`): a prop outside the list is an
//! unknown prop (the render fails in debug builds, a warning in release); a listed one is not.

use std::cell::RefCell;
use std::rc::Rc;

use crate::{HostNode, LogLevel, Script, ScriptOptions, Sources};

/// Renders `ui.probe(<props>)` from a script whose `probe` node lists only `text`, and returns
/// what the script logged.
fn render_probe(props: &str) -> Vec<(LogLevel, String)> {
    let logged = Rc::new(RefCell::new(Vec::new()));
    let sink = logged.clone();
    let node = HostNode::new("probe", |args| Ok(rune::text(args.string("text").unwrap_or("")))).props(|name| name == "text");
    let options = ScriptOptions::new().node(node).on_log(move |level, message| sink.borrow_mut().push((level, message.to_owned())));
    let mut sources = Sources::new();
    sources.insert("init.luau", format!("local ui = require(\"@rune/ui\")\nlocal M = {{}}\nfunction M.render(view)\n    return ui.probe({props})\nend\nreturn M\n"));
    let script = Script::with_options("probe", sources, options);
    assert!(script.error().is_none(), "start: {:?}", script.error());
    let _ = script.render_uncached::<()>("render", false);
    logged.take()
}

#[test]
fn an_unlisted_prop_on_an_app_defined_node_is_an_error_in_debug_and_a_warning_in_release() {
    let logged = render_probe(r#"{ text = "a", autofocus = true }"#);
    let level = if cfg!(debug_assertions) { LogLevel::Error } else { LogLevel::Warn };
    assert!(
        logged.iter().any(|(l, m)| *l == level && m.contains("ui.probe: unknown prop `autofocus`")),
        "expected a {level:?} naming the prop, got {logged:?}"
    );
}

#[test]
fn a_listed_prop_on_an_app_defined_node_is_not_unknown() {
    let logged = render_probe(r#"{ text = "a", key = "probe" }"#);
    assert!(!logged.iter().any(|(l, m)| *l == LogLevel::Error || m.contains("unknown prop")), "got {logged:?}");
}
