//! Rune: how long each submission holds the GPU, in debug builds, reported to a hook when one
//! passes a budget. Windows resets a GPU whose one submission runs 2 s (TDR), and some GPUs
//! (AMD's) preempt nothing smaller than a submission, so a test or a frame that asks for too
//! much in one submission hangs the whole GPU on a slow adapter and passes on a fast one.
//!
//! Each timed submission runs between two command lists of the watch's own that write GPU
//! timestamps (the queue's clock). The span is the GPU's time from the start of the submission's
//! first command list to the end of its last: a queue wait before it (on a video decoder's
//! fence) or another device's work queued ahead of it on the same GPU isn't counted. Work another
//! context runs while this one is preempted mid-submission still is. A queue without timestamps
//! is not timed, and says so once.
//!
//! Release builds compile none of this module (`mod.rs`): there, `Queue::watch_submissions`
//! does nothing and `Queue::submission_watch_serial` is 0.

use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, OnceLock,
    },
    time::Duration,
};

use parking_lot::Mutex;
use windows::Win32::{
    Foundation,
    Graphics::{Direct3D12, Dxgi},
    System::Threading,
};

/// Timestamp pairs the ring holds: a submission whose pair is still in flight goes untimed.
const RING: u32 = 64;

pub(super) static HOOK: OnceLock<(Duration, super::SubmissionReport)> = OnceLock::new();
static SERIAL: AtomicU64 = AtomicU64::new(1);

/// The timing lists to run around one submission, and the ring slot they write.
pub(super) struct Timed {
    slot: u32,
    pub(super) begin: Direct3D12::ID3D12CommandList,
    pub(super) end: Direct3D12::ID3D12CommandList,
}

pub(super) struct Watch {
    pub(super) serial: u64,
    /// WARP and other software adapters: not timed (see `Watch::new`).
    software: bool,
    /// `None` until the first timed submission; then the timer, or `None` if timestamps can't be had.
    timer: Mutex<Option<Option<Timer>>>,
}

impl Watch {
    /// A queue on a software adapter isn't timed: on WARP, a compute submission run between the
    /// watch's timestamp lists removed the device (`DXGI_ERROR_DRIVER_INTERNAL_ERROR`, 19 Nov,
    /// adais), while an empty one ran. A software adapter has no GPU for Windows to reset.
    pub(super) fn new(software: bool) -> Self {
        Watch { serial: SERIAL.fetch_add(1, Ordering::Relaxed), software, timer: Mutex::new(None) }
    }
}

struct Slot {
    begin: Direct3D12::ID3D12GraphicsCommandList,
    end: Direct3D12::ID3D12GraphicsCommandList,
    _allocator: Direct3D12::ID3D12CommandAllocator,
    /// The fence value of this slot's last submission.
    last: u64,
}

struct Timer {
    slots: Vec<Slot>,
    next: u32,
    _readback: Shared<Direct3D12::ID3D12Resource>,
    data: *mut u64,
    sender: mpsc::Sender<(u32, u64)>,
}

/// D3D12 interfaces are free-threaded.
struct Shared<T>(T);
unsafe impl<T> Send for Shared<T> {}

impl Watch {
    /// The lists to run around a submission that will signal `fence` with `value`, if it's timed.
    pub(super) fn before(&self, queue: &Direct3D12::ID3D12CommandQueue, fence: &Direct3D12::ID3D12Fence) -> Option<Timed> {
        let &(budget, report) = HOOK.get()?;
        let mut timer = self.timer.lock();
        let software = self.software;
        let timer = timer
            .get_or_insert_with(|| match if software { Err("a software adapter (WARP) has no GPU to reset, and timing a submission there removed the device".into()) } else { Timer::new(queue, fence, budget, report, self.serial) } {
                Ok(timer) => Some(timer),
                Err(why) => {
                    eprintln!("wgpu-hal (Rune): GPU submission timing is off on this D3D12 queue: {why}");
                    None
                }
            })
            .as_mut()?;
        let slot = timer.next;
        let completed = unsafe { fence.GetCompletedValue() };
        let s = &timer.slots[slot as usize];
        // Still in flight from RING submissions ago, or a removed device.
        if completed < s.last || completed == u64::MAX {
            return None;
        }
        timer.next = (slot + 1) % RING;
        unsafe {
            timer.data.add(2 * slot as usize).write_volatile(0);
            timer.data.add(2 * slot as usize + 1).write_volatile(0);
        }
        Some(Timed { slot, begin: s.begin.clone().into(), end: s.end.clone().into() })
    }

    /// `timed`'s submission was signalled with `value`: its watcher reads its times once done.
    pub(super) fn after(&self, timed: Timed, value: u64) {
        let mut timer = self.timer.lock();
        let Some(Some(timer)) = timer.as_mut() else { return };
        timer.slots[timed.slot as usize].last = value;
        let _ = timer.sender.send((timed.slot, value));
    }
}

impl Timer {
    fn new(
        queue: &Direct3D12::ID3D12CommandQueue,
        fence: &Direct3D12::ID3D12Fence,
        budget: Duration,
        report: super::SubmissionReport,
        serial: u64,
    ) -> Result<Timer, String> {
        let frequency = unsafe { queue.GetTimestampFrequency() }.map_err(|e| format!("no timestamp frequency ({e})"))?;
        if frequency == 0 {
            return Err("a timestamp frequency of 0".into());
        }
        let mut device = None::<Direct3D12::ID3D12Device>;
        unsafe { queue.GetDevice(&mut device) }.map_err(|e| format!("no device ({e})"))?;
        let device = device.ok_or("no device")?;
        let mut heap = None::<Direct3D12::ID3D12QueryHeap>;
        unsafe {
            device.CreateQueryHeap(
                &Direct3D12::D3D12_QUERY_HEAP_DESC { Type: Direct3D12::D3D12_QUERY_HEAP_TYPE_TIMESTAMP, Count: 2 * RING, NodeMask: 0 },
                &mut heap,
            )
        }
        .map_err(|e| format!("no timestamp query heap ({e})"))?;
        let heap = heap.ok_or("no timestamp query heap")?;
        let mut readback = None::<Direct3D12::ID3D12Resource>;
        unsafe {
            device.CreateCommittedResource(
                &Direct3D12::D3D12_HEAP_PROPERTIES { Type: Direct3D12::D3D12_HEAP_TYPE_READBACK, ..Default::default() },
                Direct3D12::D3D12_HEAP_FLAG_NONE,
                &Direct3D12::D3D12_RESOURCE_DESC {
                    Dimension: Direct3D12::D3D12_RESOURCE_DIMENSION_BUFFER,
                    Alignment: 0,
                    Width: 16 * u64::from(RING),
                    Height: 1,
                    DepthOrArraySize: 1,
                    MipLevels: 1,
                    Format: Dxgi::Common::DXGI_FORMAT_UNKNOWN,
                    SampleDesc: Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Layout: Direct3D12::D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                    Flags: Direct3D12::D3D12_RESOURCE_FLAG_NONE,
                },
                Direct3D12::D3D12_RESOURCE_STATE_COPY_DEST,
                None,
                &mut readback,
            )
        }
        .map_err(|e| format!("no readback buffer ({e})"))?;
        let readback = readback.ok_or("no readback buffer")?;
        let mut data = core::ptr::null_mut();
        unsafe { readback.Map(0, None, Some(&mut data)) }.map_err(|e| format!("no readback mapping ({e})"))?;
        let data = data.cast::<u64>();
        let slots = (0..RING)
            .map(|slot| {
                let allocator: Direct3D12::ID3D12CommandAllocator =
                    unsafe { device.CreateCommandAllocator(Direct3D12::D3D12_COMMAND_LIST_TYPE_DIRECT) }.map_err(|e| e.to_string())?;
                let list = |query: u32| -> Result<Direct3D12::ID3D12GraphicsCommandList, String> {
                    let list: Direct3D12::ID3D12GraphicsCommandList =
                        unsafe { device.CreateCommandList(0, Direct3D12::D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None) }.map_err(|e| e.to_string())?;
                    unsafe {
                        list.EndQuery(&heap, Direct3D12::D3D12_QUERY_TYPE_TIMESTAMP, query);
                        list.ResolveQueryData(&heap, Direct3D12::D3D12_QUERY_TYPE_TIMESTAMP, query, 1, &readback, 8 * u64::from(query));
                        list.Close()
                    }
                    .map_err(|e| e.to_string())?;
                    Ok(list)
                };
                Ok(Slot { begin: list(2 * slot)?, end: list(2 * slot + 1)?, _allocator: allocator, last: 0 })
            })
            .collect::<Result<Vec<_>, String>>()
            .map_err(|e| format!("no timing command lists ({e})"))?;
        let (sender, jobs) = mpsc::channel();
        let (fence, queue, shared) = (Shared(fence.clone()), Shared(queue.clone()), Shared(readback.clone()));
        let at = Shared(data);
        std::thread::Builder::new()
            .name("wgpu-dx12-submission-watch".into())
            .spawn(move || {
                let (fence, queue, _keep, data) = (fence, queue, shared, at);
                watch(&fence.0, &queue.0, data.0, frequency, budget, report, serial, jobs)
            })
            .map_err(|e| format!("no watch thread ({e})"))?;
        Ok(Timer { slots, next: 0, _readback: Shared(readback), data, sender })
    }
}

#[allow(clippy::too_many_arguments)]
fn watch(
    fence: &Direct3D12::ID3D12Fence,
    queue: &Direct3D12::ID3D12CommandQueue,
    data: *mut u64,
    frequency: u64,
    budget: Duration,
    report: super::SubmissionReport,
    serial: u64,
    jobs: mpsc::Receiver<(u32, u64)>,
) {
    let Ok(event) = super::Event::create(false, false) else { return };
    let span = |from: u64, to: u64| Duration::from_secs_f64(to.saturating_sub(from) as f64 / frequency as f64);
    while let Ok((slot, value)) = jobs.recv() {
        let read = |i: usize| unsafe { data.add(2 * slot as usize + i).read_volatile() };
        let mut told = false;
        loop {
            let completed = unsafe { fence.GetCompletedValue() };
            // A removed device: Rune's `health::removed` reports it, and nothing more runs.
            if completed == u64::MAX {
                return;
            }
            if completed >= value {
                let (begin, end) = (read(0), read(1));
                if !told && begin != 0 && end != 0 && span(begin, end) > budget {
                    report(serial, value, span(begin, end), false);
                }
                break;
            }
            if unsafe { fence.SetEventOnCompletion(value, event.0) }.is_err() {
                return;
            }
            let waited = unsafe { Threading::WaitForSingleObject(event.0, 50) };
            // Still running: its begin timestamp against the queue's clock now.
            let begin = read(0);
            let (mut gpu, mut cpu) = (0, 0);
            if waited == Foundation::WAIT_TIMEOUT && !told && begin != 0 && unsafe { queue.GetClockCalibration(&mut gpu, &mut cpu) }.is_ok() && span(begin, gpu) > budget {
                told = true;
                report(serial, value, span(begin, gpu), true);
            }
        }
    }
}
