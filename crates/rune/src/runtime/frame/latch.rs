//! Transactions, `update` lifecycle, `Tx` request latching and tracing hooks.
use super::*;

impl RuntimeCore {
    /// Starts an [`Runtime::update`] transaction with the clock reading now and begins its
    /// stall timer right before the app callback runs.
    pub(in crate::runtime) fn begin_update(&self, app: &dyn ViewDriver) -> (Tx, crate::tooling::perf_warn::Stall) {
        // Its wakes count from now (`schedule_pending_wakes`), so its clock reads now too.
        let one = Tx { clock: self.now_clock(), ..self.transaction(app) };
        (one, crate::tooling::perf_warn::Stall::start())
    }

    /// Finishes an [`Runtime::update`] / `update_if` / `update_if_ordered` call after its
    /// closure ran against `app.state`.
    pub(in crate::runtime) fn finish_update(
        &mut self,
        app: &mut dyn AppDriver,
        stall: crate::tooling::perf_warn::Stall,
        mut one: Tx,
        changed: bool,
        ordered: bool,
    ) -> (bool, Option<std::num::NonZeroU64>) {
        stall.end_named("an `update` closure");
        let stale = changed || one.frame_work() || one.window_frame.is_some() || one.srgb_blend.is_some_and(|b| b != self.paint.srgb_blend()) || one.font_smoothing.is_some_and(|m| m != self.text.font_smoothing())
            || self.changes_families(&one)
            || (one.blur && self.kbd.focus.path().is_some());
        let minimize = if ordered {
            let asked = one.minimize_order;
            self.run_pick_reads(app, &mut one);
            let survived = if asked.is_some() && one.minimize_order == asked { one.minimize_order.take() } else { None };
            self.latch_after_picks(app, &mut one);
            survived
        } else {
            self.latch_requests(app, &mut one);
            None
        };
        merge(&mut self.frame.pending, one);
        self.commit.dirty |= changed;
        (stale, minimize)
    }

    /// Takes a `Tx::minimize` that `what` can't carry out, saying so in debug builds.
    pub(crate) fn ignore_minimize(&mut self, what: &str) {
        let field = self.host.minimize.field();
        self.minimize_step(crate::window::MinimizeEvent::Lost);
        if field && cfg!(debug_assertions) {
            eprintln!("rune: minimize ignored: {what} can't minimize");
        }
    }

    /// Latches a Tx's shell and clipboard requests for the shell, so `update`, a hidden poll
    /// and `on_close` reach the clipboard without a frame, and applies clipboard requests to a
    /// memory clipboard at once. A secret clear drops an earlier secret copy not yet written.
    pub(in crate::runtime) fn latch_requests(&mut self, app: &mut dyn AppDriver, tx: &mut Tx) {
        self.run_pick_reads(app, tx);
        self.latch_after_picks(app, tx);
    }

    /// [`Self::latch_requests`] once the pick callbacks have run.
    fn latch_after_picks(&mut self, app: &mut dyn AppDriver, tx: &mut Tx) {
        self.shell_request = self.shell_request.max(tx.shell.take());
        if tx.minimize_order.take().is_some() { self.minimize_step(crate::window::MinimizeEvent::Asked); }
        self.host.titlebar.toggle_maximized |= std::mem::take(&mut tx.toggle_maximized);
        self.host.titlebar.toggle_full_screen |= std::mem::take(&mut tx.toggle_full_screen);
        for ev in std::mem::take(&mut tx.keys) { self.key(ev); }
        self.latch_playground(app, tx);
        for prefix in std::mem::take(&mut tx.evict_memos) { self.invalidate_memos_by_prefix(&prefix); }
        if !tx.open_urls.is_empty() { self.open_urls(std::mem::take(&mut tx.open_urls)); }
        if std::mem::take(&mut tx.clear_secret) {
            if self.copy_request.as_ref().is_some_and(crate::ClipboardWrite::is_secret) { self.copy_request = None; }
            self.edit.clear_secret_request = true;
            self.edit.pill_copy = None;
        }
        if let Some(copy) = tx.copy.take() {
            self.edit.pill_copy = (!copy.is_secret())
                .then(|| copy.content().html())
                .flatten()
                .filter(|h| h.contains("<span data-pill="))
                .map(|h| {
                    let frag = crate::TextEdit::from_html(h);
                    (
                        zeroize::Zeroizing::new(copy.content().text().to_owned()),
                        frag.pills().map(|(r, id)| (r, id.to_owned())).collect::<Vec<_>>(),
                    )
                })
                .filter(|(_, pills)| !pills.is_empty());
            self.copy_request = Some(copy);
        }
        if std::mem::take(&mut tx.paste) { self.edit.paste_request = Some(self.paste_target()); }
        for read in tx.clipboard_reads.drain(..).filter_map(|read| read.take()) {
            self.edit.next_clipboard_read += 1;
            self.edit.clipboard_reads.push((self.edit.next_clipboard_read, read));
            self.edit.read_requests.push(self.edit.next_clipboard_read);
        }
        self.host.popups.extend(tx.popups.drain(..).filter_map(|call| call.take()));
        for request in tx.file_dialogs.drain(..).filter_map(|call| call.take()) {
            self.request_file_dialog(request);
        }
        if let Some(mode) = tx.font_smoothing.take() { self.set_font_smoothing(mode); }
        if let Some((sans, mono)) = tx.font_families.take() { self.set_font_families(&sans, &mono); }
        if let Some(srgb) = tx.srgb_blend.take() { self.set_srgb_blend(srgb); }
        if let Some(rewind) = tx.rewind.take() { self.motion.set_rewind(rewind); }
        if std::mem::take(&mut tx.blur) {
            if self.kbd.focus.path().is_some() {
                self.set_focus(app, None, self.kbd.focus.keyboard());
                self.commit.dirty = true;
            }
            // An explicit blur is not the temporary pointer-down blur of a scrim, and nothing
            // hands focus on from it: only a control the user blurred and then left does.
            self.kbd.focus.step(focus_machine::FocusEvent::Dropped);
            self.kbd.focus.step(focus_machine::FocusEvent::Settled);
        }
        if let Some(request) = tx.focus_to.take() {
            self.kbd.focus_request = Some(request);
            self.commit.dirty = true;
        }
        if let Some(title) = tx.title.take() { self.host.window_title.set(title); }
        if let Some((text, how)) = tx.announce.take() { self.host.announcement.say(text, how); }
        if let Some(mut request) = tx.window_frame.take() {
            request.spring = (request.animate && !tx.commit.snaps() && !self.reduced_motion).then(|| tx.motion.map_or(crate::tok::motion::DEFAULT, MotionSource::spring));
            self.host.window_frame = Some(self.host.window_frame.unwrap_or_default().merge(request));
        }
        crate::clipboard::flush_memory(self);
    }

    pub fn tracing(&self) -> bool { self.frame.tracing_channels() || self.scene.tracing() }

    pub(in crate::runtime) fn transaction(&self, app: &dyn ViewDriver) -> Tx {
        Tx { trace: self.tracing().then(crate::el::TraceCalls::default), mods: self.kbd.held_mods, selection: self.selection_snapshot(app),
            input_time: crate::input::time::current(), clock: self.clock, focus_visible: self.kbd.focus.keyboard(), focused: self.focused_key(), assistive: self.host.a11y.is_some() || cfg!(target_arch = "wasm32"), ..Tx::default() }
    }

    /// A scalar from the actual draw path (e.g. `shell.width`). Values hold until resampled.
    /// Call before the tooling writer records the frame; scene traces sample in `frame`.
    pub fn trace_channel(&mut self, name: &'static str, kind: &'static str, value: f32) {
        self.frame.record_trace_channel(name, kind, value);
        if let Some(trace) = self.scene.trace_mut() { trace.channel(name, kind, value); }
    }

    /// Queues an app marker once for each attached writer. Formatting is skipped when off.
    #[inline]
    pub fn trace_event(&mut self, event: impl std::fmt::Display) {
        self.trace_event_dyn(&event);
    }

    pub(crate) fn trace_event_dyn(&mut self, event: &dyn std::fmt::Display) {
        if self.frame.tracing_channels() { event_log::push(&mut self.events, event.to_string()); }
        if let Some(trace) = self.scene.trace_mut() { trace.event(event); }
    }
}
