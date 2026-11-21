//! Non-generic [`RuntimeCore`] construction, shared by every [`Runtime<S>::with_text`] call.
use super::*;

impl RuntimeCore {
    pub(super) fn new(viewport: (f32, f32), env: Env, text: TextSystem) -> Self {
        crate::frame::profile::install_text_hook();
        crate::tooling::perf_warn::warm();
        let mut scene = Scene::new();
        scene.set_pixel_scale(env.scale);
        crate::recorder::headless_reset();
        crate::video::headless_reset();
        let host = window_requests::HostState::new(viewport, &env);
        RuntimeCore {
            env,
            viewport,
            t: 0.0,
            clock: 0.0,
            time_scale: 1.0,
            reduced_motion: false,
            track_marks: false,
            animating: false,
            settled: false,
            transparent: false,
            instances: Vec::new(),
            marks: Vec::new(),
            surfaces: Vec::new(),
            tree: types::TreeState::new(),
            hit: hit::HitState::new(),
            motion: tracks::MotionState::new(),
            scroll: requests::ScrollTables::new(),
            paint: paint_state::PaintState::new(),
            scene,
            images: Default::default(),
            text,
            pointer: capture::PointerState::new(),
            kbd: focus_machine::KeyboardState::new(),
            commit: commit::CommitState::new(),
            edit: caret::TextInputState::new(),
            frame: timer_wake::FrameState::new(),
            host,
            ime_area: None,
            copy_request: None,
            shell_request: None,
            wall_start: None,
            events: Vec::new(),
            stats: FrameStats::default(),
            ambient: None,
            double_click_interval: 0.5,
            help_delay: 0.5,
            _core_end: CoreEnd,
        }
    }
}
