//! Host-side unit tests for Android per-role `screenReaderFocusable` and `heading` grouping.

use accesskit::{Action, Affine, Node, NodeId, Rect, Role, TreeId, TreeInfo, TreeUpdate};

use crate::el::{El, ScrollSpec, col, row, text};
use crate::runtime::{Cx, Env, Runtime};

use super::focusable::{
    AndroidNodeProps, AndroidTreeIndex, FLAG_HEADING, FLAG_SCREEN_READER_FOCUSABLE, NodeQuery,
    classify_tree,
};

fn node(role: Role) -> Node {
    Node::new(role)
}

fn label_node(value: &str, bounds: Rect) -> Node {
    let mut n = Node::new(Role::Label);
    n.set_value(value);
    n.set_bounds(bounds);
    n
}

fn tree(root: NodeId, focus: NodeId, nodes: Vec<(NodeId, Node)>) -> TreeUpdate {
    TreeUpdate {
        nodes,
        tree: Some(TreeInfo::new(root)),
        tree_id: TreeId::ROOT,
        focus,
    }
}

fn srf() -> AndroidNodeProps {
    AndroidNodeProps {
        screen_reader_focusable: true,
        heading: false,
    }
}

fn heading_srf() -> AndroidNodeProps {
    AndroidNodeProps {
        screen_reader_focusable: true,
        heading: true,
    }
}

fn passive() -> AndroidNodeProps {
    AndroidNodeProps::default()
}

fn runtime_tree(view: fn(&(), &mut Cx<()>) -> El) -> TreeUpdate {
    let mut rt = Runtime::new((), view, (400.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.accessibility_tree()
}

fn id_by_key(update: &TreeUpdate, key: &str) -> NodeId {
    update
        .nodes
        .iter()
        .find(|(_, n)| n.author_id() == Some(key))
        .map(|(id, _)| *id)
        .unwrap_or_else(|| panic!("missing node with key {key:?}"))
}

#[test]
fn standalone_labels_in_scroll_view_are_screen_reader_focusable_and_root_is_passive() {
    let mut root = node(Role::Window);
    root.set_label("Gallery");
    root.set_transform(Affine::scale(2.0));
    root.set_bounds(Rect::new(0.0, 0.0, 360.0, 800.0));
    root.set_children(vec![NodeId(2), NodeId(3)]);

    let mut scroll = node(Role::ScrollView);
    scroll.set_bounds(Rect::new(0.0, 0.0, 360.0, 800.0));
    scroll.set_transform(Affine::translate((0.0, -50.0)));
    scroll.set_children(vec![NodeId(4)]);

    let mut announcer = node(Role::Status);
    announcer.set_label("Saved");
    announcer.set_bounds(Rect::new(0.0, 0.0, 1.0, 1.0));

    let mut content = node(Role::Group);
    content.set_bounds(Rect::new(0.0, 0.0, 360.0, 1200.0));
    content.set_children(vec![NodeId(5), NodeId(6), NodeId(7)]);

    let l1 = label_node("Welcome", Rect::new(16.0, 100.0, 200.0, 124.0));
    let mut btn = node(Role::Button);
    btn.set_label("Continue");
    btn.add_action(Action::Click);
    btn.add_action(Action::Focus);
    btn.set_bounds(Rect::new(16.0, 140.0, 140.0, 180.0));
    let empty_label = label_node("   ", Rect::new(16.0, 200.0, 100.0, 220.0));

    let update = tree(
        NodeId(1),
        NodeId(1),
        vec![
            (NodeId(1), root),
            (NodeId(2), scroll),
            (NodeId(3), announcer),
            (NodeId(4), content),
            (NodeId(5), l1),
            (NodeId(6), btn),
            (NodeId(7), empty_label),
        ],
    );

    let mut index = AndroidTreeIndex::default();
    index.apply_update(&update);
    assert!(index.focus_is_root());
    assert_eq!(index.props_for_node(NodeId(1)), passive(), "root Window must be passive");
    assert_eq!(index.props_for_node(NodeId(2)), passive(), "ScrollView must stay passive");
    assert_eq!(index.props_for_node(NodeId(3)), passive(), "announcer Status must stay passive");
    assert_eq!(index.props_for_node(NodeId(4)), passive(), "content Group must stay passive");
    assert_eq!(index.props_for_node(NodeId(5)), srf(), "standalone Label must be screenReaderFocusable");
    assert_eq!(index.props_for_node(NodeId(6)), passive(), "Button is already actionable");
    assert_eq!(index.props_for_node(NodeId(7)), passive(), "blank Label must not create empty stop");

    // Bounds lookup accounts for root scale (2.0) and ScrollView translate (0, -50):
    // y0 = (100 - 50) * 2 = 100, y1 = (124 - 50) * 2 = 148, x0 = 32, x1 = 400.
    assert_eq!(index.indexed_bounds(NodeId(5)), Some([32, 100, 400, 148]));
    let flags = index.flags_for_query(&NodeQuery {
        class_name: "android.widget.TextView",
        bounds_px: Some([32, 100, 400, 148]),
        content_description: Some("Welcome"),
        text: None,
        child_count: 0,
        focusable: false,
        clickable: false,
    });
    assert_eq!(flags, FLAG_SCREEN_READER_FOCUSABLE);
}

#[test]
fn list_item_article_heading_and_paragraph_group_plain_text_as_one_stop() {
    let mut root = node(Role::Window);
    root.set_children(vec![NodeId(10), NodeId(20), NodeId(30), NodeId(40), NodeId(50)]);

    // 1. ListItem with two plain text leaves -> ONE stop on ListItem.
    let mut li = node(Role::ListItem);
    li.set_label("Wi-Fi Connected");
    li.set_bounds(Rect::new(0.0, 0.0, 300.0, 48.0));
    li.set_children(vec![NodeId(11), NodeId(12)]);
    let li_t1 = label_node("Wi-Fi", Rect::new(16.0, 4.0, 120.0, 22.0));
    let li_t2 = label_node("Connected", Rect::new(16.0, 24.0, 120.0, 42.0));

    // 2. Article with title + body labels -> ONE stop on Article.
    let mut article = node(Role::Article);
    article.set_bounds(Rect::new(0.0, 60.0, 300.0, 140.0));
    article.set_children(vec![NodeId(21), NodeId(22)]);
    let art_t1 = label_node("Release notes", Rect::new(16.0, 64.0, 200.0, 88.0));
    let art_t2 = label_node("TalkBack grouping landed.", Rect::new(16.0, 92.0, 280.0, 120.0));

    // 3. Heading wrapping a child Label -> ONE stop on Heading with isHeading = true.
    let mut heading = node(Role::Heading);
    heading.set_label("Settings");
    heading.set_bounds(Rect::new(0.0, 150.0, 300.0, 180.0));
    heading.set_children(vec![NodeId(31)]);
    let head_t = label_node("Settings", Rect::new(16.0, 150.0, 120.0, 180.0));

    // 4. Direct-text Heading leaf -> ONE stop with isHeading = true.
    let mut leaf_heading = node(Role::Heading);
    leaf_heading.set_label("Account");
    leaf_heading.set_bounds(Rect::new(0.0, 190.0, 300.0, 220.0));

    // 5. Paragraph with two inline text leaves -> ONE stop on Paragraph.
    let mut para = node(Role::Paragraph);
    para.set_bounds(Rect::new(0.0, 230.0, 300.0, 280.0));
    para.set_children(vec![NodeId(51), NodeId(52)]);
    let p_t1 = label_node("First sentence.", Rect::new(16.0, 230.0, 150.0, 250.0));
    let p_t2 = label_node("Second sentence.", Rect::new(16.0, 255.0, 200.0, 275.0));

    let update = tree(
        NodeId(1),
        NodeId(1),
        vec![
            (NodeId(1), root),
            (NodeId(10), li),
            (NodeId(11), li_t1),
            (NodeId(12), li_t2),
            (NodeId(20), article),
            (NodeId(21), art_t1),
            (NodeId(22), art_t2),
            (NodeId(30), heading),
            (NodeId(31), head_t),
            (NodeId(40), leaf_heading),
            (NodeId(50), para),
            (NodeId(51), p_t1),
            (NodeId(52), p_t2),
        ],
    );

    let mut index = AndroidTreeIndex::default();
    index.apply_update(&update);

    assert_eq!(index.props_for_node(NodeId(10)), srf());
    assert_eq!(index.props_for_node(NodeId(11)), passive());
    assert_eq!(index.props_for_node(NodeId(12)), passive());

    assert_eq!(index.props_for_node(NodeId(20)), srf());
    assert_eq!(index.props_for_node(NodeId(21)), passive());
    assert_eq!(index.props_for_node(NodeId(22)), passive());

    assert_eq!(index.props_for_node(NodeId(30)), heading_srf());
    assert_eq!(index.props_for_node(NodeId(31)), passive());

    assert_eq!(index.props_for_node(NodeId(40)), heading_srf());

    assert_eq!(index.props_for_node(NodeId(50)), srf());
    assert_eq!(index.props_for_node(NodeId(51)), passive());
    assert_eq!(index.props_for_node(NodeId(52)), passive());

    // Query lookup returns both FLAG_SCREEN_READER_FOCUSABLE and FLAG_HEADING for Heading,
    // and 0 for the grouped Label inside it.
    assert_eq!(
        index.flags_for_query(&NodeQuery {
            class_name: "android.view.View",
            bounds_px: Some([0, 150, 300, 180]),
            content_description: Some("Settings"),
            text: None,
            child_count: 1,
            focusable: false,
            clickable: false,
        }),
        FLAG_SCREEN_READER_FOCUSABLE | FLAG_HEADING,
    );
    assert_eq!(
        index.flags_for_query(&NodeQuery {
            class_name: "android.widget.TextView",
            bounds_px: Some([16, 150, 120, 180]),
            content_description: Some("Settings"),
            text: None,
            child_count: 0,
            focusable: false,
            clickable: false,
        }),
        0,
    );
}

#[test]
fn interactive_and_nested_grouping_cases_avoid_duplicate_or_split_stops() {
    let mut root = node(Role::Window);
    root.set_children(vec![NodeId(10), NodeId(20), NodeId(30), NodeId(40), NodeId(50)]);

    // 1. Clickable ListItem with two Labels: ListItem is natively actionable, Labels stay unsplit.
    let mut clickable_li = node(Role::ListItem);
    clickable_li.set_label("Inbox 3");
    clickable_li.add_action(Action::Click);
    clickable_li.add_action(Action::Focus);
    clickable_li.set_children(vec![NodeId(11), NodeId(12)]);
    let cli_t1 = label_node("Inbox", Rect::new(0.0, 0.0, 50.0, 20.0));
    let cli_t2 = label_node("3", Rect::new(60.0, 0.0, 80.0, 20.0));

    // 2. Non-clickable ListItem wrapping ONLY a Button("Open") whose content-derived label matches:
    //    must not add a redundant stop before the Button.
    let mut btn_only_li = node(Role::ListItem);
    btn_only_li.set_label("Open");
    btn_only_li.set_children(vec![NodeId(21)]);
    let mut open_btn = node(Role::Button);
    open_btn.set_label("Open");
    open_btn.add_action(Action::Click);

    // 3. Article wrapping Heading + Paragraph (no ungrouped Labels): Article is structural,
    //    while Heading and Paragraph are each their own stop.
    let mut doc_article = node(Role::Article);
    doc_article.set_children(vec![NodeId(31), NodeId(32)]);
    let mut doc_h = node(Role::Heading);
    doc_h.set_label("Overview");
    let mut doc_p = node(Role::Paragraph);
    doc_p.set_label("Details here.");

    // 4. Paragraph with own text AND an inline Link: Paragraph stays focusable, Link stays native.
    let mut link_para = node(Role::Paragraph);
    link_para.set_label("Read the docs");
    link_para.set_children(vec![NodeId(41)]);
    let mut inline_link = node(Role::Link);
    inline_link.set_label("docs");
    inline_link.add_action(Action::Click);

    // 5. Nested List inside a ListItem: outer groups its own label, inner ListItem is its own stop.
    let mut outer_li = node(Role::ListItem);
    outer_li.set_label("Parent Child");
    outer_li.set_children(vec![NodeId(51), NodeId(52)]);
    let outer_lbl = label_node("Parent", Rect::new(0.0, 100.0, 80.0, 120.0));
    let mut nested_list = node(Role::List);
    nested_list.set_children(vec![NodeId(53)]);
    let mut inner_li = node(Role::ListItem);
    inner_li.set_label("Child");
    inner_li.set_children(vec![NodeId(54)]);
    let inner_lbl = label_node("Child", Rect::new(16.0, 124.0, 80.0, 144.0));

    let update = tree(
        NodeId(1),
        NodeId(1),
        vec![
            (NodeId(1), root),
            (NodeId(10), clickable_li),
            (NodeId(11), cli_t1),
            (NodeId(12), cli_t2),
            (NodeId(20), btn_only_li),
            (NodeId(21), open_btn),
            (NodeId(30), doc_article),
            (NodeId(31), doc_h),
            (NodeId(32), doc_p),
            (NodeId(40), link_para),
            (NodeId(41), inline_link),
            (NodeId(50), outer_li),
            (NodeId(51), outer_lbl),
            (NodeId(52), nested_list),
            (NodeId(53), inner_li),
            (NodeId(54), inner_lbl),
        ],
    );

    let props = classify_tree(&update);
    assert_eq!(props[&10], passive(), "clickable ListItem is natively focusable");
    assert_eq!(props[&11], passive(), "label inside clickable ListItem stays grouped");
    assert_eq!(props[&12], passive(), "label inside clickable ListItem stays grouped");

    assert_eq!(props[&20], passive(), "ListItem wrapping only a matching Button adds no extra stop");
    assert_eq!(props[&21], passive(), "Button is natively clickable");

    assert_eq!(props[&30], passive(), "Article containing only Heading + Paragraph adds no empty stop");
    assert_eq!(props[&31], heading_srf());
    assert_eq!(props[&32], srf());

    assert_eq!(props[&40], srf(), "Paragraph with inline Link stays focusable for its full text");
    assert_eq!(props[&41], passive(), "inline Link is natively clickable");

    assert_eq!(props[&50], srf(), "outer ListItem groups its direct text");
    assert_eq!(props[&51], passive(), "outer ListItem's direct text is grouped");
    assert_eq!(props[&53], srf(), "nested ListItem is its own stop");
    assert_eq!(props[&54], passive(), "nested ListItem's text is grouped");
}

#[test]
fn runtime_accessibility_tree_applies_per_role_rules_end_to_end() {
    let update = runtime_tree(|_, _| {
        col()
            .w(400.0)
            .h(600.0)
            .scroll(ScrollSpec::default())
            .child(
                col()
                    .key("scroll-content")
                    .child(text("Standalone banner").key("standalone"))
                    .child(text("Section title").key("heading").role("heading").level(2))
                    .child(
                        col()
                            .key("list")
                            .role("list")
                            .child(
                                row()
                                    .key("item")
                                    .role("listitem")
                                    .child(text("Wi-Fi").key("item-title"))
                                    .child(text("Connected").key("item-sub")),
                            ),
                    )
                    .child(
                        col()
                            .key("card")
                            .role("article")
                            .child(text("Update").key("card-title"))
                            .child(text("Ready to install").key("card-body")),
                    )
                    .child(
                        row()
                            .key("para")
                            .role("paragraph")
                            .child(text("Hello ").key("p1"))
                            .child(text("world").key("p2")),
                    )
                    .child(
                        row()
                            .key("apply")
                            .role("button")
                            .focusable(true)
                            .on_press(|_: &mut ()| {})
                            .child(text("Apply")),
                    ),
            )
    });

    let props = classify_tree(&update);
    assert_eq!(props[&id_by_key(&update, "standalone").0], srf());
    assert_eq!(props[&id_by_key(&update, "heading").0], heading_srf());
    assert_eq!(props[&id_by_key(&update, "item").0], srf());
    assert_eq!(props[&id_by_key(&update, "item-title").0], passive());
    assert_eq!(props[&id_by_key(&update, "item-sub").0], passive());
    assert_eq!(props[&id_by_key(&update, "card").0], srf());
    assert_eq!(props[&id_by_key(&update, "card-title").0], passive());
    assert_eq!(props[&id_by_key(&update, "card-body").0], passive());
    assert_eq!(props[&id_by_key(&update, "para").0], srf());
    assert_eq!(props[&id_by_key(&update, "p1").0], passive());
    assert_eq!(props[&id_by_key(&update, "p2").0], passive());
    assert_eq!(props[&id_by_key(&update, "apply").0], passive());
}

#[test]
fn incremental_update_tracks_focus_and_mixed_list_item_with_switch() {
    let mut root = node(Role::Window);
    root.set_children(vec![NodeId(10)]);

    let mut li = node(Role::ListItem);
    li.set_label("Notifications Daily summary");
    li.set_bounds(Rect::new(0.0, 0.0, 320.0, 56.0));
    li.set_children(vec![NodeId(11), NodeId(12)]);

    let lbl = label_node("Notifications", Rect::new(16.0, 12.0, 180.0, 36.0));
    let mut sw = node(Role::Switch);
    sw.set_label("Enable notifications");
    sw.add_action(Action::Click);
    sw.add_action(Action::Focus);
    sw.set_bounds(Rect::new(240.0, 12.0, 304.0, 44.0));

    let mut index = AndroidTreeIndex::default();
    index.apply_update(&tree(
        NodeId(1),
        NodeId(1),
        vec![(NodeId(1), root), (NodeId(10), li), (NodeId(11), lbl), (NodeId(12), sw.clone())],
    ));
    assert!(index.focus_is_root());
    assert_eq!(index.props_for_node(NodeId(10)), srf(), "ListItem groups its plain Label");
    assert_eq!(index.props_for_node(NodeId(11)), passive(), "Label inside ListItem stays grouped");
    assert_eq!(index.props_for_node(NodeId(12)), passive(), "Switch stays natively interactive");

    // Incremental update moving keyboard focus onto the Switch flips focus_is_root().
    index.apply_update(&TreeUpdate {
        nodes: vec![(NodeId(12), sw)],
        tree: None,
        tree_id: TreeId::ROOT,
        focus: NodeId(12),
    });
    assert!(!index.focus_is_root());
}
