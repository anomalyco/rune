//! `RUNE_LOOP_CHECK=1` in a headless run: every frame no script event reached asks for a
//! loop-only step (`Runtime::loop_step`) before the ordinary frame, on the virtual clock, on any
//! platform. A step that could run is compared with the ordinary frame that runs instead
//! (`Runtime::loop_checked`: the instances that differ and by how much at most); a step that
//! couldn't says why. The run ends with the tally on stderr, and fails (exit 3) when it compared no step or any
//! step differed. `RUNE_LOOP_CHECK_LOG=<file>`
//! writes one line per frame asked (`{"t", "input", "why", "differ", "worst"}`).

use std::collections::BTreeMap;
use std::io::Write as _;

#[derive(Default)]
pub(super) struct Tally {
    checked: usize,
    differ: usize,
    most: usize,
    worst: f32,
    why: BTreeMap<(bool, &'static str), usize>,
    log: Option<std::io::BufWriter<std::fs::File>>,
    opened: bool,
}

impl Tally {
    /// The step asked before the frame at `t` (seconds), after a script's event or not, and what
    /// `loop_checked` found after it.
    pub(super) fn add(&mut self, t: f64, input: bool, asked: Result<Vec<u32>, &'static str>, checked: Option<(usize, f32)>) {
        if !self.opened {
            self.opened = true;
            self.log = std::env::var_os("RUNE_LOOP_CHECK_LOG").and_then(|p| std::fs::File::create(p).ok()).map(std::io::BufWriter::new);
        }
        let why = match asked { Ok(_) => "stepped", Err(why) => why };
        *self.why.entry((input, why)).or_default() += 1;
        if let Some((n, d)) = checked {
            self.checked += 1;
            if n > 0 { self.differ += 1; }
            self.most = self.most.max(n);
            self.worst = self.worst.max(d);
        }
        if let Some(log) = self.log.as_mut() {
            let (n, d) = checked.map_or((serde_json::Value::Null, serde_json::Value::Null), |(n, d)| (n.into(), if d.is_finite() { f64::from(d).into() } else { "inf".into() }));
            let _ = writeln!(log, "{}", serde_json::json!({ "t": t, "input": input, "why": why, "differ": n, "worst": d }));
        }
    }

    /// Prints the tally, and whether the check passed: it compared at least one step and none
    /// differed. A run whose every frame was refused passes nothing. False when it failed (the
    /// run then exits non-zero); true when nothing asked (`RUNE_LOOP_CHECK` unset).
    pub(super) fn report(&mut self) -> bool {
        if !self.opened { return true; }
        if let Some(log) = self.log.as_mut() { let _ = log.flush(); }
        let why: Vec<String> = self.why.iter().map(|((input, w), n)| format!("{}{w}: {n}", if *input { "after an event, " } else { "" })).collect();
        eprintln!("rune: loop check: {} steps checked against ordinary frames, {} differ (at most {} instances, largest difference {}); asked: {}",
            self.checked, self.differ, self.most, self.worst, why.join(", "));
        let fail = if self.checked == 0 { Some("no step was checked (every frame was refused: see why above)".to_owned()) }
            else if self.differ > 0 { Some(format!("{} steps differ from the ordinary frame", self.differ)) } else { None };
        match &fail {
            Some(why) => eprintln!("rune: loop check FAILED: {why}"),
            None => eprintln!("rune: loop check passed"),
        }
        fail.is_none()
    }
}
