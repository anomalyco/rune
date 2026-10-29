//! Running an app's cells and reporting their findings against its allowlist.

use std::time::Instant;

use super::check::{Finding, RULES, Recording, check, dump, sheet};

/// A finding that is the design: `(kind, element contains, cell contains, why)`, the first three
/// matched as substrings ("" for any). Every row needs its reason.
pub type Allow = (&'static str, &'static str, &'static str, &'static str);

/// Builds and records one cell.
pub type Job = Box<dyn FnOnce() -> Recording + Send>;

/// One cell's findings.
pub struct Cell {
    pub name: String,
    pub findings: Vec<Finding>,
    pub frames: usize,
    /// Fewer than 10 frames were recorded: its setup didn't apply.
    pub skipped: bool,
}

/// Why `f` in `cell` is allowed, if a row of `allow` matches it.
pub fn allowed(allow: &[Allow], f: &Finding, cell: &str) -> Option<&'static str> {
    allow.iter().find(|a| f.kind == a.0 && f.element.contains(a.1) && cell.contains(a.2)).map(|a| a.3)
}

/// Runs every cell whose name contains `filter`, in parallel on large stacks, and checks each
/// recording, in the order given. `MOTION_GATE_DUMP` and `MOTION_GATE_SHEET` write evidence for
/// the cell they name.
pub fn run(cells: Vec<(String, Job)>, filter: &str) -> Vec<Cell> {
    let jobs: Vec<(usize, (String, Job))> = cells.into_iter().filter(|c| c.0.contains(filter)).enumerate().collect();
    let jobs = std::sync::Mutex::new(jobs);
    let out = std::sync::Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(12);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            std::thread::Builder::new()
                .stack_size(64 << 20)
                .spawn_scoped(scope, || loop {
                    let Some((i, (name, job))) = jobs.lock().unwrap().pop() else { break };
                    let rec = job();
                    evidence(&name, &rec);
                    let cell = Cell { skipped: rec.frames < 10, frames: rec.frames, findings: check(&rec), name };
                    out.lock().unwrap().push((i, cell));
                })
                .unwrap();
        }
    });
    let mut cells = out.into_inner().unwrap();
    cells.sort_by_key(|c| c.0);
    cells.into_iter().map(|c| c.1).collect()
}

#[expect(clippy::disallowed_methods, reason = "reads MOTION_GATE_* output flags in test harness")]
fn evidence(cell: &str, rec: &Recording) {
    let out = std::env::var("MOTION_GATE_OUT").ok();
    if let Ok(spec) = std::env::var("MOTION_GATE_DUMP") {
        let mut p = spec.split('|');
        if let (Some(c), Some(path)) = (p.next(), p.next())
            && c == cell
        {
            let from = p.next().and_then(|x| x.parse().ok()).unwrap_or(0);
            let to = p.next().and_then(|x| x.parse().ok()).unwrap_or(usize::MAX);
            let text = dump(rec, path, from, to);
            match &out {
                Some(out) => std::fs::write(format!("{out}.dump"), text).unwrap(),
                None => eprint!("{text}"),
            }
        }
    }
    if let Ok(spec) = std::env::var("MOTION_GATE_SHEET") {
        let mut p = spec.split('|');
        if let (Some(c), Some(Ok(at))) = (p.next(), p.next().map(str::parse))
            && c == cell
        {
            let out = out.unwrap_or_else(|| std::env::temp_dir().join("motion-gate").display().to_string());
            sheet(rec, at, p.next().unwrap_or("")).save(format!("{out}.sheet.png")).unwrap();
        }
    }
}

/// What a run found.
pub struct Report {
    /// The summary line, then a line per finding that isn't allowed (and per allowed one, when
    /// asked), then what each kind found means.
    pub text: String,
    pub summary: String,
    /// Findings that aren't allowed, plus skipped cells.
    pub failing: usize,
    pub allowed: usize,
}

/// Reads `cells` against `allow`; `list_allowed` lists allowed findings too.
pub fn report(cells: &[Cell], allow: &[Allow], list_allowed: bool, secs: f32) -> Report {
    let mut text = String::new();
    let (mut failing, mut allowed_n) = (0, 0);
    for c in cells {
        if c.skipped {
            failing += 1;
            text += &format!("SKIP {} (its setup didn't apply)\n", c.name);
        }
        for f in &c.findings {
            match allowed(allow, f, &c.name) {
                Some(why) => {
                    allowed_n += 1;
                    if list_allowed {
                        text += &format!("allowed  {}  in {}  ({why})\n", f, c.name);
                    }
                }
                None => {
                    failing += 1;
                    text += &format!("{}  in {}\n", f, c.name);
                }
            }
        }
    }
    let frames: usize = cells.iter().map(|c| c.frames).sum();
    let summary = format!("motion gate: {} cells, {frames} frames, {failing} findings ({allowed_n} allowed) in {secs:.1} s\n", cells.len());
    let mut kinds: Vec<&str> = cells.iter().flat_map(|c| c.findings.iter().filter(|f| allowed(allow, f, &c.name).is_none()).map(|f| f.kind)).collect();
    kinds.sort();
    kinds.dedup();
    for r in RULES.iter().filter(|r| kinds.contains(&r.kind)) {
        text += &format!("  {}: {} ({})\n", r.kind, r.means, r.threshold);
    }
    Report { text: summary.clone() + &text, summary, failing, allowed: allowed_n }
}

/// The gate as a test: runs the cells `MOTION_GATE` keeps, writes the report to
/// `MOTION_GATE_OUT`, and panics with it when a finding isn't allowed.
#[expect(clippy::disallowed_methods, reason = "reads MOTION_GATE filter and output flags")]
pub fn gate(cells: Vec<(String, Job)>, allow: &[Allow]) {
    let filter = std::env::var("MOTION_GATE").unwrap_or_default();
    let started = Instant::now();
    let cells = run(cells, &filter);
    let r = report(&cells, allow, std::env::var_os("MOTION_GATE_ALLOWED").is_some(), started.elapsed().as_secs_f32());
    if let Ok(path) = std::env::var("MOTION_GATE_OUT") {
        std::fs::write(path, &r.text).unwrap();
    }
    assert!(r.failing == 0, "{}", r.text);
    eprint!("{}", r.summary);
}
