//! Drawing a window's frame: the drawable, the quiet timer frame that draws nothing new, and
//! the frames a covered or occluded window steps without drawing.

use super::*;
use super::stepping::FrameMove;

/// The first frame's marks (crate::marks), for the first window to draw.
static FIRST_BUILD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static FIRST_ENCODED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The first frame the swap chain called occluded (a window DXGI won't present, as on a desktop
/// no one sees): the window then waits `OCCLUDED_RETRY` before it tries again.
static FIRST_OCCLUDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl<S: 'static> Host<S> {
    /// A shown frame with no drawable. Asking for a redraw here would turn the loop at once, for
    /// as long as the window is occluded or its displays sleep, so it waits out a backoff
    /// (`Backoff`), with its display link stopped so no tick redraws sooner. `stepped` when this
    /// frame already ran the runtime.
    pub(super) fn missed_frame(&mut self, missed: Missed, stepped: bool) {
        let Some(l) = self.win.as_mut() else { return };
        l.timer_frame.drawn();
        l.presented = None;
        let now = Instant::now();
        if missed == Missed::Occluded {
            crate::marks::mark_once(&FIRST_OCCLUDED, "frame.occluded");
            // Covered or asleep with no notification yet: rest as if one came, and look
            // again when the backoff is up.
            self.life.cover(Cover::Surface, true);
            l.first_frame.skipped(now);
            #[cfg(target_os = "macos")]
            l.set_live_resize(false);
            self.rt.set_live_visible(false);
            self.secure_input.set(false);
            self.idle = true;
        }
        let paced = l.paced();
        l.backoff_step(BackoffEvent::Missed { missed, paced, now });
        self.meter.set_pacing(l.pacing_state(missed != Missed::Occluded));
        // What this frame would have run still runs, without drawing (`covered_step`). Misses
        // come at the retry's pace, not the display's.
        if !stepped { covered_step(&mut self.rt); }
        self.clipboard.flush(&mut self.rt);
        self.frame_reports.skipped();
    }

    /// A frame for a window that's hidden, covered, asleep or minimised: while shown, what it
    /// would have run runs without drawing (`covered_step`). It isn't live: a minimize X11
    /// reports with no event lands here first, and the frame that next draws settles.
    pub(super) fn covered_frame(&mut self) {
        let Some(l) = self.win.as_mut() else { return };
        l.timer_frame.drawn();
        l.presented = None;
        l.stop_pacing();
        let shown = self.life.shown();
        l.first_frame.skipped(Instant::now());
        // Covered by other windows, it still draws what changed, later (`CoveredDraw`).
        #[cfg(target_os = "windows")]
        if shown && self.life.covered() && !l.minimized() { l.covered_draw = l.covered_draw.step(CoveredDrawEvent::Asked(Instant::now())); }
        self.land_frame();
        if shown {
            self.rt.set_live_visible(false);
            covered_step(&mut self.rt);
            self.clipboard.flush(&mut self.rt);
            self.frame_reports.skipped();
        }
    }

    pub(super) fn redraw(&mut self) { self.draw(false) }

    /// Draws a frame. `resizing` draws one for a size the user just dragged the window to, at
    /// once: AppKit shows the new size when the event returns, with or without it.
    pub(super) fn draw(&mut self, resizing: bool) {
        // Windows: a window still on the CPU renderer draws there (`cpu_frame`).
        #[cfg(target_os = "windows")]
        {
            self.look();
            self.hand_over_if_done();
            if self.win.as_ref().is_some_and(|w| w.native.mode.cpu_draws()) { return self.draw_cpu(resizing); }
        }
        let launcher = self.launcher();
        let Some(l) = self.win.as_ref() else { return };
        let hidden = launcher && !self.life.shown();
        let covered = (!launcher && !self.life.uncovered()) || l.minimized();
        #[cfg(target_os = "windows")]
        let owed = covered && !l.minimized() && l.covered_draw.drawing();
        #[cfg(not(target_os = "windows"))]
        let owed = false;
        if covered && !owed {
            // A covered window's clips stop presenting on their layers, as they stop being pulled.
            #[cfg(target_os = "windows")]
            if let Some(w) = self.win.as_mut() {
                let native = &mut w.native;
                native.video_layers.release(native.mode.handover());
            }
            return self.covered_frame();
        }
        // A covered window's owed frame shows the settled state, as its first one uncovered does.
        if owed { self.rt.settle_all(); }
        // A missed frame's retry, or the timer pacing unpaced motion: input waits for it, and
        // `new_events` runs due timers meanwhile.
        if !resizing && l.backing_off(Instant::now()) { return; }
        // A device that died (a driver reset, a GPU removed, a resume from Modern Standby) and a
        // surface that refused its configuration can draw nothing: the window makes both again
        // before this frame, or waits out a retry of its own (`SurfaceRetries`) and then gives
        // them up. Neither can give a drawable, and wgpu panics asking one for it.
        if !self.replace_lost_surface() { return; }
        // A clip's layer began or stopped showing: the UI's swap chain composites premultiplied
        // only while one does (`video_layer`), made again before this frame draws.
        #[cfg(target_os = "windows")]
        if let Some(premultiplied) = self.win.as_mut().and_then(|w| w.native.video_layers.take_alpha())
            && let Err(error) = self.rebind_alpha(premultiplied)
        {
            eprintln!("rune: the window's swap chain couldn't change its alpha for a clip's layer: {error}");
        }
        // The swap chain still holds a frame the compositor hasn't shown: this one draws when it
        // has room, built from the input that came by then (`slot`).
        #[cfg(target_os = "windows")]
        if !hidden && let Some(l) = self.win.as_mut() {
            // SAFETY: the object is the live swap chain's; `Slot` waits on its own copy.
            let waitable = l.surface.as_ref().and_then(|s| unsafe { s.as_hal::<wgpu::hal::api::Dx12>().and_then(|h| h.waitable_handle()) });
            let handle = self.native_handle.clone();
            if l.native.slot.take(waitable.map(|h| h.0), move || handle.send(WakeEvent::FrameSlot)) == super::slot::Take::Later { return; }
        }
        let Some(l) = self.win.as_mut() else { return };
        #[cfg(target_os = "windows")]
        if owed { l.covered_draw = l.covered_draw.step(CoveredDrawEvent::Drawn); }
        // The frame a due timer asked for is here, drawing or running covered below.
        l.timer_frame.drawn();
        // Starting the display link for a window that isn't on screen would wait for a tick that
        // sleeping displays never send, and a running link holds back its timers (`wake_deadline`).
        #[cfg(target_os = "macos")]
        if !hidden {
            if !l.natively_occluded() {
                l.native.prime.stop();
            } else if !l.native.prime.presumes() {
                self.meter.skipped();
                self.missed_frame(Missed::Occluded, false);
                return;
            }
        }
        #[cfg(target_os = "macos")]
        let _prewarm = if hidden { l.native.panel.as_ref().map(|p| p.prewarm()) } else { None };
        // Only a timer woke the resting window, so its frame may draw what's on screen. It
        // decides that before starting the display link, which would draw it at once anyway.
        #[cfg(target_os = "macos")]
        let deferred = l.timer_frame.quiet() && !hidden && !resizing && !l.native.prime.presumes()
            && l.native.pacing.as_ref().is_some_and(|p| p.defers_start(Instant::now()));
        #[cfg(not(target_os = "macos"))]
        let deferred = false;
        #[cfg(target_os = "macos")]
        let target = if hidden || resizing || deferred { None } else if let Some(pacing) = &mut l.native.pacing {
            if l.native.prime.presumes() && !pacing.running() {
                None
            } else {
                let Some(target) = pacing.next_frame().target() else { return };
                target
            }
        } else { None };
        #[cfg(target_os = "macos")]
        let shown = if !hidden && l.native.prime.presumes() {
            l.native.prime.spend();
            if let Some(surface) = l.surface.as_ref()
                && let Some(shown) = crate::platform::macos::PresumeShown::new(surface) {
                Some(shown)
            } else {
                self.meter.skipped();
                self.missed_frame(Missed::Occluded, false);
                return;
            }
        } else {
            None
        };
        if self.native_handle.native.number.load(Ordering::Relaxed) == 0 {
            self.native_handle.native.number.store(native_window_number(&l.window).unwrap_or(0), Ordering::Release);
        }
        #[cfg(not(target_os = "macos"))]
        let target: Option<Instant> = None;
        if !hidden { self.meter.redraw(l.paced_for_stats() || deferred); }
        #[cfg(target_os = "macos")]
        let live_resize = !hidden && (resizing || crate::platform::macos::in_live_resize(&l.window));
        #[cfg(not(target_os = "macos"))]
        let live_resize = resizing;
        // The frame moves before the drawable is acquired, so it has the new size; never while
        // the user holds the window's edge in a live resize.
        if !hidden && !live_resize { self.step_frame(target.unwrap_or_else(Instant::now)); }
        let (Some(l), Some((gpu, renderer))) = (self.win.as_mut(), self.gpu.as_mut()) else { return };
        #[cfg(target_os = "macos")]
        l.set_live_resize(resizing && live_resize);
        // Acquire before latching live values so a drawable wait cannot age the input.
        l.backoff_step(BackoffEvent::Lapsed);
        if renderer.srgb_blend() != self.rt.srgb_blend() {
            let Some(surface) = l.surface.as_ref() else { return };
            if !reblend(surface, gpu, &mut l.config, renderer, &mut l.alpha, &mut self.rt) {
                // A surface that won't take the new format is replaced before the next frame.
                l.surface_ok = false;
                return;
            }
        }
        #[cfg(target_os = "macos")]
        { self.rt.native_materials = l.native.materials.native(); }
        if self.frame_reports.wanted() { self.frame_reports.set_refresh(display_hz(l)); }
        // Only a timer woke the resting window: build the frame first, and take no drawable
        // when it draws what's on screen.
        #[cfg(feature = "frame-profile")]
        let fp_entry = crate::frame::live_log::on().then(crate::frame::live_log::Stamp::now);
        let quiet = l.timer_frame.take_quiet() && !hidden && !live_resize && l.presented.is_some() && !composed(self.comp.as_deref())
            && self.trace.is_none() && self.args.trace.is_none() && self.rt.live_visible()
            && self.rt.window_frame.is_none() && !FrameMove::moving(&self.frame_move);
        #[cfg(target_os = "macos")]
        let quiet = quiet && shown.is_none() && !l.native.materials.native();
        let built = if quiet {
            let now = Instant::now();
            self.rt.window_resting = true;
            let (animating, dt) = self.clock.advance(&mut self.rt, target.unwrap_or(now), false);
            let rt = &mut self.rt;
            #[cfg(target_os = "macos")]
            let next_tick = if deferred { l.native.pacing.as_ref().map(|p| p.tick_after(now)) } else { l.next_tick(now) };
            #[cfg(not(target_os = "macos"))]
            let next_tick = l.next_tick(now);
            if renderer.srgb_blend() == rt.srgb_blend()
                && l.presented.as_ref().is_some_and(|p| p.unchanged(rt, &l.config, animating, next_tick)) {
                sync_platform(l, rt, &mut self.secure_input, &mut self.ime_allowed, &self.clipboard);
                l.stop_pacing();
                self.meter.set_pacing(l.pacing_state(false));
                self.frame_reports.unchanged(target);
                return;
            }
            Some((animating, dt, now))
        } else {
            None
        };
        // It draws: the link starts, and still says draw at once.
        #[cfg(target_os = "macos")]
        if deferred && let Some(pacing) = &mut l.native.pacing { let _ = pacing.next_frame(); }
        #[cfg(feature = "frame-profile")]
        let fp_start = crate::frame::live_log::on().then(crate::frame::live_log::Stamp::now);
        let Some(surface) = l.surface.as_ref() else { return };
        let frame = match acquire(surface, gpu, &l.config, l.surface_ok, 1, &mut self.meter) {
            Ok(frame) => frame,
            // A surface with no presentation (a refused configuration, or a device that is gone)
            // is replaced before the next frame rather than asked for a drawable.
            Err(Missed::Unconfigured) => {
                l.surface_ok = false;
                return self.missed_frame(Missed::Unconfigured, built.is_some());
            }
            Err(_) if hidden => { l.first_frame.missed(); l.backoff_step(BackoffEvent::HiddenMissed { preparing: l.first_frame.deadline().is_some(), now: Instant::now() }); self.frame_reports.skipped(); return; }
            Err(missed) => return self.missed_frame(missed, built.is_some()),
        };
        #[cfg(feature = "frame-profile")]
        let fp_acquired = fp_start.map(|_| crate::frame::live_log::Stamp::now());
        // A covered window's owed frame (`CoveredDraw`) draws settled and stays covered: live, its
        // loops would ask for frames no one sees.
        if !hidden && !self.life.covered() && !self.rt.live_visible() && { let size = l.window.inner_size(); size.width > 0 && size.height > 0 } {
            // Uncovered after a missed frame said it was occluded.
            self.rt.set_live_visible(true);
            self.rt.settle_all();
            #[cfg(target_os = "macos")]
            if l.native.pacing.is_none() { l.native.pacing = crate::platform::display_link::DisplayLink::new(&l.window, self.frame_wake.clone()); }
        }
        #[cfg(target_os = "macos")]
        let mut drawable = l.native.present.as_ref().and(self.present_log.as_ref()).and_then(|log| log.acquired());
        #[cfg(not(target_os = "macos"))]
        let drawable = None;
        let now = built.map_or_else(Instant::now, |(_, _, now)| now);
        if self.trace.is_none() && let Some(path) = &self.args.trace {
            let hz = l.window.current_monitor().and_then(|m| m.refresh_rate_millihertz()).map_or(60.0, |hz| hz as f64 / 1000.0);
            #[cfg(target_os = "macos")]
            let hz = l.native.pacing.as_ref().map_or(hz, |pacing| pacing.refresh_hz());
            let mut trace = crate::tooling::Trace::open_clock(path, self.rt.viewport, hz, self.rt.env.scale, "wall");
            trace.all = self.args.trace_all;
            #[cfg(target_os = "macos")]
            if let Some(log) = self.present_log.clone() && l.native.present.is_some() { trace.record_presents(log, now); }
            trace.record_surface(self.meter.shared(), self.rt.app_stats);
            self.trace = Some(trace);
            self.trace_start = Some(now);
            self.rt.scene.store_mut().audit_retargets(true);
        }
        let frame_time = target.unwrap_or(now);
        let presents_at = target;
        self.rt.window_resting = !FrameMove::moving(&self.frame_move) && self.rt.window_frame.is_none();
        let target_ms = target.zip(self.trace_start).map(|(target, start)| crate::frame::present_time::ms_since(start, target));
        crate::marks::mark_once(&FIRST_BUILD, "frame.first.build");
        let rt = &mut self.rt;
        if hidden && l.first_frame.settles() { rt.settle_all(); }
        let (mut animating, dt) = if let Some((animating, dt, _)) = built {
            (animating, dt)
        } else if hidden {
            self.clock.hold(frame_time);
            (rt.frame(0.0), 0.0)
        } else {
            self.clock.advance(rt, frame_time, composing(self.comp.as_deref()))
        };
        #[cfg(target_os = "macos")]
        let frame = if l.surface.as_ref().is_some_and(|surface| l.native.materials.update(rt.transparent, !rt.materials.is_empty(), &l.window, surface)) {
            // This drawable presents without the views: build again natively into a new one.
            drop(frame);
            rt.native_materials = true;
            animating |= rt.frame(0.0);
            let Some(surface) = l.surface.as_ref() else { return };
            let frame = match acquire(surface, gpu, &l.config, l.surface_ok, 1, &mut self.meter) {
                Ok(frame) => frame,
                Err(Missed::Unconfigured) => {
                    l.surface_ok = false;
                    return self.missed_frame(Missed::Unconfigured, true);
                }
                // Retrying at once is what spins while occluded; wait like any missed frame.
                Err(_) if hidden => { l.first_frame.missed(); l.backoff_step(BackoffEvent::HiddenMissed { preparing: l.first_frame.deadline().is_some(), now: Instant::now() }); self.frame_reports.skipped(); return; }
                Err(missed) => return self.missed_frame(missed, true),
            };
            drawable = l.native.present.as_ref().and(self.present_log.as_ref()).and_then(|log| log.acquired());
            frame
        } else { frame };
        // A handler switched blending (`Tx::set_srgb_blend`): this drawable has the old format.
        let frame = if renderer.srgb_blend() != rt.srgb_blend() {
            drop(frame);
            let Some(surface) = l.surface.as_ref() else { return };
            if !reblend(surface, gpu, &mut l.config, renderer, &mut l.alpha, rt) {
                l.surface_ok = false;
                return;
            }
            let frame = match acquire(surface, gpu, &l.config, l.surface_ok, 1, &mut self.meter) {
                Ok(frame) => frame,
                Err(Missed::Unconfigured) => {
                    l.surface_ok = false;
                    return self.missed_frame(Missed::Unconfigured, true);
                }
                Err(_) if hidden => { l.first_frame.missed(); l.backoff_step(BackoffEvent::HiddenMissed { preparing: l.first_frame.deadline().is_some(), now: Instant::now() }); self.frame_reports.skipped(); return; }
                Err(missed) => return self.missed_frame(missed, true),
            };
            #[cfg(target_os = "macos")]
            { drawable = l.native.present.as_ref().and(self.present_log.as_ref()).and_then(|log| log.acquired()); }
            frame
        } else { frame };
        #[cfg(target_os = "macos")]
        drop(shown);
        l.backoff_step(BackoffEvent::Presented);
        if hidden { l.hidden_pace.rendered(now); }
        let update_ms = now.elapsed().as_secs_f64() * 1000.0;
        #[cfg(feature = "frame-profile")]
        let fp_built = fp_start.map(|_| crate::frame::live_log::Stamp::now());
        sync_platform(l, rt, &mut self.secure_input, &mut self.ime_allowed, &self.clipboard);
        let target_format = if rt.srgb_blend() { l.config.format.remove_srgb_suffix() } else { l.config.format };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: (target_format != l.config.format).then_some(target_format),
            ..Default::default()
        });
        // An image's pixels changed where its instances didn't: the whole frame draws (`damage`).
        #[cfg(target_os = "windows")]
        let images_changed = rt.images.pages.iter().any(|p| p.dirty.is_some());
        renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
        renderer.build_surfaces(&gpu.device, &rt.surfaces, rt.live_notifier());
        // Playing clips on their own layer (`video_layer`): the surfaces this frame punches out.
        #[cfg(target_os = "windows")]
        {
            let premultiplied = l.config.alpha_mode == wgpu::CompositeAlphaMode::PreMultiplied;
            let punch_ready = renderer.surface_punch_ready();
            let native = &mut l.native;
            let layers = native.video_layers.frame(&rt.surfaces, native.mode.handover(), premultiplied, punch_ready);
            renderer.set_punched_surfaces(layers.punched);
            animating |= layers.again;
        }
        renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (l.config.width, l.config.height));
        #[cfg(feature = "frame-profile")]
        let fp_uploaded = fp_start.map(|_| crate::frame::live_log::Stamp::now());
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        #[cfg(target_os = "windows")]
        let mut present_rect = None;
        // Why the frame drew all of the window (frame-profile).
        #[allow(unused_mut)]
        let mut whole_why: Option<&'static str> = None;
        let target = l.alpha.as_ref().map_or(&view, |a| &a.view);
        renderer.set_frame_wake(rt.live_wake());
        let size = (l.config.width, l.config.height);
        // Only what changed is redrawn and presented (`damage`): the renderer's `draw` takes the
        // armed rectangles when nothing but the instances draws there, a compositor's
        // `Frame::draw_ui` included.
        #[cfg(target_os = "windows")]
        let part = {
            let whole = if l.alpha.is_some() { Some("alpha") } else if images_changed { Some("image") }
                else if !l.config.usage.contains(wgpu::TextureUsages::COPY_DST) { Some("no copies") } else { None };
            match l.native.damage.plan(&rt.instances, size, rt.clear_color(), whole.is_none()) {
                super::damage::Plan::All(why) => { whole_why = Some(whole.unwrap_or(why)); None }
                super::damage::Plan::Part { draw, present } => {
                    renderer.arm_damage(&gpu.device, &gpu.queue, &draw, rt.clear_color(), size, l.config.format);
                    Some(present)
                }
            }
        };
        if let Some(c) = self.comp.as_mut() {
            let mut f = Frame {
                device: &gpu.device,
                queue: &gpu.queue,
                encoder: &mut enc,
                target,
                format: target_format,
                size,
                dt,
                update_ms,
                rt: &mut *rt,
                ui: &mut *renderer,
            };
            c.render(&mut f);
            animating |= c.animating();
        } else {
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
            renderer.draw(&mut enc, target, rt.clear_color());
        }
        // Every frame, so a frame drawn whole leaves the renderer's kept frame behind.
        #[cfg(target_os = "windows")]
        match (renderer.damage_drawn(), part) {
            (Ok(()), part) => present_rect = part,
            (Err(why), Some(_)) => whole_why = Some(why),
            (Err(_), None) => {}
        }
        #[cfg(not(feature = "frame-profile"))]
        let _ = whole_why;
        if let Some(alpha) = &l.alpha { alpha.draw(&mut enc, &view); }
        #[cfg(feature = "frame-profile")]
        let fp_encoded = fp_start.map(|_| crate::frame::live_log::Stamp::now());
        renderer.submit(&gpu.queue, enc);
        #[cfg(feature = "frame-profile")]
        let fp_submitted = fp_start.map(|_| crate::frame::live_log::Stamp::now());
        crate::marks::mark_once(&FIRST_ENCODED, "frame.first.encoded");
        let encoded = self.frame_reports.stamp();
        l.window.pre_present_notify();
        #[cfg(target_os = "macos")]
        l.native.materials.present(&rt.materials, rt.env.scale, || gpu.queue.present(frame));
        #[cfg(target_os = "windows")]
        if let (Some(rects), Some(surface)) = (&present_rect, l.surface.as_ref())
            // SAFETY: only names the rectangles the next present changed.
            && let Some(hal) = unsafe { surface.as_hal::<wgpu::hal::api::Dx12>() } {
            let rects: Vec<_> = rects.iter().map(|&[left, top, right, bottom]| windows::Win32::Foundation::RECT { left, top, right, bottom }).collect();
            hal.set_present_rects(&rects);
        }
        #[cfg(not(target_os = "macos"))]
        gpu.queue.present(frame);
        #[cfg(target_os = "windows")]
        l.native.slot.presented();
        // A swap chain made again on a handed-over window's visual takes over now (`cpu_frame`).
        #[cfg(target_os = "windows")]
        if matches!(l.native.mode, super::cpu_frame::SurfaceMode::Rebinding { .. }) {
            let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
            if let super::cpu_frame::SurfaceMode::Rebinding { handover } = std::mem::take(&mut l.native.mode) {
                if let Err(error) = handover.commit() { eprintln!("rune: the window's new swap chain couldn't take over: {error}"); }
                l.native.mode = super::cpu_frame::SurfaceMode::Handed { handover };
            }
        }
        #[cfg(target_os = "windows")]
        l.native.video_layers.drawn(|key| renderer.surface_drew_isolated(key));
        crate::marks::presented();
        crate::render::gpu_events::presented();
        #[cfg(feature = "frame-profile")]
        let fp_frame = match (fp_start, fp_acquired, fp_built, fp_uploaded, fp_encoded, fp_submitted) {
            (Some(start), Some(acquired), Some(built), Some(uploaded), Some(encoded), Some(submitted)) => Some(crate::frame::live_log::Frame {
                entry: fp_entry.unwrap_or(start), start, acquired, built, uploaded, encoded, submitted, presented: crate::frame::live_log::Stamp::now(),
                phases: crate::frame::profile::take(), nodes: rt.nodes.len(), shaped: rt.stats.shaped, instances: rt.instances.len(),
                animating, why: std::mem::take(&mut rt.commit_why), next: 0, poll_due: None, loop_due: None, loops: Vec::new(),
                #[cfg(target_os = "windows")]
                damage: present_rect,
                #[cfg(not(target_os = "windows"))]
                damage: None,
                whole: whole_why,
                #[cfg(target_os = "windows")]
                change: l.native.damage.seen,
                #[cfg(not(target_os = "windows"))]
                change: None,
            }),
            _ => None,
        };
        // The frame reached the screen: a surface replaced by `replace_lost_surface` works, and
        // the next loss starts its retries over.
        l.surface_retries.worked();
        #[cfg(target_os = "macos")]
        if let Some(shadow) = &l.native.shadow { shadow.presented(); }
        // The frame landed with this present; later ones needn't wait for a transaction.
        #[cfg(target_os = "macos")]
        if !FrameMove::moving(&self.frame_move) { l.present_with_transaction(false); }
        if !l.first_frame.ready()
            && gpu.device.poll(wgpu::PollType::wait_indefinitely()).is_ok() {
            l.first_frame.gpu_done();
            crate::marks::mark("frame.first.gpu_done");
            #[cfg(target_os = "windows")]
            crate::platform::win32::unidle_d3d_threads_soon();
        }
        // The window shows once the GPU has finished the frame it presents.
        #[cfg(target_os = "windows")]
        if l.native.cloak.held() {
            let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
            l.native.cloak.lift(&l.window);
            crate::marks::mark("window.shown");
        }
        let submitted = Instant::now();
        self.clock.presented(rt, submitted);
        self.meter.presented();
        #[cfg(target_os = "macos")]
        if let Some(pacing) = &mut l.native.pacing { pacing.presented(submitted); }
        // A display link that couldn't be made while the displays slept: they're back.
        #[cfg(target_os = "macos")]
        if !hidden && l.native.pacing.is_none() { l.native.pacing = crate::platform::display_link::DisplayLink::retry(&l.window, self.frame_wake.clone()); }
        self.frame_reports.drawn(encoded, presents_at, now, update_ms);
        rt.interaction_presented(submitted, presents_at);
        self.frame_reports.inspect(rt);
        if let Some(trace) = self.trace.as_mut() {
            // A frame count keeps the trace redrawing while idle, as below.
            let continuous = !self.idle || self.args.frames.is_some();
            let timing = crate::tooling::FrameTiming { target_ms, drawable, continuous };
            trace.frame_timed(rt, (submitted - self.trace_start.unwrap()).as_secs_f64() * 1000.0,
                now.elapsed().as_secs_f64() * 1000.0, timing);
            self.trace_frames += 1;
            if self.args.frames.is_some_and(|n| self.trace_frames >= n) {
                // While the window still shows its last frames.
                trace.settle_presents(crate::frame::present_time::SETTLE);
                self.life.finish();
                return;
            }
        }
        // A poll asking for the next frame (`wake_after(Duration::ZERO)`) keeps a running display
        // link going; without one its timer waits a frame in `wake_deadline`.
        let next_tick = l.next_tick(now).filter(|_| !hidden);
        let rests = !hidden && !(self.trace.is_some() && self.args.frames.is_some());
        self.idle = Presented::settle(&mut l.presented, rt, &l.config, animating || FrameMove::moving(&self.frame_move), next_tick, rests, composed(self.comp.as_deref()));
        // A drawn frame asks for nothing: it only puts the rest off.
        if !hidden { let _ = rest::step(&mut self.rest, RestEvent::Drew { now, low: rest::low_memory() }); }
        // Why the window draws again: each bit is one of `Presented::settle`'s reasons.
        #[cfg(feature = "frame-profile")]
        let fp_next = [animating, FrameMove::moving(&self.frame_move), rt.needs_commit(), rt.stale() && !rt.needs_commit(), rt.has_pending_input(), rt.live_pending(),
            rt.window_frame.is_some(), super::timer_wants_frame(rt, next_tick)]
            .iter().enumerate().fold(0u16, |bits, (i, on)| bits | (u16::from(*on) << i));
        #[cfg(target_os = "macos")]
        if !hidden && let Some(latency) = l.native.launch_drawables.settle(self.idle, now) {
            l.config.desired_maximum_frame_latency = latency;
            if let Some(surface) = l.surface.as_ref()
                && let Err(error) = configure(surface, gpu, &l.config) {
                eprintln!("rune: a window's surface refused its frame latency: {error}");
                l.surface_ok = false;
            }
        }
        #[cfg(feature = "frame-profile")]
        if let Some(mut f) = fp_frame {
            f.next = fp_next;
            f.poll_due = rt.poll_wake_deadline().map(|d| d - rt.polled_at());
            f.loop_due = rt.motion_wake_deadline().map(|d| d - rt.polled_at());
            if f.loop_due.is_some() { f.loops = rt.fp_loops(); }
            crate::frame::live_log::frame(f);
        }
        if !hidden && (!self.idle || (self.trace.is_some() && self.args.frames.is_some())) {
            // Without a running display link, macOS hands out drawables faster than the
            // display shows them, so a timer paces the next frame instead of the loop.
            #[cfg(target_os = "macos")]
            if !l.native.pacing.as_mut().is_some_and(|p| p.start()) { l.backoff_step(BackoffEvent::At(now + TIMER_FRAME)); }
            #[cfg(not(target_os = "macos"))]
            l.request_redraw();
            self.meter.set_pacing(l.pacing_state(true));
        } else {
            l.stop_pacing();
            self.meter.set_pacing(l.pacing_state(false));
        }
        // A frame change this frame's commit asked for starts with the layout springs it
        // started, which have shown a step unless they held for the window; the next frame
        // moves it. While the user drags the edge, it waits until they let go.
        #[cfg(target_os = "macos")]
        if !hidden && !live_resize && self.rt.window_frame.is_some() {
            let shown = if self.rt.fit.take_held() { 0.0 } else { crate::frame::clock::RESUME_STEP };
            self.take_frame_request(self.frame_secs(frame_time) - shown, true);
        }
    }
}

/// The window's display refresh: its display link's on macOS, else the monitor's.
fn display_hz(l: &Win) -> Option<f64> {
    #[cfg(target_os = "macos")]
    if let Some(pacing) = &l.native.pacing { return Some(pacing.refresh_hz()); }
    l.window.current_monitor().and_then(|m| m.refresh_rate_millihertz()).map(|hz| hz as f64 / 1000.0)
}
