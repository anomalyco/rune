use super::*;

fn sankey(src: &str) -> Sankey {
    match crate::parse(src).map(|p| p.diagram) {
        Some(crate::Diagram::Sankey(s)) => s,
        _ => panic!("not a sankey diagram: {src:?}"),
    }
}

#[test]
fn rows_read_as_csv() {
    let s = sankey(
        "sankey-beta\n\nAgricultural 'waste',Bio-conversion,124.729\n\"Bio, conversion\",Liquid,0.597\n\"Say \"\"hi\"\"\",Losses,26.862\nbad row\nA,A,3\nA,B,-1\nA,B,x",
    );
    assert_eq!(s.nodes, ["Agricultural 'waste'", "Bio-conversion", "Bio, conversion", "Liquid", "Say \"hi\"", "Losses"]);
    assert_eq!(s.links.len(), 3, "self links, negative and unreadable values are skipped");
    assert_eq!(s.links[2], (4, 5, 26.862));
    assert!(crate::parse("sankey-beta\nnot,a number,here").is_none());
    let budget = format!("sankey-beta\n{}", (0..MAX_ITEMS).map(|i| format!("a{i},b{i},1")).collect::<Vec<_>>().join("\n"));
    assert!(crate::parse(&budget).is_none());
}

#[test]
fn sinks_go_last_and_cycles_stay_bounded() {
    let s = sankey("sankey-beta\nA,B,5\nB,C,3\nA,D,2\nC,B,1");
    let col = columns(&s);
    assert_eq!(col[0], 0);
    assert_eq!(col[3], *col.iter().max().unwrap(), "a sink sits in the last column");
    assert!(col.iter().all(|&c| c < s.nodes.len()));
}

#[test]
fn flows_run_left_to_right() {
    let paint = DiagramStyle::neutral(true);
    let s = crate::scene("sankey-beta\nSolar,Grid,10\nWind,Grid,20\nGrid,Homes,30", &paint, None).unwrap();
    let at = |t: &str| s.labels.iter().find(|l| l.text.starts_with(t)).unwrap().at;
    assert!(at("Solar")[0] < at("Grid")[0] && at("Grid")[0] < at("Homes")[0]);
    assert!(s.labels.iter().any(|l| l.text == "Grid 30"), "a node shows its flow");
}
