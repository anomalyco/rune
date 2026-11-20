use crate::{FakeMicrophone, Recorder};
use std::path::PathBuf;

/// A temporary directory, removed on drop.
struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rune-recorder-rt-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn runtime() -> crate::Runtime<()> {
    crate::Runtime::new((), |_: &(), _: &mut crate::Cx<()>| crate::el().size(10.0, 10.0), (10.0, 10.0), crate::Env::new(true, 2.0))
}

#[track_caller]
fn near(got: f64, want: f64, tolerance: f64) {
    assert!((got - want).abs() <= tolerance, "got {got}, want {want}");
}

#[test]
fn a_new_runtime_restarts_elapsed_from_its_own_clock() {
    FakeMicrophone::silence().install();
    let dir = Scratch::new("clock");
    let mut old = runtime();
    old.frame(5.0);
    let rec = Recorder::to_file(dir.join("note.wav"));
    rec.start();
    let mut rt = runtime();
    rt.frame(0.1);
    rt.frame(0.1);
    near(rec.elapsed(), 0.2, 1e-9);
}

/// A bound meter shows this frame's level in this frame, and draws nothing while idle.
#[test]
fn a_bound_meter_follows_the_level_on_the_same_frame() {
    let dir = Scratch::new("bound");
    FakeMicrophone::sine(1000.0, 0.5).install();
    let rec = Recorder::to_file(dir.join("a.wav"));
    let view = |r: &Recorder, _: &mut crate::Cx<Recorder>| {
        crate::el().key("meter").size(10.0, 10.0).fill(crate::Color::WHITE).live_opacity(&r.live_level())
    };
    let mut rt = crate::Runtime::new(rec.clone(), view, (10.0, 10.0), crate::Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(rt.presented("meter").unwrap().1, 0.0);
    assert!(!rt.frame(0.1), "an idle recorder asks for no frames");
    rec.start();
    rt.frame(1.0);
    near(rt.presented("meter").unwrap().1 as f64, rec.level().meter() as f64, 1e-6);
    assert!(rec.level().meter() > 0.8);
    rec.stop().unwrap();
    rt.frame(0.1);
    assert_eq!(rt.presented("meter").unwrap().1, 0.0);
}
