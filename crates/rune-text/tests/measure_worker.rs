//! Worker threads that never build a `TextSystem` inherit the process-wide fonts and follow
//! `set_families` from unguarded `TextSystem`s, while `with_fonts` guards stay thread-local.

use rune_core::tok::TextStyle;
use rune_text::{TextSystem, font_epoch, measure, with_fonts};

const INTER: &[u8] = include_bytes!("../../rune/assets/Inter-Regular.ttf");
const PLEX: &[u8] = include_bytes!("../../rune/assets/IBMPlexMono-Regular.ttf");

#[test]
fn worker_thread_inherits_fonts_and_follows_set_families() {
    let style = TextStyle::new(14.0, 400.0);
    let mono = style.mono();

    // Before any TextSystem is built, with_fonts must not mutate process-wide FONTS or GLOBAL_EPOCH.
    {
        let _g = with_fonts(&[PLEX], ("IBM Plex Mono", "IBM Plex Mono"));
        let _ = measure("Hello", &[], style, None);
        let worker_epoch = std::thread::spawn(font_epoch).join().unwrap();
        assert_eq!(worker_epoch, 0, "with_fonts must not seed or bump the process-wide epoch");
    }

    // Main thread builds a TextSystem with Inter + IBM Plex Mono, starting with Inter in both slots.
    let mut ts = TextSystem::new(&[INTER, PLEX], ("Inter", "Inter"));
    let inter_w = measure("iiiiiiiiii", &[], mono, None).0;
    let main_epoch = font_epoch();

    // A worker thread that never builds a TextSystem inherits the process-wide fonts and families.
    let (worker_before, worker_epoch_before) = std::thread::spawn(move || {
        (measure("iiiiiiiiii", &[], mono, None).0, font_epoch())
    })
    .join()
    .unwrap();
    assert_eq!(worker_before, inter_w, "worker thread inherits TextSystem::new fonts and families");
    assert_ne!(worker_epoch_before, 0);
    assert_ne!(worker_epoch_before, main_epoch, "main thread's own epoch is scoped separately from the global epoch");

    // Switching families on the main thread moves both the main thread and worker threads.
    assert!(ts.set_families("Inter", "IBM Plex Mono"));
    let plex_w = measure("iiiiiiiiii", &[], mono, None).0;
    assert_ne!(plex_w, inter_w, "IBM Plex Mono measures wider than proportional Inter");
    assert_ne!(font_epoch(), main_epoch);

    let (worker_after, worker_epoch_after) = std::thread::spawn(move || {
        (measure("iiiiiiiiii", &[], mono, None).0, font_epoch())
    })
    .join()
    .unwrap();
    assert_eq!(worker_after, plex_w, "worker thread follows set_families");
    assert_ne!(worker_epoch_after, worker_epoch_before, "worker thread sees updated global font_epoch");

    // A guarded TextSystem::new and set_families on the main thread must not move worker threads.
    {
        let _g = with_fonts(&[INTER], ("Inter", "Inter"));
        let mut guarded = TextSystem::default();
        guarded.set_families("Inter", "Inter");
        let (worker_guarded, worker_epoch_guarded) = std::thread::spawn(move || {
            (measure("iiiiiiiiii", &[], mono, None).0, font_epoch())
        })
        .join()
        .unwrap();
        assert_eq!(worker_guarded, plex_w, "guarded notes must not change worker thread measure");
        assert_eq!(worker_epoch_guarded, worker_epoch_after, "guarded notes must not change global font_epoch");
    }

    // Two unguarded TextSystem threads calling set_emoji_family concurrently stay isolated from
    // each other's measure() and font_epoch(), while a worker thread that never built a TextSystem
    // follows unguarded set_emoji_family changes.
    let ready = std::sync::Arc::new(std::sync::Barrier::new(2));
    let done = std::sync::Arc::new(std::sync::Barrier::new(2));
    let r1 = ready.clone();
    let d1 = done.clone();
    let thread_a = std::thread::spawn(move || {
        let mut a = TextSystem::new(&[INTER], ("Inter", "ui-monospace"));
        assert!(a.set_emoji_family(Some("Inter")));
        let ep_before = font_epoch();
        let w_before = measure("Hello world", &[], style, None).0;
        r1.wait();
        d1.wait();
        (w_before, measure("Hello world", &[], style, None).0, ep_before, font_epoch())
    });
    let thread_b = std::thread::spawn(move || {
        ready.wait();
        let mut b = TextSystem::new(&[INTER, PLEX], ("Inter", "IBM Plex Mono"));
        let ep0 = font_epoch();
        assert!(b.set_emoji_family(Some("IBM Plex Mono")));
        let ep1 = font_epoch();
        assert!(b.set_emoji_family(None));
        let ep2 = font_epoch();
        done.wait();
        (ep0, ep1, ep2)
    });
    let (w_before, w_after, ep_before, ep_after) = thread_a.join().unwrap();
    let (ep0, ep1, ep2) = thread_b.join().unwrap();
    assert_eq!(w_before, w_after, "another thread's unguarded set_emoji_family disturbed measure");
    assert_eq!(ep_before, ep_after, "another thread's unguarded set_emoji_family disturbed font_epoch");
    assert_ne!(ep0, ep1, "set_emoji_family must advance its own thread's font_epoch");
    assert_ne!(ep1, ep2, "clearing emoji_family must advance its own thread's font_epoch");
}
