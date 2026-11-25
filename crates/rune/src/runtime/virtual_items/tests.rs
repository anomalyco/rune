use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashMap as StdMap;
use std::rc::Rc;
use std::sync::Arc;

use crate::{
    Color, Cx, El, Env, GrowthSpec, Key, KeyEvent, Live, LiveText, Mods, Runtime, ScrollSpec,
    TextEdit, TextStyle, VirtualItems, col, el, memo, row, text, text_input,
};

struct CanvasSource {
    ver: Cell<u64>,
    keys: RefCell<Vec<String>>,
    budget: Cell<Option<usize>>,
    priorities: RefCell<StdMap<String, f32>>,
    built_order: RefCell<Vec<String>>,
    lives: RefCell<StdMap<String, Live<f32>>>,
    live_texts: RefCell<StdMap<String, LiveText>>,
    use_memo: bool,
    with_live_text: bool,
    nested_key: RefCell<Option<String>>,
}

impl CanvasSource {
    fn new(keys: Vec<String>) -> Rc<Self> {
        Rc::new(Self {
            ver: Cell::new(1),
            keys: RefCell::new(keys),
            budget: Cell::new(None),
            priorities: RefCell::new(StdMap::new()),
            built_order: RefCell::new(Vec::new()),
            lives: RefCell::new(StdMap::new()),
            live_texts: RefCell::new(StdMap::new()),
            use_memo: false,
            with_live_text: false,
            nested_key: RefCell::new(None),
        })
    }

    fn set_keys(&self, keys: Vec<String>) {
        *self.keys.borrow_mut() = keys;
        self.ver.set(self.ver.get() + 1);
    }

    fn take_built(&self) -> Vec<String> {
        std::mem::take(&mut *self.built_order.borrow_mut())
    }

    fn live_for(&self, key: &str) -> Live<f32> {
        self.lives
            .borrow_mut()
            .entry(key.to_owned())
            .or_insert_with(|| Live::new(1.0))
            .clone()
    }

    fn live_text_for(&self, key: &str) -> LiveText {
        self.live_texts
            .borrow_mut()
            .entry(key.to_owned())
            .or_insert_with(|| LiveText::new(format!("val-{key}")))
            .clone()
    }
}

impl VirtualItems for CanvasSource {
    fn version(&self) -> u64 {
        self.ver.get()
    }

    fn keys(&self) -> Vec<String> {
        self.keys.borrow().clone()
    }

    fn build(&self, key: &str) -> El {
        self.built_order.borrow_mut().push(key.to_owned());
        if self.nested_key.borrow().as_deref() == Some(key) {
            let inner = CanvasSource::new(vec!["inner-0".into()]);
            return el().size(80.0, 40.0).absolute(0.0, 0.0).virtual_items(inner);
        }
        let idx: usize = key.trim_start_matches('n').parse().unwrap_or(0);
        let x = (idx % 10) as f32 * 90.0;
        let y = (idx / 10) as f32 * 50.0;
        let op = self.live_for(key);
        let content = if self.with_live_text {
            let lt = self.live_text_for(key);
            text("").live_text(&lt).font(TextStyle::new(12.0, 400.0))
        } else if self.use_memo {
            let k = key.to_owned();
            memo(format!("m-{k}"), 1u32, move || text(k.clone()).font(TextStyle::new(12.0, 400.0)))
        } else {
            text(key).font(TextStyle::new(12.0, 400.0))
        };
        col()
            .size(80.0, 40.0)
            .absolute(x, y)
            .fill(Color { r: 0.2, g: 0.3, b: 0.4, a: 1.0 })
            .live_opacity(&op)
            .role("button")
            .label(format!("Node {key}"))
            .focusable(true)
            .child(content)
    }

    fn mount_budget(&self) -> Option<usize> {
        self.budget.get()
    }

    fn mount_priority(&self, key: &str) -> f32 {
        self.priorities.borrow().get(key).copied().unwrap_or(0.0)
    }
}

fn mounted_child_keys<S: 'static>(rt: &Runtime<S>, container_key: &str) -> Vec<String> {
    let nodes = &rt.core().tree.nodes;
    let Some(ci) = nodes.iter().position(|n| n.key.as_deref() == Some(container_key)) else {
        return Vec::new();
    };
    nodes
        .iter()
        .skip(ci + 1)
        .take_while(|n| n.parent.is_some_and(|p| p >= ci))
        .filter(|n| n.parent == Some(ci))
        .filter_map(|n| n.key.clone())
        .collect()
}

fn settle<S: 'static>(rt: &mut Runtime<S>) {
    for _ in 0..80 {
        if !rt.frame(1.0 / 60.0) && rt.wake_deadline().is_none() {
            return;
        }
    }
    panic!("runtime did not settle idle");
}

#[test]
fn window_change_remounts_without_running_view_or_rebuilding_staying_items() {
    thread_local! { static VIEWS: Cell<usize> = const { Cell::new(0) }; }
    struct App {
        cam: Live<[f32; 2]>,
        src: Rc<CanvasSource>,
    }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        VIEWS.set(VIEWS.get() + 1);
        el().key("canvas")
            .w_full()
            .h_full()
            .clip_soft(0.0)
            .live_offset(&s.cam)
            .virtual_items(s.src.clone())
    }

    let src = CanvasSource {
        use_memo: true,
        ..Rc::try_unwrap(CanvasSource::new(vec!["n0".into(), "n1".into(), "n2".into()])).ok().unwrap()
    };
    src.ver.set(1);
    let src = Rc::new(src);
    VIEWS.set(0);
    let mut rt = Runtime::new(
        App { cam: Live::new([0.0, 0.0]), src: src.clone() },
        view,
        (600.0, 400.0),
        Env::new(true, 1.0),
    );
    settle(&mut rt);
    assert_eq!(VIEWS.get(), 1);
    assert_eq!(mounted_child_keys(&rt, "canvas"), vec!["n0", "n1", "n2"]);
    assert_eq!(src.take_built(), vec!["n0", "n1", "n2"]);

    let n1_before = rt.core().tree.nodes.iter().find(|n| n.key.as_deref() == Some("n1")).unwrap();
    let (n1_id, n1_epoch) = (n1_before.id, n1_before.epoch);
    let sub_n1 = rt.core().frame.live_subscription(src.live_for("n1").source.id()).expect("n1 subscribed");
    let sub_n0_id = src.live_for("n0").source.id();
    assert!(rt.core().frame.live_subscription(sub_n0_id).is_some());
    let memo_builds_before = rt.core().commit.memo_builds();

    // Shift window: n0 leaves, n1 & n2 stay, n3 enters.
    rt.state.cam.set([-100.0, 0.0]);
    src.set_keys(vec!["n1".into(), "n2".into(), "n3".into()]);
    assert!(rt.core().virtual_window_changed());
    let still_anim = rt.frame(1.0 / 60.0);
    assert!(!still_anim, "unbudgeted remount settles in one frame");
    assert_eq!(rt.wake_deadline(), None);

    // Proof 1: view did not run!
    assert_eq!(VIEWS.get(), 1, "window change must not run view");
    assert_eq!(mounted_child_keys(&rt, "canvas"), vec!["n1", "n2", "n3"]);

    // Proof 2: staying items were not rebuilt, kept NodeId/epoch/memo and exact Live subscription!
    assert_eq!(src.take_built(), vec!["n3"], "only entering item n3 calls build");
    let n1_after = rt.core().tree.nodes.iter().find(|n| n.key.as_deref() == Some("n1")).unwrap();
    assert_eq!(n1_after.id, n1_id, "staying item keeps NodeId");
    assert_eq!(n1_after.epoch, n1_epoch, "staying item keeps PaintKeep epoch");
    assert_eq!(
        rt.core().commit.memo_builds(),
        memo_builds_before + 1,
        "only entering item n3 ran its memo closure"
    );
    assert!(
        Arc::ptr_eq(&rt.core().frame.live_subscription(src.live_for("n1").source.id()).unwrap(), &sub_n1),
        "staying item's Live value must not be re-subscribed"
    );
    // Leaving item n0 unmounted immediately (no exit retention, Live subscription dropped).
    assert!(rt.core().tree.nodes.iter().all(|n| n.key.as_deref() != Some("n0")));
    assert!(rt.core().frame.live_subscription(sub_n0_id).is_none());
    // Reorder staying items (n2 before n1): zero builds, order updates.
    src.set_keys(vec!["n2".into(), "n1".into(), "n3".into()]);
    assert!(!rt.frame(1.0 / 60.0));
    assert_eq!(VIEWS.get(), 1);
    assert!(src.take_built().is_empty(), "reordering staying items calls build 0 times");
    assert_eq!(mounted_child_keys(&rt, "canvas"), vec!["n2", "n1", "n3"]);
}

#[test]
fn live_text_inside_virtual_items_stays_on_remount_and_avoids_reshaping_unchanged() {
    thread_local! { static VIEWS: Cell<usize> = const { Cell::new(0) }; }
    struct App {
        src: Rc<CanvasSource>,
    }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        VIEWS.set(VIEWS.get() + 1);
        el().key("canvas").w_full().h_full().virtual_items(s.src.clone())
    }

    let mut inner = Rc::try_unwrap(CanvasSource::new(vec!["n0".into(), "n1".into()])).ok().unwrap();
    inner.with_live_text = true;
    let src = Rc::new(inner);
    VIEWS.set(0);
    let mut rt = Runtime::new(App { src: src.clone() }, view, (600.0, 400.0), Env::new(true, 1.0));
    settle(&mut rt);
    assert_eq!(VIEWS.get(), 1);
    let sub_lt0 = rt.core().frame.live_subscription(src.live_text_for("n0").source.id()).expect("lt0 subscribed");

    // Update n0's LiveText in place without changing keys -> 0 view runs.
    src.live_text_for("n0").set("updated-0");
    rt.frame(1.0 / 60.0);
    assert_eq!(VIEWS.get(), 1);

    // Drop n1 while n0 stays -> Remount, 0 view runs, 0 reshapes of n0's unchanged LiveText!
    src.set_keys(vec!["n0".into()]);
    rt.frame(1.0 / 60.0);
    assert_eq!(VIEWS.get(), 1, "LiveText inside virtual_items stays on CommitKind::Remount");
    assert_eq!(rt.stats.shaped, 0, "unchanged staying LiveText must not be reshaped on remount");
    assert!(
        Arc::ptr_eq(&rt.core().frame.live_subscription(src.live_text_for("n0").source.id()).unwrap(), &sub_lt0),
        "staying LiveText subscription preserved"
    );
}

#[test]
fn focused_item_and_focused_text_input_stay_pinned_until_blur() {
    thread_local! { static VIEWS: Cell<usize> = const { Cell::new(0) }; }

    // Part A: focusable(true) item pinned across omission, unmounts on blur via Remount.
    struct FocusApp {
        src: Rc<CanvasSource>,
    }
    fn focus_view(s: &FocusApp, _: &mut Cx<FocusApp>) -> El {
        VIEWS.set(VIEWS.get() + 1);
        col().w_full().h_full()
            .child(el().key("outside").size(60.0, 24.0).focusable(true).role("button").label("Outside"))
            .child(el().key("canvas").w_full().grow(1.0).virtual_items(s.src.clone()))
    }

    let src = CanvasSource::new(vec!["n0".into(), "n1".into()]);
    VIEWS.set(0);
    let mut rt = Runtime::new(FocusApp { src: src.clone() }, focus_view, (600.0, 400.0), Env::new(true, 1.0));
    settle(&mut rt);
    assert_eq!(VIEWS.get(), 1);

    // Focus n0 via pointer click (n0 is at (0, 0) inside /canvas, below outside's 24 pt height).
    let n0_rect = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("n0")).unwrap().rect;
    rt.pointer_down(n0_rect[0] + 10.0, n0_rect[1] + 10.0);
    rt.pointer_up(n0_rect[0] + 10.0, n0_rect[1] + 10.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.core().kbd.focus.path(), Some("#0/canvas/n0"));
    let views_before = VIEWS.get();

    // Pan away: omit n0 from keys(), add n2. n0 MUST stay pinned and mounted!
    src.set_keys(vec!["n1".into(), "n2".into()]);
    src.take_built();
    assert!(!rt.frame(1.0 / 60.0));
    assert_eq!(VIEWS.get(), views_before, "pinning remount runs view 0 times");
    assert_eq!(src.take_built(), vec!["n2"], "pinned n0 stays mounted without calling build");
    assert_eq!(mounted_child_keys(&rt, "canvas"), vec!["n1", "n2", "n0"]);
    assert_eq!(rt.core().kbd.focus.path(), Some("#0/canvas/n0"));

    // Blur by clicking empty canvas space: focus clears and n0 unmounts immediately.
    rt.pointer_down(500.0, 350.0);
    rt.pointer_up(500.0, 350.0);
    settle(&mut rt);
    assert_eq!(rt.core().kbd.focus.path(), None);
    assert_eq!(mounted_child_keys(&rt, "canvas"), vec!["n1", "n2"]);

    // Also verify clicking another focusable element outside unmounts a pinned item.
    let n1_rect = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("n1")).unwrap().rect;
    rt.pointer_down(n1_rect[0] + 10.0, n1_rect[1] + 10.0);
    rt.pointer_up(n1_rect[0] + 10.0, n1_rect[1] + 10.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.core().kbd.focus.path(), Some("#0/canvas/n1"));
    src.set_keys(vec!["n2".into()]);
    rt.frame(1.0 / 60.0);
    assert_eq!(mounted_child_keys(&rt, "canvas"), vec!["n2", "n1"]);
    let out_rect = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("outside")).unwrap().rect;
    rt.pointer_down(out_rect[0] + 5.0, out_rect[1] + 5.0);
    rt.pointer_up(out_rect[0] + 5.0, out_rect[1] + 5.0);
    settle(&mut rt);
    assert_eq!(mounted_child_keys(&rt, "canvas"), vec!["n2"]);

    // Part B: text_input inside a virtual item stays pinned and accepts typing when omitted from keys().
    struct EditSource {
        ver: Cell<u64>,
        keys: RefCell<Vec<String>>,
        edits: RefCell<[TextEdit; 2]>,
    }
    impl VirtualItems for EditSource {
        fn version(&self) -> u64 { self.ver.get() }
        fn keys(&self) -> Vec<String> { self.keys.borrow().clone() }
        fn build(&self, key: &str) -> El {
            let idx = usize::from(key != "e0");
            let x = if idx == 0 { 0.0 } else { 150.0 };
            let edits = self.edits.borrow();
            col().size(140.0, 32.0).absolute(x, 0.0).child(
                text_input(&edits[idx], TextStyle::new(13.0, 400.0))
                    .key("field")
                    .w(120.0)
                    .h(24.0)
                    .on_edit(move |s: &mut EditApp| &mut s.edits[idx]),
            )
        }
    }
    struct EditApp {
        edits: [TextEdit; 2],
        src: Rc<EditSource>,
    }
    fn edit_view(s: &EditApp, _: &mut Cx<EditApp>) -> El {
        *s.src.edits.borrow_mut() = s.edits.clone();
        el().key("canvas").w_full().h_full().virtual_items(s.src.clone())
    }
    let esrc = Rc::new(EditSource {
        ver: Cell::new(1),
        keys: RefCell::new(vec!["e0".into(), "e1".into()]),
        edits: RefCell::new([TextEdit::new("hi"), TextEdit::new("other")]),
    });
    let mut ert = Runtime::new(
        EditApp { edits: [TextEdit::new("hi"), TextEdit::new("other")], src: esrc.clone() },
        edit_view,
        (400.0, 200.0),
        Env::new(true, 1.0),
    );
    settle(&mut ert);
    // Focus e0's text_input.
    let f_rect = ert.elements().into_iter().find(|e| e.key.as_deref() == Some("field")).unwrap().rect;
    ert.pointer_down(f_rect[0] + 5.0, f_rect[1] + 5.0);
    ert.pointer_up(f_rect[0] + 5.0, f_rect[1] + 5.0);
    ert.frame(1.0 / 60.0);
    assert_eq!(ert.core().kbd.focus.path(), Some("canvas/e0/field"));

    // Omit e0 from keys() -> e0 stays mounted and keeps receiving typed input!
    *esrc.keys.borrow_mut() = vec!["e1".into()];
    esrc.ver.set(2);
    ert.frame(1.0 / 60.0);
    assert_eq!(mounted_child_keys(&ert, "canvas"), vec!["e1", "e0"]);
    ert.key(KeyEvent { key: Key::Text("!".into()), mods: Mods::default() });
    ert.frame(1.0 / 60.0);
    assert!(ert.state.edits[0].text.contains('!'), "focused pinned text_input still accepts typing: {:?}", ert.state.edits[0].text);
}

#[test]
fn mount_budget_and_priority_mount_nearest_first_render_in_keys_order_and_settle_idle() {
    thread_local! { static VIEWS: Cell<usize> = const { Cell::new(0) }; }
    struct App {
        src: Rc<CanvasSource>,
    }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        VIEWS.set(VIEWS.get() + 1);
        el().key("canvas").w_full().h_full().virtual_items(s.src.clone())
    }

    let src = CanvasSource::new(Vec::new());
    src.budget.set(Some(4));
    VIEWS.set(0);
    let mut rt = Runtime::new(App { src: src.clone() }, view, (600.0, 400.0), Env::new(true, 1.0));
    settle(&mut rt);
    assert_eq!(VIEWS.get(), 1);
    assert!(mounted_child_keys(&rt, "canvas").is_empty());

    // 10 items in keys() order n0..n9, with explicit priorities including ties and non-finite.
    let keys: Vec<String> = (0..10).map(|i| format!("n{i}")).collect();
    {
        let mut p = src.priorities.borrow_mut();
        p.insert("n0".into(), f32::NAN);
        p.insert("n1".into(), 5.0);
        p.insert("n2".into(), 1.0);
        p.insert("n3".into(), f32::INFINITY);
        p.insert("n4".into(), 2.0);
        p.insert("n5".into(), 1.0); // tie with n2 -> n2 before n5 (keys order)
        p.insert("n6".into(), 0.5); // smallest -> mounts first!
        p.insert("n7".into(), f32::NEG_INFINITY); // non-finite -> sorts last!
        p.insert("n8".into(), 3.0);
        p.insert("n9".into(), 4.0);
    }
    src.set_keys(keys.clone());

    // Frame 1: mounts at most 4 (n6, n2, n5, n4), rendered in keys() order (n2, n4, n5, n6).
    let more = rt.frame(1.0 / 60.0);
    assert!(more, "more budgeted items remain");
    assert_eq!(VIEWS.get(), 1, "budgeted remount must not run view");
    assert_eq!(src.take_built(), vec!["n2", "n4", "n5", "n6"]);
    assert_eq!(mounted_child_keys(&rt, "canvas"), vec!["n2", "n4", "n5", "n6"]);

    // Frame 2: mounts next 4 (n8, n9, n1, n0 — n0 is first non-finite by keys order).
    let more = rt.frame(1.0 / 60.0);
    assert!(more, "2 items still remain");
    assert_eq!(VIEWS.get(), 1);
    let built2 = src.take_built();
    assert_eq!(built2.len(), 4, "never exceeds budget of 4: {built2:?}");
    assert_eq!(built2, vec!["n0", "n1", "n8", "n9"]);
    assert_eq!(
        mounted_child_keys(&rt, "canvas"),
        vec!["n0", "n1", "n2", "n4", "n5", "n6", "n8", "n9"],
        "mounted children always stay in keys() back-to-front order"
    );

    // Frame 3: mounts final 2 non-finite items (n3, n7) and settles idle!
    let more = rt.frame(1.0 / 60.0);
    assert!(!more, "all items mounted; must reach idle");
    assert_eq!(rt.wake_deadline(), None);
    assert_eq!(VIEWS.get(), 1);
    assert_eq!(src.take_built(), vec!["n3", "n7"]);
    assert_eq!(mounted_child_keys(&rt, "canvas"), keys);

    // Some(0) budget clamps to 1 and never livelocks.
    src.budget.set(Some(0));
    src.set_keys(vec!["n10".into(), "n11".into()]);
    assert!(rt.frame(1.0 / 60.0));
    assert_eq!(mounted_child_keys(&rt, "canvas").len(), 1);
    assert!(!rt.frame(1.0 / 60.0));
    assert_eq!(mounted_child_keys(&rt, "canvas"), vec!["n10", "n11"]);
}

#[test]
fn duplicate_keys_are_deduplicated_with_warnings_and_no_synthetic_build() {
    struct App {
        src: Rc<CanvasSource>,
    }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        el().key("canvas").w_full().h_full().virtual_items(s.src.clone())
    }
    let src = CanvasSource::new(vec!["n0".into(), "n1".into(), "n0".into(), "n1".into()]);
    let mut rt = Runtime::new(App { src: src.clone() }, view, (400.0, 300.0), Env::new(true, 1.0));
    rt.core_mut().commit.enable_key_warnings();
    settle(&mut rt);
    assert_eq!(mounted_child_keys(&rt, "canvas"), vec!["n0", "n1"]);
    assert_eq!(src.take_built(), vec!["n0", "n1"], "no synthetic key passed to build");
    let warnings = rt.core().commit.key_warnings().unwrap();
    assert_eq!(warnings.len(), 2, "two duplicate key warnings recorded: {warnings:?}");
    assert!(warnings[0].contains("duplicate virtual item key"));
}

#[test]
fn accessibility_tree_reflects_mounted_and_pinned_items_without_list_set_size() {
    struct App {
        src: Rc<CanvasSource>,
    }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        el().key("canvas")
            .role("list")
            .label("Graph canvas")
            .w_full()
            .h_full()
            .virtual_items(s.src.clone())
    }
    let src = CanvasSource::new(vec!["n0".into(), "n1".into()]);
    let mut rt = Runtime::new(App { src: src.clone() }, view, (600.0, 400.0), Env::new(true, 1.0));
    settle(&mut rt);

    fn find_by_key<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
        if v["key"] == key {
            return Some(v);
        }
        v["children"].as_array()?.iter().find_map(|c| find_by_key(c, key))
    }

    let json = rt.accessibility_json();
    let canvas = find_by_key(&json, "canvas").expect("canvas node in a11y JSON");
    assert!(canvas["setSize"].is_null(), "virtual_items must not auto-assign size_of_set: {canvas}");
    let n0 = find_by_key(canvas, "n0").expect("n0 mounted in a11y");
    assert_eq!(n0["role"], "button");
    assert_eq!(n0["name"], "Node n0");
    assert!(n0["position"].is_null(), "virtual_items must not auto-assign position_in_set: {n0}");
    assert!(find_by_key(canvas, "n1").is_some());
    assert!(find_by_key(canvas, "n2").is_none());

    // Focus n0, then pan away to [n1, n2]: a11y tree has n1, n2 AND pinned focused n0!
    let n0_rect = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("n0")).unwrap().rect;
    rt.pointer_down(n0_rect[0] + 10.0, n0_rect[1] + 10.0);
    rt.pointer_up(n0_rect[0] + 10.0, n0_rect[1] + 10.0);
    rt.frame(1.0 / 60.0);

    src.set_keys(vec!["n1".into(), "n2".into()]);
    rt.frame(1.0 / 60.0);

    let json2 = rt.accessibility_json();
    let canvas2 = find_by_key(&json2, "canvas").unwrap();
    assert!(find_by_key(canvas2, "n0").is_some(), "pinned focused item stays in a11y tree");
    assert!(find_by_key(canvas2, "n1").is_some());
    assert!(find_by_key(canvas2, "n2").is_some());

    // Also verify AccessKit tree reflects the mounted buttons with no auto set_size.
    let ak = rt.accessibility_tree();
    let names: Vec<&str> = ak.nodes.iter().filter_map(|(_, n)| n.label()).collect();
    assert!(names.contains(&"Node n0") && names.contains(&"Node n1") && names.contains(&"Node n2"));
}

#[test]
fn coexists_with_virtual_rows_handles_nested_fallback_and_prunes_on_unmount() {
    thread_local! { static VIEWS: Cell<usize> = const { Cell::new(0) }; }
    struct App {
        show_canvas: bool,
        src: Rc<CanvasSource>,
    }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        VIEWS.set(VIEWS.get() + 1);
        let sidebar = col()
            .key("scroll")
            .w(150.0)
            .h_full()
            .scroll(ScrollSpec { stick_bottom: false })
            .child(
                col()
                    .key("rows")
                    .w_full()
                    .growth(GrowthSpec::default())
                    .virtual_rows(
                        200,
                        20.0,
                        |_: &App, i| Cow::Owned(format!("r{i}")),
                        |_: &App, i| text(format!("row {i}")).h(20.0),
                    ),
            );
        let mut r = row().w_full().h_full().child(sidebar);
        if s.show_canvas {
            r = r.child(el().key("canvas").grow(1.0).h_full().virtual_items(s.src.clone()));
        }
        r
    }

    let src = CanvasSource::new(vec!["n0".into(), "n1".into()]);
    VIEWS.set(0);
    let mut rt = Runtime::new(
        App { show_canvas: true, src: src.clone() },
        view,
        (600.0, 300.0),
        Env::new(true, 1.0),
    );
    settle(&mut rt);
    let views0 = VIEWS.get();

    // Window shift on canvas alongside virtual_rows stays on CommitKind::Remount!
    src.set_keys(vec!["n1".into(), "n2".into()]);
    settle(&mut rt);
    assert_eq!(VIEWS.get(), views0, "coexistence with virtual_rows stays on Remount");
    assert_eq!(mounted_child_keys(&rt, "canvas"), vec!["n1", "n2"]);

    // Entering item with a nested virtual_items triggers Fallback and still renders cleanly.
    *src.nested_key.borrow_mut() = Some("n3".into());
    src.set_keys(vec!["n1".into(), "n3".into()]);
    settle(&mut rt);
    assert!(VIEWS.get() > views0, "nested virtual container fell back to Rebuild");
    assert_eq!(mounted_child_keys(&rt, "canvas"), vec!["n1", "n3"]);

    // Unmounting the virtual_items container prunes it cleanly.
    rt.update(|s, _| s.show_canvas = false);
    settle(&mut rt);
    assert!(rt.core().commit.virtual_items.is_empty(), "unmounted virtual_items container pruned");
}

#[test]
fn unused_virtual_items_has_zero_impact() {
    fn plain(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().h_full().child(text("hello"))
    }
    let mut rt = Runtime::new((), plain, (300.0, 200.0), Env::new(true, 1.0));
    settle(&mut rt);
    assert!(rt.core().commit.virtual_items.is_empty());
    assert!(!rt.core().virtual_window_changed());
    assert!(!rt.frame(1.0 / 60.0));
    assert_eq!(rt.wake_deadline(), None);
}

// Paired --profile perf benchmark lives in crates/rune/examples/virtual_items_bench.rs.
