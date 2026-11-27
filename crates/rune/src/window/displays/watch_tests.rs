use super::*;

fn display(name: &str, position: (i32, i32), size: (u32, u32), scale: f64, primary: bool) -> Display {
    Display {
        id: None,
        name: Some(name.into()),
        position,
        size,
        scale,
        primary,
    }
}

#[test]
fn display_changes_are_told_as_removals_then_changes_then_additions() {
    let a = display("DP-1", (0, 0), (2560, 1440), 1.0, true);
    let b = display("HDMI-1", (2560, 0), (1920, 1080), 1.0, false);
    let c_old = display("DP-2", (4480, 0), (3840, 2160), 2.0, false);

    let b_changed = display("HDMI-1", (0, 0), (1080, 1920), 1.5, true);
    let d_added = display("eDP-1", (1080, 0), (2880, 1800), 2.0, false);

    let events = diff_displays(&[a.clone(), b, c_old.clone()], &[d_added.clone(), b_changed.clone()]);
    assert_eq!(
        events,
        vec![
            DisplayEvent::Removed(a),
            DisplayEvent::Removed(c_old),
            DisplayEvent::Changed(b_changed),
            DisplayEvent::Added(d_added),
        ],
    );
}

#[test]
fn diff_displays_emits_nothing_when_unchanged() {
    let list = vec![
        display("DP-1", (0, 0), (2560, 1440), 1.25, true),
        display("DP-2", (2048, 0), (1920, 1080), 1.0, false),
    ];
    assert!(diff_displays(&list, &list).is_empty());
}

#[test]
fn diff_displays_detects_scale_size_position_and_primary_changes() {
    let base = display("DP-1", (0, 0), (2560, 1440), 1.0, true);
    let cases = [
        display("DP-1", (100, 0), (2560, 1440), 1.0, true),
        display("DP-1", (0, 0), (1440, 2560), 1.0, true),
        display("DP-1", (0, 0), (2560, 1440), 1.5, true),
        display("DP-1", (0, 0), (2560, 1440), 1.0, false),
    ];
    for changed in cases {
        assert_eq!(
            diff_displays(&[base.clone()], &[changed.clone()]),
            vec![DisplayEvent::Changed(changed)],
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn display_watch_updates_remembered_displays_before_invoking_hooks() {
    use std::cell::RefCell;
    use std::rc::Rc;

    crate::window::displays::reset();
    let d1 = display("DP-1", (0, 0), (1920, 1080), 1.0, true);
    let d2 = display("DP-2", (1920, 0), (2560, 1440), 1.5, false);
    crate::window::displays::remember(&[d1.clone()]);

    let seen: Rc<RefCell<Vec<(DisplayEvent, Vec<Display>, Vec<Display>)>>> = Rc::default();
    let seen_cb = seen.clone();
    let mut watch = DisplayWatch::test_with_hook(vec![d1.clone()], move |event, all| {
        seen_cb.borrow_mut().push((event.clone(), all.to_vec(), crate::displays()));
    });

    // Identical snapshot does not fire the hook.
    watch.apply_snapshot(vec![d1.clone()]);
    assert!(seen.borrow().is_empty());

    // Added display updates rune::displays() before the callback runs.
    watch.apply_snapshot(vec![d1.clone(), d2.clone()]);
    let records = seen.borrow();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].0, DisplayEvent::Added(d2.clone()));
    assert_eq!(records[0].1, vec![d1.clone(), d2.clone()]);
    assert_eq!(records[0].2, vec![d1, d2], "rune::displays() must already reflect the new snapshot");

    drop(records);
    watch.shutdown();
    crate::window::displays::reset();
}

#[cfg(target_os = "windows")]
#[test]
#[allow(unsafe_code)]
fn win32_display_watch_receives_wm_displaychange_and_coalesces() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_SETWORKAREA, SendMessageW, WM_DISPLAYCHANGE, WM_SETTINGCHANGE,
    };

    let wakes = Arc::new(AtomicUsize::new(0));
    let wakes_cb = wakes.clone();
    let watch = win32::Win32DisplayWatch::new(Arc::new(move || {
        wakes_cb.fetch_add(1, Ordering::Relaxed);
    }))
    .expect("create hidden Win32 display watch window");

    // SAFETY: `watch.hwnd()` is a live window owned by this thread.
    unsafe {
        SendMessageW(watch.hwnd(), WM_DISPLAYCHANGE, 32, (1080 << 16) | 1920);
        // Back-to-back broadcasts coalesce into one wake until clear_queued() runs.
        SendMessageW(watch.hwnd(), WM_DISPLAYCHANGE, 32, (1440 << 16) | 2560);
        SendMessageW(watch.hwnd(), WM_SETTINGCHANGE, SPI_SETWORKAREA as usize, 0);
    }
    assert_eq!(wakes.load(Ordering::Relaxed), 1);

    watch.clear_queued();
    // SAFETY: `watch.hwnd()` is still live.
    unsafe {
        SendMessageW(watch.hwnd(), WM_SETTINGCHANGE, SPI_SETWORKAREA as usize, 0);
    }
    assert_eq!(wakes.load(Ordering::Relaxed), 2);
}
