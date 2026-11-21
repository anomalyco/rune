use super::*;

fn meter() -> (SurfaceMeter, Instant) {
    let meter = SurfaceMeter::new("main", Arc::default());
    let start = meter.started;
    (meter, start)
}

#[test]
fn rates_are_per_second_over_the_last_window_and_go_stale() {
    let (mut meter, start) = meter();
    for _ in 0..120 { meter.redraw(true); meter.presented(); }
    for _ in 0..3 { meter.failed(Missed::Timeout); }
    meter.skipped();
    meter.turn(false, false);
    meter.turn(true, false);
    meter.set_pacing(Pacing::DisplayLink);
    assert_eq!(meter.shared.lock().unwrap().per_second, SurfaceCounts::default(), "rates wait for a whole second");
    assert!(meter.tick(start + Duration::from_millis(1000)).is_none());
    let stats = meter.shared.lock().unwrap().clone();
    assert_eq!(stats.name, "main");
    assert_eq!((stats.per_second.redraws, stats.per_second.unpaced, stats.per_second.presents), (120, 0, 120));
    assert_eq!((stats.per_second.timeouts, stats.per_second.failed(), stats.per_second.skipped), (3, 3, 1));
    assert_eq!((stats.per_second.turns, stats.per_second.wakes), (2, 1));
    assert_eq!(stats.pacing, Pacing::DisplayLink);
    assert_eq!(stats.totals.presents, 120);

    // Half the work over the next two seconds: rates are per second, totals keep counting.
    for _ in 0..60 { meter.redraw(false); meter.presented(); }
    meter.tick(start + Duration::from_millis(3000));
    let stats = meter.shared.lock().unwrap().clone();
    assert_eq!((stats.per_second.redraws, stats.per_second.unpaced, stats.per_second.presents), (30, 30, 30));
    assert_eq!((stats.totals.redraws, stats.totals.presents), (180, 180));
    assert_eq!(meter.totals(), stats.totals);

    let fresh = stats.clone().aged(start + Duration::from_millis(4000));
    assert_eq!(fresh.per_second.presents, 30);
    let stale = stats.aged(start + Duration::from_millis(5500));
    assert_eq!(stale.per_second, SurfaceCounts::default(), "a surface quiet for two seconds reports no rate");
    assert_eq!(stale.totals.presents, 180);
}

#[test]
fn totals_and_pacing_publish_at_once() {
    // A window that draws for less than a second, then rests, still reports what it drew.
    let (mut meter, _) = meter();
    meter.set_pacing(Pacing::DisplayLink);
    assert_eq!(read(&meter.shared).pacing, Pacing::DisplayLink, "pacing set after the frame's counts still lands");
    for _ in 0..3 { meter.redraw(true); meter.presented(); }
    meter.set_pacing(Pacing::Idle);
    let stats = read(&meter.shared);
    assert_eq!((stats.totals.redraws, stats.totals.presents), (3, 3));
    assert_eq!(stats.pacing, Pacing::Idle);
    assert_eq!(stats.per_second, SurfaceCounts::default());
    assert!(stats.summary().contains("totals 3 presents"), "{}", stats.summary());
}

#[test]
fn only_redraws_that_waited_for_nothing_are_unpaced() {
    let (mut meter, _) = meter();
    meter.redraw(true);
    assert_eq!(meter.totals().unpaced, 0, "a display tick or vsync paces it");
    meter.waited();
    meter.redraw(false);
    assert_eq!(meter.totals().unpaced, 0, "an event asked for it");
    meter.turn(true, true);
    meter.redraw(false);
    assert_eq!(meter.totals().unpaced, 0, "a timer that was still ahead paced it");
    meter.turn(true, false);
    meter.redraw(false);
    assert_eq!(meter.totals().unpaced, 1, "an overdue deadline or poll waited for nothing");
    meter.waited();
    meter.redraw(true);
    meter.redraw(false);
    assert_eq!(meter.totals().unpaced, 2, "each wait paces one redraw");
}

#[test]
fn the_guard_warns_once_after_two_seconds_of_spinning() {
    let (mut meter, start) = meter();
    let at = |ms| start + Duration::from_millis(ms);
    let spin = |meter: &mut SurfaceMeter, n| for _ in 0..n { meter.redraw(true); meter.failed(Missed::Occluded); };
    meter.set_pacing(Pacing::Failed(-6661));
    spin(&mut meter, 5000);
    assert!(meter.tick(at(1000)).is_none(), "one hot second isn't a spin");
    spin(&mut meter, 5000);
    let warning = meter.tick(at(2000)).expect("two hot seconds are");
    assert!(warning.contains("main") && warning.contains("5000 failed acquires/s") && warning.contains("occluded 5000"), "{warning}");
    assert!(warning.contains("display link failed (-6661)"), "{warning}");
    spin(&mut meter, 5000);
    assert!(meter.tick(at(3000)).is_none(), "it warns once");

    // Unpaced redraws spin too; a calm second in between resets the streak.
    let (mut meter, start) = super::tests::meter();
    let at = |ms| start + Duration::from_millis(ms);
    let unpaced = |meter: &mut SurfaceMeter, n| for _ in 0..n { meter.redraw(false); meter.presented(); };
    unpaced(&mut meter, 1000);
    assert!(meter.tick(at(1000)).is_none());
    unpaced(&mut meter, 60);
    assert!(meter.tick(at(2000)).is_none(), "a 60 Hz second resets the streak");
    unpaced(&mut meter, 1000);
    assert!(meter.tick(at(3000)).is_none());
    unpaced(&mut meter, 1000);
    assert!(meter.tick(at(4000)).is_some_and(|w| w.contains("1000 redraws/s (1000 unpaced)")));

    // A display's own rate, even 360 Hz, never trips it.
    let (mut meter, start) = super::tests::meter();
    for second in 1..=5 {
        unpaced(&mut meter, 360);
        assert!(meter.tick(start + Duration::from_secs(second)).is_none());
    }
}
