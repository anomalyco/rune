//! UI-loop effects for an app which keeps its hosts while it saves. A retained slot is never
//! reused by `Windows`; a window closed before a reopen cannot be confused with a replacement.
use super::*;
use crate::deferred_quit::{Effect, Event, Message, State, hooks, step};

#[derive(Clone, Copy)]
pub(super) enum Reveal {
    Shown,
    Front,
    Minimized,
}
#[derive(Default)]
pub(super) struct Session {
    pub(super) state: State,
    seq: crate::machine::Seq,
    retained: Vec<(usize, Reveal)>,
}

impl Windows {
    pub(super) fn last_quit_host(&self, slot: usize) -> bool {
        self.hosts.iter().enumerate().all(|(i, h)| i == slot || !h.as_ref().is_some_and(|h| h.keeps_loop() && !h.finished()))
    }
    pub(super) fn resume_quit(&mut self, el: &ActiveEventLoop) -> bool {
        if matches!(self.deferred.state, State::Quitting { .. } | State::Saving { .. } | State::Overdue { .. }) {
            self.deferred_step(el, Event::Reopen);
            true
        } else {
            false
        }
    }
    pub(super) fn deferred_message(&mut self, el: &ActiveEventLoop, message: Message) {
        match message {
            #[cfg(target_os = "macos")]
            Message::Link(url) => {
                self.resume_quit(el);
                crate::open_url::incoming::receive(url);
            }
            Message::Event(e) => self.deferred_step(el, e),
            Message::Close(i) => {
                if self.last_quit_host(i) && self.hosts.get(i).and_then(Option::as_ref).is_some_and(|h| h.close_quits()) {
                    self.deferred_step(el, Event::Quit(Instant::now(), crate::quit::After::Exit));
                } else if let Some(Some(h)) = self.hosts.get_mut(i) {
                    h.close_one(el);
                }
            }
            Message::Activate(a) => {
                if Instant::now() >= a.until || el.exiting() || !self.deferred.state.accepts_activation() {
                    a.answer(false);
                    return;
                }
                if !self.resume_quit(el) {
                    self.reopen(el);
                }
                for link in a.args.iter().filter(|arg| crate::open_url::instance::is_link(arg)) {
                    crate::open_url::incoming::receive(link.clone());
                }
                a.answer(true);
            }
        }
    }
    pub(super) fn deferred_deadline(&mut self, el: &ActiveEventLoop, now: Instant) -> Option<Instant> {
        let (k, due) = self.deferred.state.due()?;
        if now < due {
            return Some(due);
        }
        self.deferred_step(el, Event::Due(k, now));
        None
    }
    pub(super) fn deferred_step(&mut self, el: &ActiveEventLoop, e: Event) {
        let Some(hooks) = hooks() else { return };
        for fx in step(&mut self.deferred.state, &mut self.deferred.seq, e) {
            match fx {
                #[cfg(target_os = "windows")]
                Effect::Consent => crate::deferred_quit::resolved(true),
                #[cfg(target_os = "windows")]
                Effect::SessionExit => {
                    crate::marks::mark("quit.session_end");
                    el.exit();
                }
                Effect::HideAndPrepare(k) => {
                    self.deferred.retained.clear();
                    for (i, h) in self.hosts.iter_mut().enumerate() {
                        if let Some(mode) = h.as_mut().and_then(|h| h.suspend_quit(el)) {
                            self.deferred.retained.push((i, mode));
                        }
                    }
                    crate::marks::mark("quit.hidden");
                    (hooks.begin)(k);
                    self.deferred_step(el, Event::Prepared(k));
                }
                Effect::Status(k) => (hooks.overdue)(k),
                Effect::CheckRevision(k, revision) => {
                    // No event-loop turn between closing admission and committing the quit.
                    if (hooks.try_seal)(k, revision) { self.deferred_step(el, Event::Sealed(k)); }
                }
                Effect::Restore(k) => {
                    #[cfg(target_os = "windows")]
                    crate::deferred_quit::resolved(false);
                    (hooks.resumed)(k);
                    #[cfg(target_os = "macos")]
                    {
                        crate::quit::mac::reply(false);
                        crate::platform::reopen::unhide();
                    }
                    self.deferred.retained.sort_by_key(|(_, mode)| matches!(mode, Reveal::Front));
                    for (i, mode) in self.deferred.retained.drain(..) {
                        if let Some(Some(h)) = self.hosts.get_mut(i) {
                            h.restore_quit(el, mode);
                        }
                    }
                    crate::marks::mark("quit.resumed");
                }
                Effect::Finalize(k, after) => {
                    #[cfg(target_os = "windows")]
                    crate::deferred_quit::resolved(true);
                    crate::marks::mark("quit.confirmed");
                    (hooks.confirmed)(k);
                    let _ = crate::quit::send(crate::quit::Event::Start { ending: false, after });
                    #[cfg(target_os = "macos")]
                    crate::quit::mac::reply(true);
                    el.exit();
                }
                Effect::ForceExit => {
                    crate::marks::mark("quit.forced");
                    std::process::exit(2);
                }
            }
        }
    }
}
