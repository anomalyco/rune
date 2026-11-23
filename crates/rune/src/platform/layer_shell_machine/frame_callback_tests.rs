use super::{Effect, Event, State, step};

fn each_sequence<T: Copy>(alphabet: &[T], depth: usize, seq: &mut Vec<T>, each: &mut impl FnMut(&[T])) {
    each(seq);
    if seq.len() == depth {
        return;
    }
    for &symbol in alphabet {
        seq.push(symbol);
        each_sequence(alphabet, depth, seq, each);
        seq.pop();
    }
}

#[test]
fn frame_callback_gates_present_until_matching_done_and_ignores_stale_callbacks() {
    let mut state = State::default();
    assert!(state.ready(), "a newly configured layer surface can present its first frame");

    // Presenting requests callback 1 and blocks further presents until callback 1 completes.
    let effects: Vec<_> = step(&mut state, Event::Request).into_iter().collect();
    assert_eq!(effects, [Effect::Request(1)]);
    assert!(!state.ready());

    // Stale or mismatched callback ids cannot unlock the surface prematurely.
    assert!(step(&mut state, Event::Done(0)).is_empty());
    assert!(step(&mut state, Event::Done(2)).is_empty());
    assert!(!state.ready());

    // The matching done event unlocks the next present.
    assert!(step(&mut state, Event::Done(1)).is_empty());
    assert!(state.ready());

    // Duplicate done while ready is a no-op.
    assert!(step(&mut state, Event::Done(1)).is_empty());
    assert!(state.ready());

    // Next present gets a fresh monotonic id (2); Reset clears pending and advances past it so a
    // late done(2) cannot clear callback 3.
    let effects: Vec<_> = step(&mut state, Event::Request).into_iter().collect();
    assert_eq!(effects, [Effect::Request(2)]);
    assert!(!state.ready());
    assert!(step(&mut state, Event::Reset).is_empty());
    assert!(state.ready());
    let effects: Vec<_> = step(&mut state, Event::Request).into_iter().collect();
    assert_eq!(effects, [Effect::Request(3)]);
    assert!(!state.ready());
    assert!(step(&mut state, Event::Done(2)).is_empty());
    assert!(!state.ready(), "a late done from before Reset must not clear the new callback");
    assert!(step(&mut state, Event::Done(3)).is_empty());
    assert!(state.ready());

    // Exhaustive walk over short sequences: ready() matches whether the latest requested id is
    // still awaiting its done, and issued callback ids strictly increase.
    let alphabet = [Event::Request, Event::Done(1), Event::Done(2), Event::Done(3), Event::Reset];
    each_sequence(&alphabet, 5, &mut Vec::new(), &mut |seq| {
        let mut s = State::default();
        let mut pending: Option<u64> = None;
        let mut last_id = 0u64;
        for &event in seq {
            let effects: Vec<_> = step(&mut s, event).into_iter().collect();
            match event {
                Event::Request => {
                    assert_eq!(effects.len(), 1);
                    let Effect::Request(id) = effects[0];
                    assert!(id > last_id, "callback ids must strictly increase: {id} <= {last_id} in {seq:?}");
                    last_id = id;
                    pending = Some(id);
                }
                Event::Done(id) => {
                    assert!(effects.is_empty());
                    if pending == Some(id) {
                        pending = None;
                    }
                }
                Event::Reset => {
                    assert!(effects.is_empty());
                    pending = None;
                }
            }
            assert_eq!(s.ready(), pending.is_none(), "state mismatch after {seq:?}");
        }
    });
}
