//! Headless tests for macOS thermal-state pacing, Low Power Mode on external high-refresh
//! displays, periodic power polling, and visible unfocused-window pacing (`App::unfocused_fps`).

use std::time::Duration;

use super::*;

fn drive_ticks(
    core: &mut DisplayLinkCore,
    t0: Instant,
    hw_period: Duration,
    frame_work: Duration,
    ticks: u32,
    mut before_tick: impl FnMut(&mut DisplayLinkCore, u32, Instant),
) -> (u32, u32) {
    let _ = core.start();
    let mut draws = 0u32;
    let mut waits = 0u32;
    for i in 0..ticks {
        let now = t0 + hw_period * i;
        before_tick(core, i, now);
        let target = now + hw_period;
        match core.claim(Some(target), now) {
            Opportunity::Tick(_) | Opportunity::Now | Opportunity::UrgentNow => {
                core.presented(now + frame_work);
                draws += 1;
            }
            Opportunity::Wait => waits += 1,
        }
    }
    (draws, waits)
}

#[test]
fn thermal_state_nominal_leaves_100hz_and_120hz_pacing_uncapped() {
    let t0 = crate::time::now();
    let frame_work = Duration::from_micros(900);

    for (hz, ticks) in [(120u64, 120u32), (100u64, 100u32)] {
        let period = Duration::from_nanos(1_000_000_000 / hz);
        let mut core = DisplayLinkCore::new(period);
        assert_eq!(core.thermal(), ThermalState::Nominal);
        let (draws, waits) = drive_ticks(&mut core, t0, period, frame_work, ticks, |_, _, _| {});
        assert_eq!(draws, ticks, "ThermalState::Nominal must keep full {hz} Hz");
        assert_eq!(waits, 0);
    }
}

#[test]
fn thermal_state_fair_caps_unsteered_motion_while_preserving_full_steered_rate() {
    let t0 = crate::time::now();
    let period_120 = Duration::from_nanos(1_000_000_000 / 120);
    let period_100 = Duration::from_nanos(1_000_000_000 / 100);
    let frame_work = Duration::from_micros(900);

    // 120 Hz panel under Fair thermal pressure: unsteered halves to 60 Hz, steered keeps 120 Hz.
    let mut unsteered_120 = DisplayLinkCore::new(period_120);
    unsteered_120.set_thermal(ThermalState::Fair);
    let (draws, waits) = drive_ticks(&mut unsteered_120, t0, period_120, frame_work, 120, |_, _, _| {});
    assert_eq!(draws, 60, "Fair thermal pressure caps unsteered 120 Hz to 60 Hz");
    assert_eq!(waits, 60);

    let mut steered_120 = DisplayLinkCore::new(period_120);
    steered_120.set_thermal(ThermalState::Fair);
    let (draws, waits) = drive_ticks(&mut steered_120, t0, period_120, frame_work, 120, |c, i, now| {
        if i % 2 == 0 { c.note_input(now); }
    });
    assert_eq!(draws, 120, "Fair thermal pressure keeps user-steered input at full 120 Hz");
    assert_eq!(waits, 0);

    // 100 Hz external 4K display under Fair thermal pressure: unsteered halves to 50 Hz.
    let mut unsteered_100 = DisplayLinkCore::new(period_100);
    unsteered_100.set_thermal(ThermalState::Fair);
    let (draws, waits) = drive_ticks(&mut unsteered_100, t0, period_100, frame_work, 100, |_, _, _| {});
    assert_eq!(draws, 50, "Fair thermal pressure caps unsteered 100 Hz external display to 50 Hz");
    assert_eq!(waits, 50);
}

#[test]
fn thermal_state_serious_and_critical_step_down_steered_and_unsteered_rates() {
    let t0 = crate::time::now();
    let period_120 = Duration::from_nanos(1_000_000_000 / 120);
    let period_100 = Duration::from_nanos(1_000_000_000 / 100);
    let period_60 = Duration::from_nanos(1_000_000_000 / 60);
    let frame_work = Duration::from_micros(900);

    // Serious: steered caps at 60 Hz (50 Hz on 100 Hz), unsteered caps at 30 Hz (25 Hz on 100 Hz).
    let mut serious_120_ambient = DisplayLinkCore::new(period_120);
    serious_120_ambient.set_thermal(ThermalState::Serious);
    let (draws, waits) = drive_ticks(&mut serious_120_ambient, t0, period_120, frame_work, 120, |_, _, _| {});
    assert_eq!(draws, 30, "Serious thermal pressure quarters unsteered 120 Hz to 30 Hz");
    assert_eq!(waits, 90);

    let mut serious_120_steered = DisplayLinkCore::new(period_120);
    serious_120_steered.set_thermal(ThermalState::Serious);
    let (draws, _) = drive_ticks(&mut serious_120_steered, t0, period_120, frame_work, 120, |c, i, now| {
        if i % 2 == 0 { c.note_input(now); }
    });
    assert_eq!(draws, 60, "Serious thermal pressure caps steered 120 Hz to 60 Hz");

    let mut serious_100_ambient = DisplayLinkCore::new(period_100);
    serious_100_ambient.set_thermal(ThermalState::Serious);
    let (draws, waits) = drive_ticks(&mut serious_100_ambient, t0, period_100, frame_work, 100, |_, _, _| {});
    assert_eq!(draws, 25, "Serious thermal pressure quarters unsteered 100 Hz external display to 25 Hz");
    assert_eq!(waits, 75);

    let mut serious_60_ambient = DisplayLinkCore::new(period_60);
    serious_60_ambient.set_thermal(ThermalState::Serious);
    let (draws, waits) = drive_ticks(&mut serious_60_ambient, t0, period_60, frame_work, 60, |_, _, _| {});
    assert_eq!(draws, 30, "Serious thermal pressure halves unsteered 60 Hz panel to 30 Hz");
    assert_eq!(waits, 30);

    // Critical: steered caps at 30 Hz, unsteered caps at 20 Hz (divides 60, 100, and 120 Hz evenly).
    for (hz, ticks) in [(120u64, 120u32), (100u64, 100u32), (60u64, 60u32)] {
        let period = Duration::from_nanos(1_000_000_000 / hz);
        let mut crit_ambient = DisplayLinkCore::new(period);
        crit_ambient.set_thermal(ThermalState::Critical);
        let (draws, _) = drive_ticks(&mut crit_ambient, t0, period, frame_work, ticks, |_, _, _| {});
        assert_eq!(draws, 20, "Critical thermal pressure must cap unsteered {hz} Hz to 20 Hz");
    }

    let mut crit_120_steered = DisplayLinkCore::new(period_120);
    crit_120_steered.set_thermal(ThermalState::Critical);
    let (draws, _) = drive_ticks(&mut crit_120_steered, t0, period_120, frame_work, 120, |c, i, now| {
        if i % 2 == 0 { c.note_input(now); }
    });
    assert_eq!(draws, 30, "Critical thermal pressure caps steered 120 Hz to 30 Hz");
}

#[test]
fn thermal_pressure_adapts_and_recovers_mid_stream_without_stopping_link() {
    let t0 = crate::time::now();
    let period_100 = Duration::from_nanos(1_000_000_000 / 100);
    let frame_work = Duration::from_micros(900);
    let mut core = DisplayLinkCore::new(period_100);

    // 4 seconds of continuous unsteered streaming across Nominal -> Fair -> Serious -> Nominal.
    let mut per_second = [0u32; 4];
    let _ = core.start();
    for i in 0..400u32 {
        let sec = (i / 100) as usize;
        if i % 100 == 0 {
            let thermal = match sec {
                0 => ThermalState::Nominal,
                1 => ThermalState::Fair,
                2 => ThermalState::Serious,
                _ => ThermalState::Nominal,
            };
            core.set_thermal(thermal);
        }
        let now = t0 + period_100 * i;
        if matches!(core.claim(Some(now + period_100), now), Opportunity::Tick(_) | Opportunity::Now) {
            core.presented(now + frame_work);
            per_second[sec] += 1;
        }
    }
    assert_eq!(per_second, [100, 50, 25, 100], "continuous stream must step down and recover cleanly");
    assert_eq!(core.coast(), Effects::one(LinkEffect::ArmCoast { ticks_left: 15 }));
}

#[test]
fn power_poll_due_refreshes_once_per_interval() {
    let t0 = crate::time::now();
    assert!(power_poll_due(None, t0), "first check is always due");
    assert!(!power_poll_due(Some(t0), t0 + Duration::from_millis(999)));
    assert!(power_poll_due(Some(t0), t0 + POWER_POLL_INTERVAL));
    assert!(power_poll_due(Some(t0), t0 + POWER_POLL_INTERVAL + Duration::from_millis(250)));
}

#[test]
fn low_power_mode_caps_external_100hz_display_to_50hz() {
    let t0 = crate::time::now();
    let period_100 = Duration::from_nanos(1_000_000_000 / 100);
    let frame_work = Duration::from_micros(900);

    let mut core = DisplayLinkCore::new(period_100);
    core.set_capped(true);
    let (draws, waits) = drive_ticks(&mut core, t0, period_100, frame_work, 100, |_, _, _| {});
    assert_eq!(draws, 50, "Low Power Mode halves 100 Hz external display to 50 Hz");
    assert_eq!(waits, 50);
    assert!((core.refresh_hz() - 60.0).abs() < 0.1);
}

#[test]
fn unfocused_fps_caps_visible_unfocused_window_without_pausing_and_lifts_on_input_or_focus() {
    let t0 = crate::time::now();
    let period_120 = Duration::from_nanos(1_000_000_000 / 120);
    let frame_work = Duration::from_micros(900);

    let mut core = DisplayLinkCore::new(period_120);
    core.set_ambient_period(crate::app::ambient_period_from_fps(60.0));
    core.set_unfocused_period(crate::app::ambient_period_from_fps(30.0));
    assert!(core.focused());

    // Focused + unsteered: ambient_fps (60 Hz) applies, unfocused_fps does not.
    let (draws, _) = drive_ticks(&mut core, t0, period_120, frame_work, 120, |_, _, _| {});
    assert_eq!(draws, 60, "focused window uses ambient_fps(60.0), not unfocused_fps(30.0)");

    // Window loses focus: unfocused_fps (30 Hz) applies without pausing the stream.
    let t1 = t0 + period_120 * 120;
    core.set_focused(false);
    assert!(!core.focused());
    let (draws, waits) = drive_ticks(&mut core, t1, period_120, frame_work, 120, |_, _, _| {});
    assert_eq!(draws, 30, "unfocused visible window caps to unfocused_fps(30.0) without pausing");
    assert_eq!(waits, 90);

    // Live input reaching the unfocused window (e.g. scrolling an inactive window on macOS)
    // lifts the cap immediately back to full 120 Hz.
    let t2 = t1 + period_120 * 120;
    let (draws, waits) = drive_ticks(&mut core, t2, period_120, frame_work, 120, |c, i, now| {
        if i % 2 == 0 { c.note_input(now); }
    });
    assert_eq!(draws, 120, "live scroll/pointer input over an unfocused window stays at 120 Hz");
    assert_eq!(waits, 0);

    // Regaining focus restores focused ambient pacing (60 Hz when unsteered).
    let t3 = t2 + period_120 * 120 + Duration::from_secs(3);
    core.set_focused(true);
    let (draws, _) = drive_ticks(&mut core, t3, period_120, frame_work, 120, |_, _, _| {});
    assert_eq!(draws, 60, "regaining focus lifts unfocused_fps back to ambient_fps(60.0)");
}

#[test]
fn losing_focus_drops_stale_steering_immediately_for_ambient_and_unfocused_caps() {
    let t0 = crate::time::now();
    let period_120 = Duration::from_nanos(1_000_000_000 / 120);
    let frame_work = Duration::from_micros(900);

    let mut core = DisplayLinkCore::new(period_120);
    core.set_ambient_period(crate::app::ambient_period_from_fps(60.0));
    core.set_unfocused_period(crate::app::ambient_period_from_fps(30.0));
    let _ = core.start();

    // User presses Enter at t = 0 and presents one frame (enters Steered { run: Active }).
    core.note_input(t0);
    assert!(matches!(core.claim(Some(t0 + period_120), t0), Opportunity::Tick(_)));
    core.presented(t0 + frame_work);
    assert!(core.steering().is_steered(t0 + period_120));

    // User immediately Cmd-Tabs away 5 ms later: Steering drops to Ambient at once so the
    // very next tick respects unfocused_fps(30.0) without waiting out 600 ms / 2 s.
    core.set_focused(false);
    assert_eq!(core.steering(), Steering::Ambient);
    assert_eq!(core.claim(Some(t0 + period_120 * 2), t0 + period_120), Opportunity::Wait);
    assert_eq!(core.claim(Some(t0 + period_120 * 3), t0 + period_120 * 2), Opportunity::Wait);
    assert_eq!(core.claim(Some(t0 + period_120 * 4), t0 + period_120 * 3), Opportunity::Wait);
    assert!(matches!(core.claim(Some(t0 + period_120 * 5), t0 + period_120 * 4), Opportunity::Tick(_)));
}

#[test]
fn caps_compose_monotonically_across_ambient_unfocused_low_power_and_thermal() {
    let t0 = crate::time::now();
    let period_120 = Duration::from_nanos(1_000_000_000 / 120);
    let frame_work = Duration::from_micros(900);

    // ambient_fps(20.0) beats higher unfocused_fps(30.0) when unfocused -> 20 Hz wins.
    let mut ambient_lower = DisplayLinkCore::new(period_120);
    ambient_lower.set_ambient_period(crate::app::ambient_period_from_fps(20.0));
    ambient_lower.set_unfocused_period(crate::app::ambient_period_from_fps(30.0));
    ambient_lower.set_focused(false);
    let (draws, _) = drive_ticks(&mut ambient_lower, t0, period_120, frame_work, 120, |_, _, _| {});
    assert_eq!(draws, 20, "lower ambient_fps(20.0) must win over unfocused_fps(30.0)");

    // Serious thermal (30 Hz ambient) beats unfocused_fps(60.0) -> 30 Hz wins.
    let mut thermal_lower = DisplayLinkCore::new(period_120);
    thermal_lower.set_unfocused_period(crate::app::ambient_period_from_fps(60.0));
    thermal_lower.set_focused(false);
    thermal_lower.set_thermal(ThermalState::Serious);
    let (draws, _) = drive_ticks(&mut thermal_lower, t0, period_120, frame_work, 120, |_, _, _| {});
    assert_eq!(draws, 30, "Serious thermal 30 Hz must win over unfocused_fps(60.0)");

    // unfocused_fps(15.0) beats Serious thermal (30 Hz) and Low Power Mode (60 Hz) -> 15 Hz wins.
    let mut unfocused_lowest = DisplayLinkCore::new(period_120);
    unfocused_lowest.set_capped(true);
    unfocused_lowest.set_thermal(ThermalState::Serious);
    unfocused_lowest.set_ambient_period(crate::app::ambient_period_from_fps(60.0));
    unfocused_lowest.set_unfocused_period(crate::app::ambient_period_from_fps(15.0));
    unfocused_lowest.set_focused(false);
    let (draws, _) = drive_ticks(&mut unfocused_lowest, t0, period_120, frame_work, 120, |_, _, _| {});
    assert_eq!(draws, 15, "unfocused_fps(15.0) must win over thermal 30 Hz and Low Power 60 Hz");
}
