use std::sync::atomic::Ordering;

use super::{GpuJob, Pick, PickEffect, PickEvent, PickState, SurfaceMode, defers_software, step_pick};
use crate::gpu::fallback::Policy;
use crate::window::CpuFirst;
use crate::platform::cpu_frames::Put;
use crate::window::linux::cpu_frame::Cpu;

/// What reaches a window while its GPU is made and around the hand-over, as the host answers
/// each: with a frame, drawn by whichever side has the window (`Host::draw`).
#[derive(Clone, Copy, Debug)]
enum Event {
    /// A redraw: the compositor's frame callback, a timer's frame, an input event's.
    Redraw,
    /// The window is another size: the next frame is that size.
    Resize(u32, u32),
    /// The display's scale changed: the same window in more device pixels, painted whole.
    Scale(u32),
    /// The display lost the window's pixels and asks for them again (X11).
    Expose,
    /// A frame in which nothing changed.
    Still,
}

const EVENTS: [Event; 5] = [Event::Redraw, Event::Resize(320, 200), Event::Scale(2), Event::Expose, Event::Still];

/// The CPU's answer to `event`, through the one way the host has to a window's CPU frames
/// (`SurfaceMode::cpu`). True when the CPU drew.
fn cpu_frame(mode: &mut SurfaceMode, event: Event) -> bool {
    let Some(cpu) = mode.cpu() else { return false };
    let size = match event {
        Event::Resize(width, height) => (width, height),
        Event::Scale(scale) => (160 * scale, 100 * scale),
        Event::Redraw | Event::Expose | Event::Still => (160, 100),
    };
    if matches!(event, Event::Scale(_) | Event::Expose) { cpu.frames.forget(); }
    if cpu.frames.begin(size).is_none() { return false; }
    cpu.frames.present(if matches!(event, Event::Still) { Put::Rects(&[]) } else { Put::All });
    true
}

/// Every order of `EVENTS`.
fn orders() -> Vec<Vec<Event>> {
    fn go(rest: &[Event], order: &mut Vec<Event>, out: &mut Vec<Vec<Event>>) {
        if rest.is_empty() { return out.push(order.clone()); }
        for i in 0..rest.len() {
            let mut left = rest.to_vec();
            order.push(left.remove(i));
            go(&left, order, out);
            order.pop();
        }
    }
    let mut out = Vec::new();
    go(&EVENTS, &mut Vec::new(), &mut out);
    out
}

fn building(explicit_sync: bool) -> (SurfaceMode, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let (cpu, sent) = Cpu::fake(explicit_sync);
    (SurfaceMode::Cpu { cpu, job: GpuJob::Building { tries: 1, latest: Default::default(), pick: Default::default() } }, sent)
}

#[test]
fn the_cpu_draws_every_event_until_the_switch_and_none_after_it() {
    for order in orders() {
        // The switch after each number of events, the rest of them after it.
        for at in 0..=order.len() {
            let (mut mode, sent) = building(true);
            let before = order[..at].iter().filter(|event| !matches!(event, Event::Still)).count();
            for &event in &order[..at] { assert!(cpu_frame(&mut mode, event), "{event:?} before the switch draws on the CPU"); }
            assert_eq!(sent.load(Ordering::Relaxed), before);
            assert_eq!(mode.switch(), Some(1));
            assert!(!mode.cpu_draws());
            for &event in &order[at..] { assert!(!cpu_frame(&mut mode, event), "{event:?} after the switch drew on the CPU"); }
            // The GPU's first frame presented (`handed`): the CPU's side goes.
            mode = SurfaceMode::Gpu;
            for &event in &order { assert!(!cpu_frame(&mut mode, event)); }
            assert_eq!(sent.load(Ordering::Relaxed), before, "a CPU frame reached the display after the switch ({order:?}, switch after {at})");
        }
    }
}

#[test]
fn a_window_switches_once_and_only_while_its_gpu_is_being_made() {
    for job in [GpuJob::Waiting { tries: 0, retry_at: None }, GpuJob::SoftwareDeferred, GpuJob::Failed] {
        let (cpu, sent) = Cpu::fake(false);
        let mut mode = SurfaceMode::Cpu { cpu, job };
        assert_eq!(mode.switch(), None);
        assert!(cpu_frame(&mut mode, Event::Redraw));
        assert_eq!(sent.load(Ordering::Relaxed), 1);
    }
    let (mut mode, _) = building(false);
    assert_eq!(mode.switch(), Some(1));
    assert_eq!(mode.switch(), None);
    assert_eq!(SurfaceMode::Gpu.switch(), None);
}

#[test]
fn a_failed_surface_gives_the_window_back_to_the_cpu_where_no_explicit_sync_is() {
    let (mut mode, sent) = building(false);
    assert!(cpu_frame(&mut mode, Event::Redraw));
    mode.switch();
    assert!(mode.switch_back(GpuJob::Waiting { tries: 1, retry_at: None }));
    assert!(mode.cpu_draws());
    // What the buffers hold counts as no frame's: the next one is painted whole.
    assert!(!mode.cpu().unwrap().frames.begin((160, 100)).unwrap().kept);
    assert!(cpu_frame(&mut mode, Event::Redraw));
    assert_eq!(sent.load(Ordering::Relaxed), 2);
}

#[test]
fn a_failed_surface_leaves_the_window_with_the_gpu_under_explicit_sync() {
    for order in orders().into_iter().take(12) {
        let (mut mode, sent) = building(true);
        assert!(cpu_frame(&mut mode, Event::Redraw));
        mode.switch();
        assert!(!mode.switch_back(GpuJob::Waiting { tries: 1, retry_at: None }));
        assert!(matches!(mode, SurfaceMode::Handing { tries: 1, .. }));
        for &event in &order { assert!(!cpu_frame(&mut mode, event)); }
        // A second failed try changes nothing.
        assert!(!mode.switch_back(GpuJob::Failed));
        for &event in &order { assert!(!cpu_frame(&mut mode, event)); }
        assert_eq!(sent.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn only_a_window_that_switched_goes_back() {
    let (mut mode, _) = building(false);
    assert!(!mode.switch_back(GpuJob::Failed));
    assert!(matches!(mode, SurfaceMode::Cpu { job: GpuJob::Building { tries: 1, .. }, .. }));
    assert!(!SurfaceMode::Gpu.switch_back(GpuJob::Failed));
}

#[test]
fn only_an_automatic_auto_hand_over_leaves_a_software_gpu_alone() {
    assert!(defers_software(CpuFirst::Auto, Policy::Automatic));
    assert!(!defers_software(CpuFirst::Auto, Policy::Pinned));
    assert!(!defers_software(CpuFirst::Only, Policy::Automatic));
    assert!(!defers_software(CpuFirst::Off, Policy::Automatic));
}

#[test]
fn transparent_fallback_starts_with_failed_gpu_job_and_never_switches_or_retries() {
    let (cpu, sent) = Cpu::fake(false);
    let mut mode = SurfaceMode::Cpu { cpu, job: GpuJob::Failed };
    assert!(mode.cpu_draws());
    assert_eq!(mode.switch(), None, "GpuJob::Failed must never switch to GPU");
    assert!(cpu_frame(&mut mode, Event::Redraw));
    assert!(cpu_frame(&mut mode, Event::Resize(320, 200)));
    assert_eq!(sent.load(Ordering::Relaxed), 2);
    // GpuJob::Failed is not GpuJob::Waiting, so neither gpu_owed nor handover_deadline triggers.
    assert!(matches!(mode, SurfaceMode::Cpu { job: GpuJob::Failed, .. }));
}

#[test]
fn a_frame_that_needs_the_gpu_takes_the_software_one_a_thread_found() {
    // Pure state machine checks:
    // 1. Idle software adapter with no NeedsGpu -> SkipWarmFrame then DeferSoftware.
    let mut s = PickState::Pending { defer_software: true };
    assert_eq!(step_pick(&mut s, PickEvent::AdapterFound { software: true }).into_iter().collect::<Vec<_>>(), vec![PickEffect::SkipWarmFrame]);
    assert_eq!(step_pick(&mut s, PickEvent::Ready).into_iter().collect::<Vec<_>>(), vec![PickEffect::DeferSoftware]);

    // 2. NeedsGpu arrives BEFORE worker inspects the software adapter -> RunWarmFrame then SwitchToGpu.
    let mut s = PickState::Pending { defer_software: true };
    assert!(step_pick(&mut s, PickEvent::NeedsGpu).is_empty());
    assert_eq!(step_pick(&mut s, PickEvent::AdapterFound { software: true }).into_iter().collect::<Vec<_>>(), vec![PickEffect::RunWarmFrame]);
    assert_eq!(step_pick(&mut s, PickEvent::Ready).into_iter().collect::<Vec<_>>(), vec![PickEffect::SwitchToGpu]);

    // 3. Hardware adapter under idle defer -> RunWarmFrame then SwitchToGpu.
    let mut s = PickState::Pending { defer_software: true };
    assert_eq!(step_pick(&mut s, PickEvent::AdapterFound { software: false }).into_iter().collect::<Vec<_>>(), vec![PickEffect::RunWarmFrame]);
    assert_eq!(step_pick(&mut s, PickEvent::Ready).into_iter().collect::<Vec<_>>(), vec![PickEffect::SwitchToGpu]);
}

#[test]
fn needs_gpu_after_skipped_warm_frame_must_not_switch_to_unwarmed_software_gpu() {
    use std::sync::{Arc, Barrier};
    use std::sync::atomic::AtomicUsize;

    let pick = Arc::new(Pick::new(true));
    let warm_runs = Arc::new(AtomicUsize::new(0));
    let skipped = Arc::new(Barrier::new(2));
    let after_needs_gpu = Arc::new(Barrier::new(2));

    let (p_bg, w_bg, b1, b2) = (pick.clone(), warm_runs.clone(), skipped.clone(), after_needs_gpu.clone());
    let worker = std::thread::spawn(move || {
        // Worker finds software adapter while defer==true, skips warm_frame.
        if p_bg.step(PickEvent::AdapterFound { software: true }).into_iter().any(|e| e == PickEffect::RunWarmFrame) {
            w_bg.fetch_add(1, Ordering::Relaxed);
        }
        b1.wait();
        b2.wait();
    });

    // UI thread waits until worker has skipped warm_frame, then NeedsGpu arrives before gpu_ready.
    skipped.wait();
    for _ in pick.step(PickEvent::NeedsGpu) {}
    after_needs_gpu.wait();
    worker.join().unwrap();

    // gpu_ready MUST NOT switch to the unwarmed software GPU; it emits RestartForWarmFrame so
    // the worker runs warm_frame off-thread before switching.
    let ready_effects: Vec<_> = pick.step(PickEvent::Ready).into_iter().collect();
    assert_eq!(ready_effects, vec![PickEffect::RestartForWarmFrame]);
    assert_eq!(warm_runs.load(Ordering::Relaxed), 0);

    // Follow-up restart (start_gpu(false)): runs warm_frame off-thread, then SwitchToGpu.
    let restart = Pick::new(false);
    if restart.step(PickEvent::AdapterFound { software: true }).into_iter().any(|e| e == PickEffect::RunWarmFrame) {
        warm_runs.fetch_add(1, Ordering::Relaxed);
    }
    assert_eq!(restart.step(PickEvent::Ready).into_iter().collect::<Vec<_>>(), vec![PickEffect::SwitchToGpu]);
    assert_eq!(warm_runs.load(Ordering::Relaxed), 1);
}

#[test]
fn opaque_second_window_ignores_pooled_software_gpu_under_auto_automatic() {
    use super::super::cpu_frame::linux_opens_on_cpu;
    use wgpu::DeviceType::{Cpu, DiscreteGpu, IntegratedGpu, VirtualGpu};

    // Opaque second window when gpu_pool holds a software GPU (from a transparent window or
    // earlier NeedsGpu window): stays on CpuPainter under Auto + Automatic.
    assert!(linux_opens_on_cpu(CpuFirst::Auto, Policy::Automatic, false, false, Some(Cpu)));

    // Pinned adapter (WGPU_ADAPTER_NAME): reuses pooled software GPU (bit-identical).
    assert!(!linux_opens_on_cpu(CpuFirst::Auto, Policy::Pinned, false, false, Some(Cpu)));

    // Hardware GPUs in gpu_pool: reuses pooled hardware GPU (bit-identical).
    for hw in [IntegratedGpu, DiscreteGpu, VirtualGpu] {
        assert!(!linux_opens_on_cpu(CpuFirst::Auto, Policy::Automatic, false, false, Some(hw)), "{hw:?}");
        assert!(!linux_opens_on_cpu(CpuFirst::Auto, Policy::Pinned, false, false, Some(hw)), "{hw:?} pinned");
    }

    // Transparent windows, own GPU, CpuFirst::Off, and CpuFirst::Only remain unchanged.
    for mode in [CpuFirst::Auto, CpuFirst::Only, CpuFirst::Off] {
        for policy in [Policy::Automatic, Policy::Pinned] {
            for pool in [None, Some(Cpu), Some(DiscreteGpu)] {
                assert!(!linux_opens_on_cpu(mode, policy, true, false, pool), "transparent {mode:?} {policy:?} {pool:?}");
                assert!(!linux_opens_on_cpu(mode, policy, false, true, pool), "has_gpu {mode:?} {policy:?} {pool:?}");
            }
        }
    }
    assert!(!linux_opens_on_cpu(CpuFirst::Off, Policy::Automatic, false, false, None));
    assert!(!linux_opens_on_cpu(CpuFirst::Off, Policy::Automatic, false, false, Some(Cpu)));
    assert!(linux_opens_on_cpu(CpuFirst::Only, Policy::Automatic, false, false, Some(DiscreteGpu)));
    assert!(linux_opens_on_cpu(CpuFirst::Only, Policy::Automatic, false, false, Some(Cpu)));
}
