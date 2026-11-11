use super::*;

/// A launch at `id`, as `launch_elements` starts it.
fn flight(id: NodeId) -> Launched {
    Launched { id, shift: [0.0; 2], drawn: None, drift: [0.0; 2], folded: [0.0; 2], folded_at: [0.0; 2] }
}

/// A flight of the window's, which keeps the launches at its node in the air.
fn air(id: NodeId) -> (NodeId, MRect) { (id, MRect::new(0.0, 0.0, 1.0, 1.0)) }

/// The attempts this commit hands out, one per launch waiting for a destination.
fn mounts(launches: &mut Launches) -> Vec<(Attempt, String, [f32; 2])> {
    launches.commit().into_iter().map(|mount| {
        let LaunchEffect::Mount { attempt, target, from } = mount;
        (attempt, target, from)
    }).collect()
}

/// How many flights the launch at `id` has in the air: one, or the state is wrong.
fn airborne(launches: &Launches, id: NodeId) -> usize {
    launches.launches.iter().filter(|l| l.airborne_id() == Some(id)).count()
}

#[test]
fn a_launch_waits_for_its_destination_under_its_own_attempt() {
    let mut launches = Launches::default();
    assert!(!launches.pending(), "nothing asked for");
    launches.wait("msg-2/body".into(), [4.0, 9.0]);
    launches.wait("msg-3".into(), [0.0; 2]);
    assert!(launches.pending());
    let asked = mounts(&mut launches);
    assert_eq!(asked.len(), 2);
    assert_eq!((asked[0].0, asked[1].0), (Attempt(1), Attempt(2)), "one attempt per launch");
    assert_eq!((asked[0].1.as_str(), asked[0].2), ("msg-2/body", [4.0, 9.0]), "where its source's text was");
    assert!(launches.pending(), "it waits for the answer the mount attempt gets");
    assert!(!launches.flying(NodeId(1)), "a launch waiting for its node flies nothing");
}

#[test]
fn a_waiting_launch_whose_destination_never_mounts_ends_by_its_answer() {
    let mut launches = Launches::default();
    launches.wait("nowhere".into(), [0.0; 2]);
    let asked = mounts(&mut launches);
    launches.miss(asked[0].0);
    assert!(!launches.pending(), "answered, it is over rather than waiting forever");
    let wanted = NodeId(4);
    launches.take_off(asked[0].0, flight(wanted));
    assert!(!launches.flying(wanted), "an answer to a launch that is gone changes nothing");
}

#[test]
fn a_launch_in_the_air_flies_at_its_own_size_until_its_flight_leaves() {
    let mut launches = Launches::default();
    launches.wait("body".into(), [0.0; 2]);
    let asked = mounts(&mut launches);
    let to = NodeId(7);
    launches.take_off(asked[0].0, flight(to));
    assert!(launches.flying(to) && launches.launched(to), "in the air, drawn outside every clip");
    assert_eq!(airborne(&launches, to), 1, "one flight at its node");
    assert!(!launches.pending(), "taken off, it no longer waits");
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
    assert_eq!(airborne(&launches, to), 1);
    launches.keep(&[air(to)]);
    launches.keep(&[]);
    assert!(!launches.flying(to), "landed, it clips again");
    // A match without the flag leaves nothing behind, and neither does asking twice.
    launches.unclip(to, true);
    launches.unclip(to, true);
    assert_eq!(airborne(&launches, to), 1, "one entry, however often it is asked for");
    launches.unclip(to, false);
    assert!(!launches.flying(to));
}

#[test]
fn a_launch_over_a_node_matched_unclipped_is_one_flight_that_stays_unclipped() {
    let mut launches = Launches::default();
    let to = NodeId(9);
    launches.unclip(to, true);
    launches.wait("body".into(), [1.0, 2.0]);
    let asked = mounts(&mut launches);
    launches.take_off(asked[0].0, flight(to));
    assert_eq!(airborne(&launches, to), 1, "the matched entry gives way to the launch, not beside it");
    assert!(launches.launched(to) && launches.flying(to), "one flight per node, still unclipped");
    launches.keep(&[]);
    assert!(!launches.flying(to), "gone with its flight");
}
