use super::*;

/// A launch at `id`, as `launch_elements` starts it.
fn flight(id: NodeId) -> Launched {
    Launched { id, shift: [0.0; 2], drawn: None, drift: [0.0; 2], folded: [0.0; 2], folded_at: [0.0; 2] }
}

/// A flight of the window's, which keeps the launches at its node in the air.
fn air(id: NodeId) -> (NodeId, MRect) { (id, MRect::new(0.0, 0.0, 1.0, 1.0)) }

#[test]
fn a_launch_waits_for_its_destination_under_its_own_id() {
    let mut launches = Launches::default();
    assert!(!launches.pending(), "nothing asked for");
    launches.wait("msg-2/body".into(), [4.0, 9.0]);
    launches.wait("msg-3".into(), [0.0; 2]);
    assert!(launches.pending());
    let waiting = launches.take_waiting();
    assert_eq!(waiting.len(), 2);
    assert_eq!((waiting[0].id, waiting[1].id), (LaunchId(0), LaunchId(1)), "one id per launch");
    assert!(!launches.pending(), "taken for the commit that mounts them");
    assert!(!launches.flying(NodeId(1)), "a launch waiting for its node flies nothing");
}

#[test]
fn a_waiting_launch_whose_destination_never_mounts_is_dropped_by_its_id() {
    let mut launches = Launches::default();
    launches.wait("nowhere".into(), [0.0; 2]);
    let waiting = launches.take_waiting();
    assert_eq!(waiting[0].id, LaunchId(0));
    assert!(!launches.pending(), "a destination that never mounts waits one commit");
    launches.wait("afterwards".into(), [0.0; 2]);
    let waiting = launches.take_waiting();
    assert_eq!(waiting[0].id, LaunchId(1), "each launch is named, so the next one isn't taken for it");
}

#[test]
fn a_launch_in_the_air_flies_at_its_own_size_until_its_flight_leaves() {
    let mut launches = Launches::default();
    launches.wait("body".into(), [0.0; 2]);
    let launch = launches.take_waiting().pop().unwrap();
    let to = NodeId(7);
    launches.start(launch.id, flight(to));
    assert!(launches.flying(to) && launches.launched(to), "in the air, drawn outside every clip");
    launches.keep(&[air(to)]);
    assert!(launches.flying(to), "kept while its flight is in the air");
    launches.keep(&[]);
    assert!(!launches.flying(to) && !launches.launched(to), "landed, it clips again");
}

#[test]
fn a_matched_flight_started_unclipped_draws_outside_every_clip_until_it_lands() {
    let mut launches = Launches::default();
    let to = NodeId(3);
    launches.unclip(to, true);
    assert!(launches.flying(to) && !launches.launched(to), "unclipped, but not a launch");
    launches.keep(&[air(to)]);
    launches.keep(&[]);
    assert!(!launches.flying(to), "landed, it clips again");
    // A match without the flag leaves nothing behind, and neither does asking twice.
    launches.unclip(to, true);
    launches.unclip(to, true);
    launches.unclip(to, false);
    assert!(!launches.flying(to));
}

#[test]
fn a_launch_over_a_node_matched_unclipped_is_one_flight_that_stays_unclipped() {
    let mut launches = Launches::default();
    let to = NodeId(9);
    launches.unclip(to, true);
    launches.wait("body".into(), [1.0, 2.0]);
    let launch = launches.take_waiting().pop().unwrap();
    launches.start(launch.id, flight(to));
    assert!(launches.launched(to) && launches.flying(to), "one flight per node, still unclipped");
    launches.keep(&[]);
    assert!(!launches.flying(to), "gone with its flight");
}
