use std::collections::BTreeMap;

use serde_json::Value;

use super::{PatchState, item_key};
use crate::runtime::web_a11y::{A11yFont, A11yItem, A11yMirror, A11yRun, round1};
use crate::{Cx, El, Env, Runtime, ScrollSpec};

/// Reference DOM-like model used to verify that applying incremental patches matches a fresh
/// full rebuild at every step (structure, attributes, relative rects, and transformed bounds).
#[derive(Clone, Debug, Default, PartialEq)]
struct MirrorModel {
    nodes: BTreeMap<String, Value>,
    children: BTreeMap<String, Vec<String>>,
}

impl MirrorModel {
    fn from_full(items: &[A11yItem]) -> Self {
        let mut fresh = PatchState::default();
        let mut model = Self::default();
        if let Some(patch) = fresh.diff(items) {
            model.apply_patch(&patch);
        }
        model
    }

    fn apply_patch(&mut self, patch: &Value) {
        assert_eq!(patch["p"], 1, "patch protocol marker");
        if let Some(removed) = patch.get("d").and_then(Value::as_array) {
            for k in removed {
                let key = k.as_str().expect("removed key string");
                self.nodes.remove(key);
                self.children.remove(key);
                for list in self.children.values_mut() {
                    list.retain(|c| c != key);
                }
            }
        }
        if let Some(upserts) = patch.get("u").and_then(Value::as_array) {
            for item in upserts {
                let key = patch_item_key(item);
                self.nodes.insert(key, item.clone());
            }
        }
        if let Some(orders) = patch.get("o").and_then(Value::as_object) {
            for (parent_key, kids_val) in orders {
                let kids: Vec<String> = kids_val.as_array().expect("child array")
                    .iter()
                    .map(|v| v.as_str().expect("child key").to_owned())
                    .collect();
                // Detach reparented children from any previous parent, matching DOM insertBefore.
                for kid in &kids {
                    for (other_parent, list) in &mut self.children {
                        if other_parent != parent_key {
                            list.retain(|c| c != kid);
                        }
                    }
                }
                self.children.insert(parent_key.clone(), kids);
            }
        }
        self.children.retain(|_, list| !list.is_empty());
    }

    /// Computes transformed screen-space bounds `[x, y, w, h]` for every node by walking from root
    /// and applying each scroll container's `sc: [tx, ty]` `translate3d(-tx, -ty, 0)`.
    fn screen_bounds(&self) -> BTreeMap<String, [f32; 4]> {
        let mut out = BTreeMap::new();
        self.walk_bounds("", [0.0, 0.0], &mut out);
        out
    }

    fn walk_bounds(&self, parent: &str, origin: [f32; 2], out: &mut BTreeMap<String, [f32; 4]>) {
        let Some(kids) = self.children.get(parent) else { return };
        for key in kids {
            let item = &self.nodes[key];
            let r = item["r"].as_array().unwrap();
            let (rx, ry, rw, rh) = (
                r[0].as_f64().unwrap() as f32,
                r[1].as_f64().unwrap() as f32,
                r[2].as_f64().unwrap() as f32,
                r[3].as_f64().unwrap() as f32,
            );
            let x = round1(origin[0] + rx);
            let y = round1(origin[1] + ry);
            out.insert(key.clone(), [x, y, rw, rh]);
            let mut next_origin = [x, y];
            if let Some(sc) = item.get("sc").and_then(Value::as_array) {
                next_origin[0] -= sc[0].as_f64().unwrap() as f32;
                next_origin[1] -= sc[1].as_f64().unwrap() as f32;
            }
            self.walk_bounds(key, next_origin, out);
        }
    }
}

fn patch_item_key(item: &Value) -> String {
    let kind = item["k"].as_str().unwrap();
    let path = item["p"].as_str().unwrap();
    let tag = match kind {
        "group" => item.get("g").and_then(Value::as_str).unwrap_or("div").to_owned(),
        "heading" => format!("h{}", item.get("l").and_then(Value::as_u64).unwrap_or(2)),
        "button" => "button".to_owned(),
        "link" => "a".to_owned(),
        "image" | "progressbar" | "scroll_x" => "div".to_owned(),
        _ => "p".to_owned(),
    };
    format!("{tag}:{path}")
}

#[test]
fn virtual_scroll_shift_keeps_staying_rows_unmutated_and_non_virtual_commits_stable() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let rows = crate::col().key("rows").w_full().growth(crate::el::GrowthSpec::default()).virtual_rows(
            200,
            40.0,
            |_: &(), i| std::borrow::Cow::Owned(format!("m{i}")),
            |_: &(), i| crate::text(format!("Message {i}")).w_full().h(40.0),
        );
        crate::col().w_full().h_full().child(
            crate::col().key("pane").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(rows),
        )
    }

    let mut rt = Runtime::new((), view, (400.0, 240.0), Env::new(false, 1.0));
    rt.frame(0.0);
    for _ in 0..8 { rt.frame(1.0 / 60.0); }

    let mut mirror = A11yMirror::default();
    let mut model = MirrorModel::default();
    let first: Value = serde_json::from_str(
        rt.core().web_a11y_patch(&mut mirror).expect("initial patch"),
    ).unwrap();
    model.apply_patch(&first);
    assert_eq!(model, MirrorModel::from_full(&rt.web_a11y_items()));
    assert!(rt.core().web_a11y_patch(&mut mirror).is_none(), "idle frame emits no patch");

    let initial_commits = rt.core().commit.commits;
    let initial_non_virtual = rt.core().commit.non_virtual_commits;

    // Scroll far enough to trigger CommitWhy::VIRTUAL window shift while keeping Message 8 mounted.
    rt.wheel(200.0, 120.0, -300.0);
    for _ in 0..10 { rt.frame(1.0 / 60.0); }

    assert!(rt.core().commit.commits > initial_commits, "virtual window shift committed");
    assert_eq!(
        rt.core().commit.non_virtual_commits,
        initial_non_virtual,
        "CommitWhy::VIRTUAL must not increment non_virtual_commits",
    );
    let ev = rt.core().web_a11y_frame_event(&mut mirror, crate::time::now(), false);
    assert!(
        matches!(ev, crate::runtime::web_a11y::schedule::ScheduleEvent::Frame { dirty: true, scrolling: true, non_virtual_commit: false, .. }),
        "virtual scroll shift frame must report per-frame scroll movement: {ev:?}",
    );

    // Flush once scrolling settles: staying rows must not be re-upserted.
    let settle: Value = serde_json::from_str(
        rt.core().web_a11y_patch(&mut mirror).expect("settle patch"),
    ).unwrap();
    let upserts = settle["u"].as_array().expect("settle patch has upserts");
    let scroll_upsert = upserts.iter().find(|it| it.get("sc").is_some()).expect("scroll container updated");
    assert!(scroll_upsert["sc"][1].as_f64().unwrap() > 100.0, "scroll container carries updated translate3d offset");
    assert!(
        !upserts.iter().any(|it| it.get("t").and_then(Value::as_str) == Some("Message 8")),
        "staying row Message 8 must not be upserted on scroll settle: {upserts:?}",
    );

    model.apply_patch(&settle);
    let full_after = rt.web_a11y_items();
    assert_eq!(model, MirrorModel::from_full(&full_after));

    // Verify staying row's effective screen-space top matches its drawn box.
    let bounds = model.screen_bounds();
    let row8_item = full_after.iter().find(|it| it.text == "Message 8").expect("Message 8 mounted");
    let row8_node = rt.core().tree.nodes.iter().position(|n| n.path == row8_item.path).unwrap();
    let drawn = rt.core().drawn_box(row8_node);
    let row8_screen = bounds[&item_key(row8_item)];
    assert!((row8_screen[1] - drawn[1]).abs() <= 0.2, "transformed top {} vs drawn {}", row8_screen[1], drawn[1]);
}

#[test]
fn sticky_header_counter_shifts_while_normal_rows_stay_unmutated() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let mut list = crate::col().key("list").w_full()
            .child(
                crate::row().key("hdr").w_full().h(30.0).sticky_top(0.0)
                    .child(crate::text("Section A").key("hdr-text").h(30.0)),
            );
        for i in 0..25 {
            list = list.child(crate::text(format!("Row {i}")).key(format!("r{i}")).w_full().h(40.0));
        }
        crate::col().w_full().h_full().child(
            crate::col().key("pane").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(list),
        )
    }

    let mut rt = Runtime::new((), view, (400.0, 200.0), Env::new(false, 1.0));
    rt.frame(0.0);

    let mut mirror = A11yMirror::default();
    let mut model = MirrorModel::default();
    let first: Value = serde_json::from_str(rt.core().web_a11y_patch(&mut mirror).unwrap()).unwrap();
    model.apply_patch(&first);

    // Scroll down 120 px so the sticky header sticks at top=0 while rows scroll up by 120 px.
    rt.update(|_, tx| tx.scroll_to_instant("pane", 0.0, 120.0));
    rt.frame(0.0);

    let patch: Value = serde_json::from_str(rt.core().web_a11y_patch(&mut mirror).expect("scroll patch")).unwrap();
    let upserts = patch["u"].as_array().expect("upserts");
    // Only the scroll container and the counter-shifted sticky header text should be upserted;
    // none of the 25 regular rows should be touched.
    assert_eq!(upserts.len(), 2, "expected only scroll container + sticky header upserted, got: {upserts:?}");
    assert!(upserts.iter().any(|it| it.get("sc").is_some()));
    assert!(upserts.iter().any(|it| it.get("t").and_then(Value::as_str) == Some("Section A")));

    model.apply_patch(&patch);
    let full = rt.web_a11y_items();
    assert_eq!(model, MirrorModel::from_full(&full));

    let bounds = model.screen_bounds();
    let hdr_key = item_key(full.iter().find(|it| it.text == "Section A").unwrap());
    let row5_key = item_key(full.iter().find(|it| it.text == "Row 5").unwrap());
    assert!((bounds[&hdr_key][1] - 0.0).abs() <= 0.2, "sticky header stays pinned at top=0: {:?}", bounds[&hdr_key]);
    // Row 5 starts at 30 (header) + 5*40 = 230, minus 120 scroll = 110.
    assert!((bounds[&row5_key][1] - 110.0).abs() <= 0.2, "row 5 screen top should be 110: {:?}", bounds[&row5_key]);
}

fn next_u64(seed: &mut u64) -> u64 {
    let mut x = *seed;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *seed = x;
    x
}

fn make_item(kind: &'static str, path: &str, rect: [f32; 4]) -> A11yItem {
    A11yItem {
        kind,
        path: path.to_owned(),
        rect,
        font_size: 0.0,
        level: None,
        expanded: None,
        text: String::new(),
        runs: Vec::new(),
        tag: None,
        role: None,
        parent: None,
        id: None,
        href: None,
        font: None,
        selectable: false,
        description: String::new(),
        value: String::new(),
        pressed: None,
        selected: None,
        checked: None,
        set: None,
        live: None,
        range: None,
        value_text: String::new(),
        disabled: false,
        busy: false,
        scroll: None,
        aria: Default::default(),
        dir: None,
    }
}

#[test]
fn differential_random_patch_sequences_match_full_rebuild() {
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut state = PatchState::default();
    let mut patched = MirrorModel::default();

    for step in 0..200usize {
        let mut items: Vec<A11yItem> = Vec::new();
        let zoom = [1.0f32, 1.25, 1.5][(next_u64(&mut seed) % 3) as usize];
        let pane_scroll_y = round1(((next_u64(&mut seed) % 50) as f32) * 12.5 * zoom);
        let pane_scroll_x = if next_u64(&mut seed) % 3 == 0 { round1(((next_u64(&mut seed) % 20) as f32) * 4.0 * zoom) } else { 0.0 };
        let pane_is_scroll = next_u64(&mut seed) % 5 != 0;

        // Optional transient wrapper container (tests reparenting out of a deleted parent).
        let has_box_a = next_u64(&mut seed) % 2 == 0;
        let box_a_idx = if has_box_a {
            let mut g = make_item("group", "#0/box-a", [round1(10.0 * zoom), round1(10.0 * zoom), 300.0, 80.0]);
            g.tag = Some(if next_u64(&mut seed) % 2 == 0 { "section" } else { "div" });
            g.role = (g.tag == Some("div")).then_some("group");
            g.text = "Box A".into();
            items.push(g);
            Some(items.len() - 1)
        } else {
            None
        };

        // Scrollable (or temporarily non-scrollable) virtual pane.
        let mut pane = make_item("group", "#0/pane", [0.0, round1(100.0 * zoom), round1(400.0 * zoom), round1(300.0 * zoom)]);
        pane.tag = Some("div");
        if pane_is_scroll {
            pane.scroll = Some([pane_scroll_x, pane_scroll_y]);
        }
        items.push(pane);
        let pane_idx = items.len() - 1;

        // Sticky header counter-shifted inside the pane.
        if next_u64(&mut seed) % 4 != 0 {
            let mut hdr = make_item(
                "heading",
                "#0/pane/sticky",
                [0.0, round1(100.0 * zoom + if pane_is_scroll { pane_scroll_y } else { 0.0 }), 300.0, 28.0],
            );
            hdr.level = Some(((next_u64(&mut seed) % 3) + 1) as u32);
            hdr.text = format!("Sticky {}", step % 4);
            hdr.parent = Some(pane_idx);
            items.push(hdr);
        }

        // Virtualized window of rows with randomized start/len and occasional reordering.
        let start = (next_u64(&mut seed) % 12) as usize;
        let len = (next_u64(&mut seed) % 8) as usize;
        let mut row_ids: Vec<usize> = (start..start + len).collect();
        if row_ids.len() >= 2 && next_u64(&mut seed) % 3 == 0 {
            let a = (next_u64(&mut seed) as usize) % row_ids.len();
            let b = (next_u64(&mut seed) as usize) % row_ids.len();
            row_ids.swap(a, b);
        }
        for r in row_ids {
            let mut row = make_item(
                "text",
                &format!("#0/pane/r{r}"),
                [round1(8.0 * zoom), round1((140.0 + r as f32 * 36.0) * zoom), round1(360.0 * zoom), round1(32.0 * zoom)],
            );
            row.parent = Some(pane_idx);
            row.font_size = round1(14.0 * zoom);
            row.font = Some(A11yFont {
                family: "sans".into(),
                weight: 400.0,
                line_height: round1(18.0 * zoom),
                tracking: 0.0,
                italic: false,
                align: "start",
            });
            if r % 3 == 0 {
                row.runs = vec![
                    A11yRun { text: format!("Row {r} "), href: None, path: None, link: None, description: None },
                    A11yRun {
                        text: "link".into(),
                        href: Some("https://example.com".into()),
                        path: Some(row.path.clone()),
                        link: Some(0),
                        description: (next_u64(&mut seed) % 2 == 0).then(|| "Open link".into()),
                    },
                ];
            } else {
                row.text = format!("Row {r} v{}", if next_u64(&mut seed) % 4 == 0 { step } else { 0 });
            }
            items.push(row);
        }

        // Morphing node that changes tag, kind, and parent across steps.
        let morph_parent = match (next_u64(&mut seed) % 3, box_a_idx) {
            (0, Some(idx)) => Some(idx),
            (1, _) => Some(pane_idx),
            _ => None,
        };
        let morph_rect = [round1(12.0 * zoom), round1(120.0 * zoom), 120.0, 28.0];
        let morph = match next_u64(&mut seed) % 5 {
            0 => {
                let mut it = make_item("button", "#0/morph", morph_rect);
                it.text = "Preset".into();
                it.role = Some("combobox");
                it.expanded = Some(next_u64(&mut seed) % 2 == 0);
                it.value = format!("Opt {}", next_u64(&mut seed) % 3);
                it
            }
            1 => {
                let mut it = make_item("heading", "#0/morph", morph_rect);
                it.level = Some(if next_u64(&mut seed) % 2 == 0 { 2 } else { 4 });
                it.text = "Title".into();
                it
            }
            2 => {
                let mut it = make_item("text", "#0/morph", morph_rect);
                it.role = Some("status");
                it.live = Some("polite");
                it.text = format!("Saved {}", step % 3);
                it
            }
            3 => {
                let mut it = make_item("progressbar", "#0/morph", morph_rect);
                it.role = Some("progressbar");
                it.text = "Upload".into();
                it.range = Some([0.0, 1.0, 0.5]);
                it.value_text = "50%".into();
                it
            }
            _ => {
                let mut it = make_item("image", "#0/morph", morph_rect);
                it.text = "Logo".into();
                it
            }
        };
        let mut morph = morph;
        morph.parent = morph_parent;
        items.push(morph);

        if let Some(patch) = state.diff(&items) {
            patched.apply_patch(&patch);
        }
        let full = MirrorModel::from_full(&items);
        assert_eq!(patched, full, "step {step}: patched model diverged from full rebuild");
        assert_eq!(
            patched.screen_bounds(),
            full.screen_bounds(),
            "step {step}: transformed screen bounds diverged from full rebuild",
        );
    }
}
