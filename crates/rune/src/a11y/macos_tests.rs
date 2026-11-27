//! Regression checks for macOS NSAccessibility role mappings in `vendor/accesskit_macos`
//! alongside Rune's scroll-viewport AccessKit output.

use accesskit::{Action as Ak, Role};

use crate::{Cx, El, Env, Runtime, ScrollSpec, col, el, row, text};

const MACOS_NODE_RS: &str = include_str!("../../../../vendor/accesskit_macos/src/node.rs");
const MACOS_PATCH: &str = include_str!("../../../../vendor/accesskit_macos.patch");

fn ns_role_body() -> &'static str {
    let start = MACOS_NODE_RS
        .find("fn ns_role(")
        .expect("fn ns_role present in vendor/accesskit_macos/src/node.rs");
    let end = MACOS_NODE_RS[start..]
        .find("fn label_is_exposed_in_value(")
        .expect("fn label_is_exposed_in_value follows fn ns_role");
    &MACOS_NODE_RS[start..start + end]
}

fn ns_role_arm(role_variant: &str) -> &'static str {
    let needle = format!("Role::{role_variant} =>");
    ns_role_body()
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with(&needle))
        .unwrap_or_else(|| panic!("missing ns_role arm for Role::{role_variant}"))
}

#[test]
fn scroll_viewports_emit_scroll_view_and_map_to_ax_scroll_area_on_macos() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        row().size(900.0, 320.0)
            .child(
                col().key("pane").w(300.0).h(300.0).scroll(ScrollSpec::default())
                    .children((0..50).map(|i| text(format!("Row {i}")))),
            )
            .child(
                row().key("strip").w(300.0).h(300.0).scroll_x()
                    .child(el().size(900.0, 200.0)),
            )
            .child(
                col().key("board").w(300.0).h(300.0).scroll_xy()
                    .child(el().size(900.0, 900.0)),
            )
    }

    let mut rt = Runtime::new((), view, (900.0, 320.0), Env::new(true, 1.0));
    for _ in 0..2 {
        rt.frame(0.0);
    }

    let tree = rt.accessibility_tree();
    let json = rt.accessibility_json();

    for key in ["pane", "strip", "board"] {
        let (_, node) = tree
            .nodes
            .iter()
            .find(|(_, n)| n.author_id() == Some(key))
            .unwrap_or_else(|| panic!("missing scrolling viewport {key}"));
        assert_eq!(node.role(), Role::ScrollView, "{key} must emit Role::ScrollView");
        assert!(node.clips_children(), "{key} must clip children");
        assert!(node.supports_action(Ak::SetScrollOffset), "{key} must support SetScrollOffset");
        assert!(node.supports_action(Ak::ScrollDown), "{key} must support ScrollDown");
        if key != "pane" {
            assert!(node.supports_action(Ak::ScrollRight), "{key} must support ScrollRight");
        }
    }

    // Verify JSON dump and that every visible node role in the tree avoids AXUnknown on macOS.
    let children = json["children"].as_array().expect("root children");
    for key in ["pane", "strip", "board"] {
        let entry = children
            .iter()
            .find(|c| c["key"].as_str() == Some(key))
            .unwrap_or_else(|| panic!("missing {key} in accessibility_json()"));
        assert_eq!(entry["role"].as_str(), Some("scroll-view"));
    }

    for (_, node) in &tree.nodes {
        let role = node.role();
        if matches!(role, Role::GenericContainer | Role::TextRun) {
            // Excluded from the macOS AX tree by accesskit_consumer::common_filter.
            continue;
        }
        let arm = ns_role_arm(&format!("{role:?}"));
        assert!(
            !arm.contains("NSAccessibilityUnknownRole"),
            "visible role {role:?} must not map to NSAccessibilityUnknownRole on macOS: {arm}"
        );
    }
}

#[test]
fn vendored_accesskit_macos_maps_scroll_view_to_scroll_area_and_patch_preserves_it() {
    assert_eq!(
        ns_role_arm("ScrollView"),
        "Role::ScrollView => NSAccessibilityScrollAreaRole,",
        "vendor/accesskit_macos/src/node.rs must map Role::ScrollView to NSAccessibilityScrollAreaRole"
    );
    assert!(
        MACOS_PATCH.contains("-            Role::ScrollView => NSAccessibilityUnknownRole,")
            && MACOS_PATCH.contains("+            Role::ScrollView => NSAccessibilityScrollAreaRole,"),
        "vendor/accesskit_macos.patch must carry the ScrollView -> NSAccessibilityScrollAreaRole mapping"
    );
}
