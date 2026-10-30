use super::*;

fn mindmap(src: &str) -> Mindmap {
    match crate::parse(src).map(|p| p.diagram) {
        Some(crate::Diagram::Mindmap(m)) => m,
        _ => panic!("not a mindmap: {src:?}"),
    }
}

fn texts(m: &Mindmap, i: usize) -> Vec<&str> {
    m.nodes[i].children.iter().map(|&k| m.nodes[k].text.as_str()).collect()
}

#[test]
fn nests_by_indentation_with_shapes() {
    let m = mindmap(
        "mindmap\n  root((mindmap))\n    Origins\n      Long history\n      ::icon(fa fa-book)\n      Popularisation\n        British author\n    Research\n      a[On effectiveness<br/>and features]\n      b))Bang((\n      c)Cloud(\n      d{{Hex}}\n      e(Rounded):::urgent\n    Tools\n      f[unclosed",
    );
    assert_eq!(m.nodes[0].text, "mindmap");
    assert_eq!(m.nodes[0].look, Look::Circle);
    assert_eq!(texts(&m, 0), ["Origins", "Research", "Tools"]);
    assert_eq!(texts(&m, 1), ["Long history", "Popularisation"]);
    let research = m.nodes[0].children[1];
    assert_eq!(texts(&m, research), ["On effectiveness\nand features", "Bang", "Cloud", "Hex", "Rounded"]);
    let looks: Vec<Look> = m.nodes[research].children.iter().map(|&k| m.nodes[k].look).collect();
    assert_eq!(looks, [Look::Square, Look::Bang, Look::Cloud, Look::Hexagon, Look::Rounded]);
    let tools = m.nodes[0].children[2];
    let f = m.nodes[tools].children[0];
    assert_eq!((m.nodes[f].text.as_str(), m.nodes[f].look), ("f[unclosed", Look::Plain), "read as written");
}

#[test]
fn one_root_and_bounds() {
    let m = mindmap("mindmap\n    Root\n  Shallower\n  Sibling\n    Child");
    assert_eq!(texts(&m, 0), ["Shallower", "Sibling"], "a line no deeper than the root hangs off it");
    assert_eq!(texts(&m, 2), ["Child"]);
    assert!(crate::parse("mindmap").is_none());
    let deep: String = (0..=MAX_DEPTH + 1).map(|d| format!("{}n{d}\n", " ".repeat(d * 2))).collect();
    assert!(crate::parse(&format!("mindmap\n{deep}")).is_none());
    let wide = format!("mindmap\nroot\n{}", (0..MAX_ITEMS).map(|i| format!("  n{i}")).collect::<Vec<_>>().join("\n"));
    assert!(crate::parse(&wide).is_none());
}

#[test]
fn branches_split_left_and_right() {
    let paint = DiagramStyle::neutral(true);
    let s = crate::scene("mindmap\n  Centre\n    One\n    Two\n    Three\n    Four", &paint, None).unwrap();
    let at = |t: &str| s.labels.iter().find(|l| l.text == t).map(|l| l.at[0] + l.size[0] / 2.0).unwrap();
    let centre = at("Centre");
    assert!(at("One") > centre && at("Two") > centre, "the first branches go right");
    assert!(at("Three") < centre && at("Four") < centre, "the rest go left");
}
