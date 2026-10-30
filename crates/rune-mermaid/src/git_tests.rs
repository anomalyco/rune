use super::*;

fn git(src: &str) -> GitGraph {
    match crate::parse(src).map(|p| p.diagram) {
        Some(crate::Diagram::Git(g)) => g,
        _ => panic!("not a git graph: {src:?}"),
    }
}

#[test]
fn branches_merges_and_cherry_picks() {
    let g = git(
        "gitGraph\n    commit id: \"Alpha\"\n    commit tag: \"v1.0\" type: HIGHLIGHT\n    branch develop order: 2\n    checkout develop\n    commit id:\"Beta\" type: REVERSE\n    commit\n    switch main\n    merge develop id: \"M\" tag: \"v2\"\n    branch \"feature/x\"\n    commit id: \"F\"\n    cherry-pick id: \"Beta\"\n    checkout nowhere\n    merge main\n    merge feature/x\n    bogus line",
    );
    assert_eq!(g.flow, Flow::Right);
    let names: Vec<&str> = g.branches.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, ["main", "develop", "feature/x"]);
    assert_eq!(g.branches[1].order, Some(2.0));
    let shape: Vec<(Option<&str>, Kind, usize, &[usize])> =
        g.commits.iter().map(|c| (c.id.as_deref(), c.kind, c.branch, c.parents.as_slice())).collect();
    assert_eq!(
        shape,
        [
            (Some("Alpha"), Kind::Normal, 0, &[][..]),
            (None, Kind::Highlight, 0, &[0][..]),
            (Some("Beta"), Kind::Reverse, 1, &[1][..]),
            (None, Kind::Normal, 1, &[2][..]),
            (Some("M"), Kind::Merge, 0, &[1, 3][..]),
            (Some("F"), Kind::Normal, 2, &[4][..]),
            (None, Kind::CherryPick, 2, &[5][..]),
        ],
        "merging main into feature/x is skipped: feature/x already has main's head"
    );
    assert_eq!(g.commits[1].tag.as_deref(), Some("v1.0"));
    assert_eq!(g.commits[4].tag.as_deref(), Some("v2"));
    assert_eq!(g.commits[6].picked, Some(2));
}

#[test]
fn flows_and_bounds() {
    assert_eq!(git("gitGraph TB:\ncommit").flow, Flow::Down);
    assert_eq!(git("gitGraph BT:\ncommit").flow, Flow::Up);
    assert!(crate::parse("gitGraph\nbranch x").is_none(), "no commits");
    let budget = format!("gitGraph\n{}", "commit\n".repeat(MAX_ITEMS + 1));
    assert!(crate::parse(&budget).is_none());
}

#[test]
fn draws_lanes_ids_and_tags() {
    for src in ["gitGraph\ncommit id: \"A\" tag: \"v1\"\nbranch dev\ncommit id: \"B\"\ncheckout main\nmerge dev", "gitGraph TB:\ncommit id: \"A\"\nbranch dev\ncommit id: \"B\" tag: \"t\""] {
        let s = crate::scene(src, &DiagramStyle::neutral(false), None).unwrap();
        let texts: Vec<&str> = s.labels.iter().map(|l| l.text.as_str()).collect();
        for t in ["main", "dev", "A", "B"] {
            assert!(texts.contains(&t), "{t} in {texts:?}");
        }
    }
}
