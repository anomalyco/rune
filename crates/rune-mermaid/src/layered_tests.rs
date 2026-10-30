use super::*;

fn graph(n: usize, edges: &[(usize, usize)]) -> Graph {
    Graph {
        sizes: vec![[60.0, 30.0]; n],
        edges: edges.iter().map(|&(from, to)| GEdge { from, to, label: None, minlen: 1 }).collect(),
        node_cluster: vec![None; n],
        clusters: Vec::new(),
    }
}
const SP: Spacing = Spacing { node: 30.0, rank: 40.0, cluster: 10.0 };

#[test]
fn a_chain_goes_down_in_a_straight_line() {
    let p = layout(&graph(3, &[(0, 1), (1, 2)]), &SP).unwrap();
    assert!(p.centers[0][1] < p.centers[1][1] && p.centers[1][1] < p.centers[2][1]);
    assert!((p.centers[0][0] - p.centers[2][0]).abs() < 0.5, "{:?}", p.centers);
    assert_eq!(p.centers[1][1] - p.centers[0][1], 30.0 + 40.0, "one rank is a node plus the rank gap");
}

#[test]
fn a_fork_spreads_children_without_overlap_and_centres_the_parent() {
    let p = layout(&graph(3, &[(0, 1), (0, 2)]), &SP).unwrap();
    let (a, b) = (p.centers[1], p.centers[2]);
    assert_eq!(a[1], b[1]);
    assert!((a[0] - b[0]).abs() >= 90.0 - 0.01, "{a:?} {b:?}");
    assert!((p.centers[0][0] - (a[0] + b[0]) / 2.0).abs() < 1.0);
}

#[test]
fn cycles_and_self_loops_lay_out() {
    let p = layout(&graph(3, &[(0, 1), (1, 2), (2, 0), (1, 1)]), &SP).unwrap();
    assert_eq!(p.paths.len(), 4);
    let back = &p.paths[2];
    assert_eq!(back.first(), Some(&p.centers[2]), "a reversed edge still runs from its source");
    assert_eq!(back.last(), Some(&p.centers[0]));
    assert!(p.centers.iter().all(|c| c[0].is_finite() && c[1].is_finite()));
}

#[test]
fn sibling_clusters_never_overlap_across_layers() {
    let mut g = graph(7, &[(0, 1), (2, 3), (2, 4), (0, 2), (3, 5), (4, 5), (4, 6)]);
    g.clusters = vec![
        GCluster { parent: None, lead: [0.0, 16.0], min_span: [0.0, 0.0] },
        GCluster { parent: None, lead: [0.0, 16.0], min_span: [0.0, 0.0] },
    ];
    g.node_cluster = vec![Some(0), Some(0), Some(1), Some(1), Some(1), None, None];
    let p = layout(&g, &SP).unwrap();
    let (a, b) = (p.clusters[0], p.clusters[1]);
    let apart = a[2] <= b[0] || b[2] <= a[0] || a[3] <= b[1] || b[3] <= a[1];
    assert!(apart, "{a:?} {b:?}");
    for v in [5, 6] {
        let c = p.centers[v];
        for bx in [a, b] {
            let inside = c[0] > bx[0] && c[0] < bx[2] && c[1] > bx[1] && c[1] < bx[3];
            assert!(!inside, "node {v} at {c:?} inside {bx:?}");
        }
    }
}

#[test]
fn isotonic_keeps_separation_and_hits_free_targets() {
    assert_eq!(isotonic(&[0.0, 100.0], &[1.0, 1.0], &[10.0]), vec![0.0, 100.0]);
    let x = isotonic(&[50.0, 50.0], &[1.0, 1.0], &[20.0]);
    assert_eq!(x, vec![40.0, 60.0]);
}

#[test]
fn clusters_stay_contiguous_and_contain_their_members() {
    let mut g = graph(4, &[(0, 1), (0, 2), (0, 3)]);
    g.clusters.push(GCluster { parent: None, lead: [0.0, 16.0], min_span: [0.0, 0.0] });
    g.node_cluster[1] = Some(0);
    g.node_cluster[3] = Some(0);
    let p = layout(&g, &SP).unwrap();
    let b = p.clusters[0];
    for v in [1, 3] {
        let c = p.centers[v];
        assert!(c[0] - 30.0 >= b[0] && c[0] + 30.0 <= b[2] && c[1] - 15.0 >= b[1] + 16.0, "{c:?} in {b:?}");
    }
    let c2 = p.centers[2];
    assert!(c2[0] + 30.0 <= b[0] || c2[0] - 30.0 >= b[2], "a non-member sits outside: {c2:?} {b:?}");
}
