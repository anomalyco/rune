//! What the component tests share: one app state every scene draws from, a headless runtime,
//! queries over `Runtime::accessibility_json`, assistive actions, and the tree goldens.

use std::path::PathBuf;

use rune::accesskit::Action;
use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, TextEdit};
use rune_kit::diff::DiffState;
use rune_kit::shell::PaneState;
use rune_kit::{ComboboxState, FileNode, Kit, ShortcutRecorderState, TableState, TreeModel};
use serde_json::Value;

/// The state every scene reads and its handlers write.
pub struct App {
    pub presses: u32,
    pub on: bool,
    pub open: bool,
    pub picked: Option<usize>,
    pub chosen: Option<String>,
    pub edit: TextEdit,
    pub combo: ComboboxState,
    pub recorder: ShortcutRecorderState,
    pub files: TreeModel<FileNode<(u32, u32)>>,
    pub table: TableState,
    pub sidebar: PaneState,
    pub diff: DiffState,
    pub level: f32,
}

impl Default for App {
    fn default() -> App {
        App {
            presses: 0,
            on: false,
            open: false,
            picked: None,
            chosen: None,
            edit: TextEdit::default(),
            combo: ComboboxState::default(),
            recorder: ShortcutRecorderState::default(),
            files: TreeModel::new(),
            table: TableState::new(),
            sidebar: PaneState::new(240.0),
            diff: DiffState::new(Vec::new()),
            level: 0.4,
        }
    }
}

pub fn kit() -> Kit {
    Kit::neutral(false)
}

pub type View = fn(&App, &mut Cx<App>) -> El;

/// `view` over `app`, laid out and settled.
pub fn mount(app: App, view: View) -> Runtime<App> {
    let mut rt = Runtime::new(app, view, (720.0, 720.0), Env::new(false, 1.0));
    settle(&mut rt);
    rt
}

pub fn settle(rt: &mut Runtime<App>) {
    rt.frame(0.0);
    rt.settle_all();
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
}

/// The node keyed `key`; panics with the whole tree when there is none.
pub fn node<'a>(tree: &'a Value, key: &str) -> &'a Value {
    find(tree, key).unwrap_or_else(|| panic!("no node keyed {key:?} in {}", serde_json::to_string_pretty(tree).unwrap()))
}

pub fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    if v["key"] == key {
        return Some(v);
    }
    v["children"].as_array()?.iter().find_map(|c| find(c, key))
}

/// Every node with `role`, in reading order.
pub fn with_role<'a>(v: &'a Value, role: &str) -> Vec<&'a Value> {
    let mut out = Vec::new();
    walk(v, &mut |n| if n["role"] == role { out.push(n) });
    out
}

/// Every node named `name`.
pub fn named<'a>(v: &'a Value, name: &str) -> Vec<&'a Value> {
    let mut out = Vec::new();
    walk(v, &mut |n| if n["name"] == name { out.push(n) });
    out
}

fn walk<'a>(v: &'a Value, f: &mut dyn FnMut(&'a Value)) {
    f(v);
    for c in v["children"].as_array().into_iter().flatten() {
        walk(c, f);
    }
}

pub fn states(v: &Value) -> Vec<&str> {
    v["states"].as_array().map_or(Vec::new(), |s| s.iter().filter_map(Value::as_str).collect())
}

pub fn actions(v: &Value) -> Vec<&str> {
    v["actions"].as_array().map_or(Vec::new(), |s| s.iter().filter_map(Value::as_str).collect())
}

pub fn has(v: &Value, state: &str) -> bool {
    states(v).contains(&state)
}

pub fn can(v: &Value, action: &str) -> bool {
    actions(v).contains(&action)
}

/// Role and name of `v`.
pub fn said(v: &Value) -> (&str, &str) {
    (v["role"].as_str().unwrap_or(""), v["name"].as_str().unwrap_or(""))
}

/// What a screen reader asks of the node keyed `key`, run as its frame would run it.
pub fn act(rt: &mut Runtime<App>, key: &str, action: Action) -> bool {
    let ok = rt.accessibility_action(key, action, None);
    settle(rt);
    ok
}

/// A pointer press and release at the middle of the node keyed `key`, where the tree says it is.
pub fn tap(rt: &mut Runtime<App>, key: &str) {
    let tree = rt.accessibility_json();
    let b = node(&tree, key)["bounds"].as_array().map(|b| b.iter().map(|v| v.as_f64().unwrap() as f32).collect::<Vec<_>>()).unwrap();
    let (x, y) = (b[0] + b[2] / 2.0, b[1] + b[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

pub fn press_key(rt: &mut Runtime<App>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    settle(rt);
}

/// The key of the node holding keyboard focus.
pub fn focused(tree: &Value) -> Option<String> {
    let mut out = None;
    walk(tree, &mut |n| if has(n, "focused") { out = n["key"].as_str().map(str::to_owned) });
    out
}

/// No text in the tree, at any depth, says `text` twice in a row: what a control's label
/// already says isn't read again from the words inside it.
pub fn no_echo(v: &Value) {
    let name = v["name"].as_str().unwrap_or("");
    for c in v["children"].as_array().into_iter().flatten() {
        if !name.is_empty() && c["role"] == "label" {
            assert_ne!(c["name"].as_str(), Some(name), "{} {name:?} reads its label twice", v["role"]);
        }
        no_echo(c);
    }
}

/// Compares `tree` without bounds or scroll offsets (layout, not semantics) or shortcuts (written
/// as each platform writes them) with the golden
/// `tests/a11y/golden/<name>.json`. `RUNE_BLESS=1` writes it instead; review the diff.
pub fn golden(name: &str, tree: &Value) {
    let mut tree = tree.clone();
    strip(&mut tree);
    let actual = serde_json::to_string_pretty(&tree).unwrap() + "\n";
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/a11y/golden").join(format!("{name}.json"));
    if std::env::var_os("RUNE_BLESS").is_some_and(|v| v != "0") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing golden {name}; run with RUNE_BLESS=1"));
    let want = want.replace("\r\n", "\n");
    if want != actual {
        let fresh = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("a11y-{name}.json"));
        std::fs::write(&fresh, &actual).unwrap();
        let line = want.lines().zip(actual.lines()).position(|(a, b)| a != b).unwrap_or(want.lines().count().min(actual.lines().count()));
        panic!(
            "accessibility golden differs: {name} (first difference at line {})\n  golden: {}\n  fresh:  {}\n  accept with RUNE_BLESS=1 after review",
            line + 1,
            path.display(),
            fresh.display(),
        );
    }
}

fn strip(v: &mut Value) {
    if let Some(o) = v.as_object_mut() {
        for k in ["bounds", "scroll", "selection", "shortcut"] {
            o.remove(k);
        }
        if let Some(Value::Array(c)) = o.get_mut("children") {
            c.iter_mut().for_each(strip);
        }
    }
}
