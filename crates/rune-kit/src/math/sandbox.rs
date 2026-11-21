//! Native off-UI-thread `wasmi` sandbox worker for `ratex-math.wasm`.
//!
//! - Zero startup cost: the worker thread, deflated `.wasm` inflation, `wasmi::Engine`,
//!   `wasmi::Module`, and KaTeX font pack are initialized lazily on the first closed math fence.
//! - Hard host-enforced limits: 16 KiB input cap, 40M instruction fuel cap, 16 MiB linear-memory
//!   cap (`StoreLimitsBuilder`), bounded recursion/value stack, 250 ms wall-clock deadline, and
//!   full `RM01` wire validation before host rasterisation.
//! - Any trap, OOM, fuel exhaustion, deadline breach, or wire error drops the guest `Store` and
//!   recovers cleanly on the next formula while caching the failed formula as `Failed`.

use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use rune::Color;
use wasmi::{Config, Engine, Instance, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};

use super::raster::{self, Rasterized};
use super::wire::{self, MAX_WIRE_BYTES};
use super::{MathKey, finish_job};

/// Maximum TeX input bytes accepted by the host before invoking the guest.
pub(super) const MAX_INPUT_BYTES: usize = 16 * 1024;
/// Maximum `wasmi` instruction fuel per formula (~20–35 ms worst-case on worker).
const FUEL_BUDGET: u64 = 40_000_000;
/// Maximum WebAssembly linear memory (16 MiB) enforced outside the guest.
const MAX_LINEAR_MEMORY: usize = 16 * 1024 * 1024;
/// Reset the healthy guest instance if its linear memory grew past 8 MiB to keep worker RSS tight.
const RECYCLE_MEMORY_BYTES: usize = 8 * 1024 * 1024;
/// Maximum wall-clock duration allowed for one formula (parse + layout + decode + raster).
const WALL_DEADLINE: Duration = Duration::from_millis(250);
/// Hard ceiling when inflating `ratex-math.wasm.deflate` (actual `.wasm` is ~810 KiB).
const MAX_WASM_INFLATE_BYTES: usize = 4 * 1024 * 1024;
/// Maximum queued background jobs so rapid scrolling over adversarial messages never backs up RAM.
const MAX_QUEUED_JOBS: usize = 64;

static RATEX_WASM_DEFLATE: &[u8] = include_bytes!("../../assets/math/ratex-math.wasm.deflate");

struct HostState {
    limits: StoreLimits,
}

struct Job {
    key: MathKey,
    em_pt: f32,
    ink: Color,
    scale: f32,
}

pub(super) struct Sandbox {
    engine: Engine,
    module: Module,
    slot: Option<(Store<HostState>, Instance)>,
}

impl Sandbox {
    pub(super) fn new() -> Option<Self> {
        let wasm = miniz_oxide::inflate::decompress_to_vec_with_limit(
            RATEX_WASM_DEFLATE,
            MAX_WASM_INFLATE_BYTES,
        )
        .ok()?;
        let mut cfg = Config::default();
        cfg.consume_fuel(true);
        cfg.set_max_recursion_depth(256);
        let _ = cfg.set_max_stack_height(512 * 1024);
        cfg.set_max_cached_stacks(1);
        let engine = Engine::new(&cfg);
        let module = Module::new(&engine, &wasm).ok()?;
        Some(Self { engine, module, slot: None })
    }

    fn fresh_instance(&self) -> Option<(Store<HostState>, Instance)> {
        let limits = StoreLimitsBuilder::new()
            .memory_size(MAX_LINEAR_MEMORY)
            .table_elements(1024)
            .instances(1)
            .memories(1)
            .build();
        let mut store = Store::new(&self.engine, HostState { limits });
        store.limiter(|s| &mut s.limits);
        store.set_fuel(FUEL_BUDGET).ok()?;
        let linker = Linker::new(&self.engine);
        let inst = linker.instantiate_and_start(&mut store, &self.module).ok()?;
        Some((store, inst))
    }

    /// Runs `tex` inside the `wasmi` sandbox and returns the raw (still untrusted) `RM01` wire bytes.
    pub(super) fn eval_wire(&mut self, tex: &str) -> Option<Vec<u8>> {
        if tex.is_empty() || tex.len() > MAX_INPUT_BYTES {
            return None;
        }
        if self.slot.is_none() {
            self.slot = self.fresh_instance();
        }
        let (store, inst) = self.slot.as_mut()?;
        if store.set_fuel(FUEL_BUDGET).is_err() {
            self.slot = None;
            return None;
        }
        let alloc = inst.get_typed_func::<u32, u32>(&*store, "alloc_input").ok()?;
        let render = inst.get_typed_func::<u32, u32>(&*store, "render_tex").ok()?;
        let out_ptr_fn = inst.get_typed_func::<(), u32>(&*store, "out_ptr").ok()?;
        let mem = inst.get_memory(&*store, "memory")?;

        let ptr = match alloc.call(&mut *store, tex.len() as u32) {
            Ok(0) => return None,
            Ok(p) => p,
            Err(_) => {
                self.slot = None;
                return None;
            }
        };
        if mem.write(&mut *store, ptr as usize, tex.as_bytes()).is_err() {
            self.slot = None;
            return None;
        }
        let out_len = match render.call(&mut *store, tex.len() as u32) {
            Ok(0) => {
                if mem.data_size(&*store) > RECYCLE_MEMORY_BYTES {
                    self.slot = None;
                }
                return None;
            }
            Ok(n) if (n as usize) <= MAX_WIRE_BYTES => n as usize,
            Ok(_) | Err(_) => {
                self.slot = None;
                return None;
            }
        };
        let Ok(op) = out_ptr_fn.call(&mut *store, ()) else {
            self.slot = None;
            return None;
        };
        let mut out = vec![0u8; out_len];
        if mem.read(&*store, op as usize, &mut out).is_err() {
            self.slot = None;
            return None;
        }
        if mem.data_size(&*store) > RECYCLE_MEMORY_BYTES {
            self.slot = None;
        }
        Some(out)
    }

    pub(super) fn render_one(&mut self, tex: &str, em_pt: f32, ink: Color, scale: f32) -> Option<Rasterized> {
        let started = Instant::now();
        let bytes = self.eval_wire(tex)?;
        if started.elapsed() > WALL_DEADLINE {
            return None;
        }
        let dl = wire::decode(&bytes)?;
        if started.elapsed() > WALL_DEADLINE {
            return None;
        }
        let rast = raster::rasterize(&dl, em_pt, ink, scale)?;
        if started.elapsed() > WALL_DEADLINE {
            return None;
        }
        Some(rast)
    }
}

static WORKER: OnceLock<Option<mpsc::SyncSender<Job>>> = OnceLock::new();

fn worker_tx() -> Option<&'static mpsc::SyncSender<Job>> {
    WORKER
        .get_or_init(|| {
            let (tx, rx) = mpsc::sync_channel::<Job>(MAX_QUEUED_JOBS);
            std::thread::Builder::new()
                .name("rune-math".into())
                .spawn(move || {
                    let mut sandbox = Sandbox::new();
                    while let Ok(job) = rx.recv() {
                        let res = sandbox
                            .as_mut()
                            .and_then(|sb| sb.render_one(&job.key.src, job.em_pt, job.ink, job.scale));
                        finish_job(job.key, res);
                    }
                })
                .ok()?;
            Some(tx)
        })
        .as_ref()
}

pub(super) fn enqueue(key: MathKey, em_pt: f32, ink: Color, scale: f32) -> bool {
    let Some(tx) = worker_tx() else {
        return false;
    };
    tx.try_send(Job { key, em_pt, ink, scale }).is_ok()
}
