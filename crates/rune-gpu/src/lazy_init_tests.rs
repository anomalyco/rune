use super::*;

fn sub(mip: u32, layer: u32) -> Subresource {
    Subresource { mip, layer }
}

fn collect(effects: Effects<Effect>) -> Vec<Effect> {
    effects.into_iter().collect()
}

/// Exhaustive table test over all (LoadKind, StoreKind) combinations on an uninitialized and an
/// initialized single-subresource texture.
#[test]
fn attach_transition_table() {
    struct Case {
        load: LoadKind,
        store: StoreKind,
        want_effects: Vec<Effect>,
        want_initialized: bool,
    }
    let cases = [
        Case { load: LoadKind::Clear, store: StoreKind::Store, want_effects: vec![], want_initialized: true },
        Case { load: LoadKind::Clear, store: StoreKind::Discard, want_effects: vec![], want_initialized: false },
        Case { load: LoadKind::Load, store: StoreKind::Store, want_effects: vec![Effect::PromoteLoadToClear], want_initialized: true },
        Case { load: LoadKind::Load, store: StoreKind::Discard, want_effects: vec![Effect::PromoteLoadToClear], want_initialized: false },
        Case { load: LoadKind::DontCare, store: StoreKind::Store, want_effects: vec![Effect::PromoteLoadToClear], want_initialized: true },
        Case { load: LoadKind::DontCare, store: StoreKind::Discard, want_effects: vec![Effect::PromoteLoadToClear], want_initialized: false },
    ];
    for c in cases {
        let mut st = State::new(Shape::new(1, 1));
        let got = collect(step(&mut st, Event::Attach { sub: sub(0, 0), load: c.load, store: c.store }));
        assert_eq!(got, c.want_effects, "uninit attach ({:?}, {:?})", c.load, c.store);
        assert_eq!(matches!(st, State::Initialized), c.want_initialized, "post-state ({:?}, {:?})", c.load, c.store);

        // Once initialized, no Attach event ever promotes or clears again.
        let mut init = State::Initialized;
        let got_init = collect(step(&mut init, Event::Attach { sub: sub(0, 0), load: c.load, store: c.store }));
        assert!(got_init.is_empty(), "initialized attach ({:?}, {:?}) must emit nothing", c.load, c.store);
        assert_eq!(init, State::Initialized);
    }
}

/// Table test for Read, WriteFull, and WritePartial on a 2-mip x 2-layer texture.
#[test]
fn read_and_write_subresource_transitions() {
    let mut st = State::new(Shape::new(2, 2));

    // Full write to (mip=0, layer=0): marks initialized with zero clear effects.
    assert!(collect(step(&mut st, Event::WriteFull(SubresourceRange::single(0, 0)))).is_empty());
    assert!(!st.is_uninit(sub(0, 0)));
    assert!(st.is_uninit(sub(1, 0)));
    assert!(st.is_uninit(sub(0, 1)));
    assert!(st.is_uninit(sub(1, 1)));

    // Reading all mips of layer 0 clears only (mip=1, layer=0), skipping already-initialized (0, 0).
    let r0 = collect(step(&mut st, Event::Read(SubresourceRange { base_mip: 0, mips: 2, base_layer: 0, layers: 1 })));
    assert_eq!(r0, vec![Effect::ClearSubresource(sub(1, 0))]);
    assert!(!st.is_uninit(sub(1, 0)));
    assert!(st.is_uninit(sub(0, 1)));

    // Partial write to (mip=0, layer=1) clears it first and marks it initialized.
    let wp = collect(step(&mut st, Event::WritePartial(SubresourceRange::single(0, 1))));
    assert_eq!(wp, vec![Effect::ClearSubresource(sub(0, 1))]);
    assert!(!st.is_uninit(sub(0, 1)));
    assert!(st.is_uninit(sub(1, 1)));

    // Second partial write to (mip=0, layer=1) emits nothing.
    assert!(collect(step(&mut st, Event::WritePartial(SubresourceRange::single(0, 1)))).is_empty());

    // Final read of (mip=1, layer=1) clears it and collapses State to Initialized.
    let r1 = collect(step(&mut st, Event::Read(SubresourceRange::single(1, 1))));
    assert_eq!(r1, vec![Effect::ClearSubresource(sub(1, 1))]);
    assert_eq!(st, State::Initialized);
}

/// Inline u64 boundary (64 subresources) and heap Vec<u64> spill (> 64 subresources).
#[test]
fn inline_and_heap_bitset_boundaries() {
    // Exactly 64 subresources stays Inline(u64::MAX).
    let mut s64 = State::new(Shape::new(8, 8));
    assert!(matches!(s64, State::Uninitialized { uninit: UninitBits::Inline(u64::MAX), .. }));
    let cleared = collect(step(&mut s64, Event::Read(SubresourceRange { base_mip: 0, mips: 8, base_layer: 0, layers: 8 })));
    assert_eq!(cleared.len(), 64);
    assert_eq!(s64, State::Initialized);

    // 65 subresources (5 mips x 13 layers) spills to Heap.
    let mut s65 = State::new(Shape::new(5, 13));
    assert!(matches!(s65, State::Uninitialized { uninit: UninitBits::Heap(_), .. }));
    // Clear the last subresource (index 64 = mip 4, layer 12).
    let last = collect(step(&mut s65, Event::Read(SubresourceRange::single(4, 12))));
    assert_eq!(last, vec![Effect::ClearSubresource(sub(4, 12))]);
    assert!(!s65.is_uninit(sub(4, 12)));
    assert!(s65.is_uninit(sub(0, 0)));
    // Full write over the entire range collapses to Initialized.
    assert!(collect(step(&mut s65, Event::WriteFull(SubresourceRange { base_mip: 0, mips: 5, base_layer: 0, layers: 13 }))).is_empty());
    assert_eq!(s65, State::Initialized);
}

/// LazyInit wrapper flips its AtomicBool fast path to false strictly when all subresources are
/// initialized.
#[test]
fn lazy_init_fast_path_flips_on_final_subresource() {
    let lazy = LazyInit::new(2, 1);
    assert!(lazy.has_uninit());
    assert!(lazy.step(Event::Attach { sub: sub(0, 0), load: LoadKind::Clear, store: StoreKind::Discard }).is_empty());
    assert!(lazy.has_uninit(), "Discard leaves subresource uninitialized");
    assert!(lazy.step(Event::Attach { sub: sub(0, 0), load: LoadKind::Clear, store: StoreKind::Store }).is_empty());
    assert!(lazy.has_uninit(), "mip 1 is still uninitialized");
    let e = collect(lazy.step(Event::Attach { sub: sub(1, 0), load: LoadKind::Load, store: StoreKind::Store }));
    assert_eq!(e, vec![Effect::PromoteLoadToClear]);
    assert!(!lazy.has_uninit(), "all subresources initialized: fast path now clear");
    assert!(lazy.step(Event::Read(SubresourceRange { base_mip: 0, mips: 2, base_layer: 0, layers: 1 })).is_empty());
}
