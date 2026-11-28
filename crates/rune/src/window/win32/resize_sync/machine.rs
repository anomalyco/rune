//! Explicit state machine and `WM_NCCALCSIZE` `WVR_VALIDRECTS` geometry for Windows
//! synchronized drag resize.
//!
//! Pure data in and out: no Win32 handles, no GPU objects, and no side effects. The host turns
//! returned [`Effect`]s into GPU/Win32 work and feeds outcomes and timers back as [`Event`]s.

use std::time::Duration;

use crate::machine::{Attempt, Effects, Seq};
use crate::time::Instant;

/// Maximum time `State::Leaving` waits for a normal D3D12 flip-model frame to present and
/// complete after `WM_EXITSIZEMOVE` before re-attaching DirectComposition unconditionally (for
/// example if the window was minimized or covered as the drag ended).
pub(crate) const LEAVING_TIMEOUT: Duration = Duration::from_millis(500);

/// Minimum number of DWM desktop compositions (`DWM_TIMING_INFO::cFrame`) that must elapse
/// after the first copy-model `Present(0, 0)` before detaching the DirectComposition root.
///
/// Copy-model `DXGI_SWAP_EFFECT_DISCARD` presents update the HWND redirection surface through
/// DWM outside the D3D11 command queue, so a post-present D3D11 fence alone does not prove
/// DWM has composed the new drag frame over the previous drag's redirection bitmap. Waiting
/// for two composed DWM frames guarantees the previous drag's contents never flash on detach.
pub(crate) const DETACH_DWM_FRAMES: u64 = 2;

/// DirectComposition detachment phase while [`State::Dragging`] is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DcompPhase {
    /// First drag frame presented into the copy-model HWND swap chain at DWM frame
    /// `presented_at`; DirectComposition stays attached until `cFrame` advances by at least
    /// [`DETACH_DWM_FRAMES`].
    Priming { presented_at: u64 },
    /// DirectComposition root detached; the copy-model HWND redirection surface is live.
    Detached,
}

/// Whether a window is eligible to use the opt-in synchronized drag resize path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Eligibility {
    /// `RUNE_RESIZE_SYNC` opt-in is enabled.
    pub opt_in: bool,
    /// Opaque app window (`!transparent`, so no `WS_EX_NOREDIRECTIONBITMAP`).
    pub opaque: bool,
    /// Overlay windows never use drag resize sync.
    pub overlay: bool,
    /// DirectComposition presentation is active (`DxgiFromVisual`, not `DxgiFromHwnd`).
    pub dxgi_from_visual: bool,
    /// Settled `SurfaceMode::Handed` owning a DirectComposition `Handover` (explicitly excludes
    /// `SurfaceMode::Gpu`, `Cpu`, `HandingOver`, and `Rebinding`).
    pub handed_over: bool,
    /// Live D3D12 device is attached.
    pub d3d12_live: bool,
    /// Hardware adapter (excludes software/WARP `wgpu::DeviceType::Cpu`).
    pub hardware_adapter: bool,
    /// Supported 8-bit swap-chain format (excludes 10-bit/16-bit HDR formats).
    pub supported_format: bool,
}

impl Eligibility {
    /// True only when every precondition holds.
    pub(crate) const fn allowed(self) -> bool {
        self.opt_in
            && self.opaque
            && !self.overlay
            && self.dxgi_from_visual
            && self.handed_over
            && self.d3d12_live
            && self.hardware_adapter
            && self.supported_format
    }
}

/// Phase of a window's synchronized drag resize lifecycle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum State {
    /// Normal DirectComposition flip-model presentation; zero drag GPU objects alive.
    #[default]
    Idle,
    /// Inside `WM_ENTERSIZEMOVE..WM_EXITSIZEMOVE`, waiting for the first eligible resize step.
    /// Pure moves (dragging the title bar) leave directly to `Idle` with zero GPU work.
    Entering,
    /// Synchronized drag resize active: drag frames render into a D3D12 shared texture and
    /// present on a D3D11 `DXGI_SWAP_EFFECT_DISCARD` HWND swap chain inside `WM_PAINT`
    /// (`BeginPaint`/`EndPaint`), with zero presents to the hidden D3D12 composition swap chain.
    /// DirectComposition stays attached during [`DcompPhase::Priming`] until DWM has composed at
    /// least [`DETACH_DWM_FRAMES`] frames after the first drag present, then detaches.
    Dragging { size: (u32, u32), dcomp: DcompPhase },
    /// `WM_EXITSIZEMOVE` received: awaiting one completed normal D3D12 flip-model frame before
    /// re-attaching DirectComposition and freeing drag textures and the D3D11 swap chain.
    Leaving { attempt: Attempt, deadline: Instant },
}

impl State {
    /// Whether the window is actively presenting drag frames through the D3D11 copy-model path.
    pub(crate) const fn is_dragging(self) -> bool {
        matches!(self, State::Dragging { .. })
    }

    /// Whether the window is still waiting for DWM to compose [`DETACH_DWM_FRAMES`] frames after
    /// the first drag present before detaching DirectComposition.
    pub(crate) const fn is_priming(self) -> bool {
        matches!(self, State::Dragging { dcomp: DcompPhase::Priming { .. }, .. })
    }

    /// Whether the window is waiting for its first post-drag D3D12 flip-model frame to complete.
    pub(crate) const fn is_leaving(self) -> bool {
        matches!(self, State::Leaving { .. })
    }

    /// The `Leaving` safety timer deadline, when armed.
    pub(crate) const fn deadline(self) -> Option<Instant> {
        match self {
            State::Leaving { deadline, .. } => Some(deadline),
            State::Idle | State::Entering | State::Dragging { .. } => None,
        }
    }

    /// The `Leaving` safety timer attempt token, when armed.
    pub(crate) const fn leaving_attempt(self) -> Option<Attempt> {
        match self {
            State::Leaving { attempt, .. } => Some(attempt),
            State::Idle | State::Entering | State::Dragging { .. } => None,
        }
    }
}

/// Input event fed into [`step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// `WM_ENTERSIZEMOVE` entered the system modal size-move loop.
    EnterSizeMove { eligibility: Eligibility },
    /// A resize step arrived inside the modal size-move loop.
    Resize { size: (u32, u32), eligibility: Eligibility },
    /// `Effect::BeginDrag` succeeded (first drag frame presented with DComp still attached and
    /// DWM composed frame count recorded at `dwm_frame`).
    DragStarted { size: (u32, u32), dwm_frame: u64 },
    /// A subsequent drag frame presented while priming, reporting DWM's current composed frame
    /// count (`None` if DWM timing became unavailable).
    DragFramePresented { dwm_frame: Option<u64> },
    /// `Effect::BeginDrag`, `Effect::RenderDragFrame`, or `Effect::DetachDcomp` failed; fall back
    /// to normal DirectComposition presentation.
    DragFailed,
    /// A redraw arrived while dragging (modal drag tick or unsolicited `WM_PAINT`).
    Redraw,
    /// `WM_EXITSIZEMOVE` ended the modal size-move loop (normal finish or Escape cancel).
    ExitSizeMove { now: Instant },
    /// The first post-drag normal D3D12 flip-model frame presented and finished on the GPU.
    ExitFrameCompleted,
    /// The `Leaving` safety timer fired.
    TimerDue { attempt: Attempt, now: Instant },
    /// CPU-to-GPU handover transitioned mid-drag.
    Handover,
    /// GPU device was lost or surface replacement started.
    DeviceLost,
    /// Window hid (may show again later, so DirectComposition must be re-attached).
    Hide,
    /// Window is closing or `WM_NCDESTROY` fired.
    Destroy,
}

/// Side effect requested by [`step`], carried out in order by the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Release active video layers, lazily initialize D3D11 + shared texture + copy-model swap
    /// chain, enable `WVR_VALIDRECTS` and `WM_ERASEBKGND` suppression, render and present the
    /// first drag frame in `WM_PAINT`, and record DWM's composed frame count while keeping
    /// DirectComposition attached.
    BeginDrag { size: (u32, u32) },
    /// Render one drag frame at `size` into the D3D12 shared texture and present via the D3D11
    /// copy-model swap chain inside `WM_PAINT`, with zero presents to the hidden D3D12 swap
    /// chain.
    RenderDragFrame { size: (u32, u32) },
    /// Detach and commit the DirectComposition root on `Handover` once DWM has composed at least
    /// [`DETACH_DWM_FRAMES`] frames after the first drag present.
    DetachDcomp,
    /// Disable `WVR_VALIDRECTS`, reconfigure the hidden D3D12 swap chain to the final size, and
    /// schedule one normal D3D12 flip-model frame while DirectComposition stays detached.
    RequestExitFrame,
    /// Schedule a wake at the `Leaving` safety deadline.
    WakeAt(Instant),
    /// Re-attach and commit the DirectComposition root on `Handover`.
    AttachDcomp,
    /// Free drag shared textures, shared fences, and the D3D11 copy-model swap chain, and clear
    /// the subclass drag-sync flag.
    ReleaseDragResources,
    /// Drop the lazily created D3D11 device (required after D3D12 device loss / TDR).
    DropD3d11Device,
}

/// Pure state transition for synchronized drag resize.
pub(crate) fn step(seq: &mut Seq, state: &mut State, event: Event) -> Effects<Effect> {
    match (*state, event) {
        (State::Idle, Event::EnterSizeMove { eligibility }) => {
            if eligibility.allowed() {
                *state = State::Entering;
            }
            Effects::none()
        }
        (State::Idle, _) => Effects::none(),

        (State::Entering, Event::Resize { size, eligibility }) => {
            if !eligibility.allowed() || size.0 == 0 || size.1 == 0 {
                *state = State::Idle;
                Effects::none()
            } else {
                Effects::one(Effect::BeginDrag { size })
            }
        }
        (State::Entering, Event::DragStarted { size, dwm_frame }) => {
            *state = State::Dragging { size, dcomp: DcompPhase::Priming { presented_at: dwm_frame } };
            Effects::none()
        }
        (State::Entering, Event::DragFailed) => {
            *state = State::Idle;
            Effects::two(Effect::AttachDcomp, Effect::ReleaseDragResources)
        }
        (
            State::Entering,
            Event::ExitSizeMove { .. } | Event::Handover | Event::Hide | Event::Destroy,
        ) => {
            *state = State::Idle;
            Effects::none()
        }
        (State::Entering, Event::DeviceLost) => {
            *state = State::Idle;
            Effects::one(Effect::DropD3d11Device)
        }
        (State::Entering, _) => Effects::none(),

        (State::Dragging { dcomp, .. }, Event::Resize { size, eligibility }) => {
            if !eligibility.allowed() || size.0 == 0 || size.1 == 0 {
                *state = State::Idle;
                Effects::two(Effect::AttachDcomp, Effect::ReleaseDragResources)
            } else {
                *state = State::Dragging { size, dcomp };
                Effects::one(Effect::RenderDragFrame { size })
            }
        }
        (State::Dragging { size, .. }, Event::Redraw) => {
            Effects::one(Effect::RenderDragFrame { size })
        }
        (
            State::Dragging { size, dcomp: DcompPhase::Priming { presented_at } },
            Event::DragFramePresented { dwm_frame },
        ) => match dwm_frame {
            None => {
                *state = State::Idle;
                Effects::two(Effect::AttachDcomp, Effect::ReleaseDragResources)
            }
            Some(now) if now < presented_at => {
                // DWM restarted or reset its composition counter: re-baseline and keep waiting.
                *state = State::Dragging { size, dcomp: DcompPhase::Priming { presented_at: now } };
                Effects::none()
            }
            Some(now) if now.saturating_sub(presented_at) >= DETACH_DWM_FRAMES => {
                *state = State::Dragging { size, dcomp: DcompPhase::Detached };
                Effects::one(Effect::DetachDcomp)
            }
            Some(_) => Effects::none(),
        },
        (State::Dragging { .. }, Event::ExitSizeMove { now }) => {
            let attempt = seq.next();
            let deadline = now + LEAVING_TIMEOUT;
            *state = State::Leaving { attempt, deadline };
            Effects::two(Effect::RequestExitFrame, Effect::WakeAt(deadline))
        }
        (State::Dragging { .. }, Event::DragFailed | Event::Handover | Event::Hide) => {
            *state = State::Idle;
            Effects::two(Effect::AttachDcomp, Effect::ReleaseDragResources)
        }
        (State::Dragging { .. }, Event::DeviceLost) => {
            *state = State::Idle;
            let mut effects = Effects::two(Effect::AttachDcomp, Effect::ReleaseDragResources);
            effects.push(Effect::DropD3d11Device);
            effects
        }
        (State::Dragging { .. }, Event::Destroy) => {
            *state = State::Idle;
            Effects::one(Effect::ReleaseDragResources)
        }
        (State::Dragging { .. }, _) => Effects::none(),

        (State::Leaving { .. }, Event::ExitFrameCompleted) => {
            *state = State::Idle;
            Effects::two(Effect::AttachDcomp, Effect::ReleaseDragResources)
        }
        (State::Leaving { attempt, deadline }, Event::TimerDue { attempt: a, now }) => {
            if a == attempt && now >= deadline {
                *state = State::Idle;
                Effects::two(Effect::AttachDcomp, Effect::ReleaseDragResources)
            } else {
                Effects::none()
            }
        }
        (State::Leaving { .. }, Event::EnterSizeMove { eligibility }) => {
            *state = if eligibility.allowed() { State::Entering } else { State::Idle };
            Effects::two(Effect::AttachDcomp, Effect::ReleaseDragResources)
        }
        (State::Leaving { .. }, Event::Handover | Event::Hide) => {
            *state = State::Idle;
            Effects::two(Effect::AttachDcomp, Effect::ReleaseDragResources)
        }
        (State::Leaving { .. }, Event::DeviceLost) => {
            *state = State::Idle;
            let mut effects = Effects::two(Effect::AttachDcomp, Effect::ReleaseDragResources);
            effects.push(Effect::DropD3d11Device);
            effects
        }
        (State::Leaving { .. }, Event::Destroy) => {
            *state = State::Idle;
            Effects::one(Effect::ReleaseDragResources)
        }
        (State::Leaving { .. }, _) => Effects::none(),
    }
}

/// Computes `(dest_valid, src_valid)` for `WM_NCCALCSIZE` `WVR_VALIDRECTS` given the proposed
/// `new_client` and previous `old_client` rectangles in screen coordinates `[left, top, right,
/// bottom]`.
///
/// Preserves the screen-space intersection of `new_client` and `old_client` in place (`dest ==
/// src`) so Windows' `SetWindowPos` does not bit-shift existing client pixels when dragging the
/// left or top edge. When the rectangles do not overlap (or either is empty), returns an empty
/// rectangle anchored at `new_client`'s top-left so Windows copies nothing and invalidates the
/// whole new client area.
pub(crate) fn valid_rects(new_client: [i32; 4], old_client: [i32; 4]) -> ([i32; 4], [i32; 4]) {
    let empty = [new_client[0], new_client[1], new_client[0], new_client[1]];
    if new_client[2] <= new_client[0]
        || new_client[3] <= new_client[1]
        || old_client[2] <= old_client[0]
        || old_client[3] <= old_client[1]
    {
        return (empty, empty);
    }
    let left = new_client[0].max(old_client[0]);
    let top = new_client[1].max(old_client[1]);
    let right = new_client[2].min(old_client[2]);
    let bottom = new_client[3].min(old_client[3]);
    if left < right && top < bottom {
        let overlap = [left, top, right, bottom];
        (overlap, overlap)
    } else {
        (empty, empty)
    }
}
