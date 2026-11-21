//! What each pipeline set Rune, rune-fx and rune-light build costs the first time a process
//! makes it, in milliseconds, as one JSON line. Run it in fresh processes, `--profile perf`,
//! with `RUNE_GPU_BACKEND=metal` (and Rune's `metal-backend` feature) or `wgpu`, after clearing
//! the Metal shader cache (`$(getconf DARWIN_USER_CACHE_DIR)/com.apple.metal`) for a cold run.

#[path = "../tests/support/builtin.rs"]
mod builtin;

fn main() {
    let start = rune::time::now();
    let gpu = rune::render::Gpu::headless();
    let mut out = vec![("device", start.elapsed().as_secs_f64() * 1e3)];
    out.extend(builtin::build_every_pipeline(&gpu));
    let fields: Vec<String> = out.iter().map(|(k, v)| format!("\"{k}\":{v:.3}")).collect();
    println!("{{{}}}", fields.join(","));
}
