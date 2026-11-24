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
}
