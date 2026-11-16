//! UI-loop effects for an app which keeps its hosts while it saves. A retained slot is never
//! reused by `Windows`; a window closed before a reopen cannot be confused with a replacement.
use super::*;
use crate::deferred_quit::{Effect, Event, Message, State, hooks, step};

#[derive(Clone, Copy)]
pub(crate) enum Reveal {
    Hidden,
    Shown,
    Front,
    Minimized,
}
#[derive(Default)]
pub(super) struct Session {
    pub(super) state: State,
    seq: crate::machine::Seq,
    retained: Vec<(usize, Reveal, Instant)>,
    pub(super) status: Option<String>,
    pub(super) parked: Vec<AnyWindowSpec>,
    requests: Vec<(usize,WakeEvent)>,
}

impl Session {
    pub(super) fn parked_host(&self,slot:usize)->bool { self.retained.iter().any(|(i,_,_)|*i==slot) }

    /// A display tick is spent, not replayed. Native operations and GPU/slot
    /// completions are delivered in order after the original hosts are back.
    pub(super) fn retain_request(&mut self,slot:usize,event:WakeEvent){
        match event {
            WakeEvent::DisplayTick(_)|WakeEvent::Live(_)=>{},
            WakeEvent::App(_)|WakeEvent::A11y(_)|WakeEvent::GpuReady(_)|WakeEvent::FrameSlot(_)
                if self.requests.iter().any(|(i,e)|*i==slot && std::mem::discriminant(e)==std::mem::discriminant(&event))=>{},
            e=>self.requests.push((slot,e)),
        }
    }
    pub(super) fn pending(&self) -> bool {
        matches!(self.state, State::Quitting { .. } | State::Saving { .. } | State::Overdue { .. }) || self.consented()
    }
    #[cfg(windows)] pub(super) fn consented(&self) -> bool { matches!(self.state, State::AwaitSessionEnd(..)) }
    #[cfg(not(windows))] pub(super) fn consented(&self) -> bool { false }
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
            #[cfg(windows)] Message::SessionQuery(id,now) => {
                use crate::deferred_quit::session::{self,Query};
                if session::matches(id) && matches!(session::current(),Query::Asking|Query::Blocked){self.deferred_step(el,Event::SystemQuery(now));}
            }
            #[cfg(windows)] Message::SessionCancel(id) => {
                if crate::deferred_quit::session::matches(id){self.deferred_step(el,Event::CancelSession);crate::deferred_quit::session::clear();}
            }
            Message::Legacy(args) => {
                if !el.exiting() && self.deferred.state.accepts_activation() { self.deliver_launch(el,&args); }
                #[cfg(any(windows,target_os="linux"))]
                if el.exiting() { crate::relaunch::spawn_with(args); }
            }
            #[cfg(target_os = "macos")]
            Message::Link(url) => {
                self.resume_quit(el);
                crate::open_url::incoming::receive(url);
            }
            Message::Event(e) => self.deferred_step(el, e),
            Message::Close(i) => {
                if self.deferred.pending() && let Some(status) = self.deferred.status.as_deref()
                    && self.hosts.get(i).and_then(Option::as_ref).and_then(|h| h.key()) == Some(status) {
                    if let (Some(hooks), Some(k)) = (hooks(), self.deferred.state.ticket()) { (hooks.status_closed)(k); }
                    return;
                }
                // A hidden window waiting for the save is not closed by a stray close: closing
                // would save and forget it, and a resume must bring back every window that was there.
                if self.deferred.pending() && self.deferred.parked_host(i) { return; }
                if self.last_quit_host(i) && self.hosts.get(i).and_then(Option::as_ref).is_some_and(|h| h.close_quits()) {
                    self.deferred_step(el, Event::Quit(Instant::now(), crate::quit::After::Exit));
                } else if let Some(Some(h)) = self.hosts.get_mut(i) {
                    h.close_one(el);
                }
            }
            Message::Activate(a) => {
                if Instant::now() >= a.until || el.exiting() || !self.deferred.state.accepts_activation() || !(a.claim)() {
                    a.answer(false);
                    return;
                }
                a.answer(true);
                self.deliver_launch(el,&a.args);
            }
        }
    }
    fn deliver_launch(&mut self, el:&ActiveEventLoop,args:&[String]) {
        let has_link=args.iter().any(|s|crate::open_url::instance::is_link(s));
        if !self.resume_quit(el) && !has_link { self.reopen(el); }
        if let Some(hooks)=hooks() { (hooks.activated)(args); }
        for url in args.iter().filter(|s|crate::open_url::instance::is_link(s)) {
            crate::open_url::incoming::receive(url.clone());
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
        #[cfg(windows)]
        if self.deferred.pending() && matches!(e,Event::Receipt(..)|Event::Empty(..)) && matches!(crate::deferred_quit::session::current(),crate::deferred_quit::session::Query::Asking|crate::deferred_quit::session::Query::Blocked){
            self.deferred_step(el,Event::SystemQuery(Instant::now()));
        }
        #[cfg(windows)]
        let system=matches!(self.deferred.state, State::Saving{after:crate::deferred_quit::Intent::System,..}|State::Overdue{after:crate::deferred_quit::Intent::System,..});
        #[cfg(windows)]
        let late=system && crate::deferred_quit::session::current()==crate::deferred_quit::session::Query::Blocked;
        #[cfg(windows)]
        let seal=if system && !late {crate::deferred_quit::Seal::Query}else{crate::deferred_quit::Seal::Exit};
        #[cfg(not(windows))]
        let seal=crate::deferred_quit::Seal::Exit;
        for fx in step(&mut self.deferred.state, &mut self.deferred.seq, e) {
            match fx {
                #[cfg(target_os = "windows")]
                Effect::Consent => {
                    use crate::deferred_quit::session::{self,Query};
                    match session::current(){
                        Query::Asking=>session::consent(),
                        _=>self.deferred_step(el,Event::CancelSession),
                    }
                }
                #[cfg(target_os = "windows")]
                Effect::SessionExit(k,r) => {
                    crate::deferred_quit::commit_exit();
                    if !(hooks.try_seal)(k,r,crate::deferred_quit::Seal::Exit){crate::marks::mark("quit.session_unconfirmed");std::process::exit(2);}
                    crate::marks::mark("quit.session_end");
                    el.exit();
                }
                #[cfg(windows)]
                Effect::LeaveSystem=>{crate::deferred_quit::commit_exit();crate::marks::mark("quit.session_saved_after_veto");el.exit();}
                Effect::HideAndPrepare(k) => {
                    // SW_HIDE moves focus synchronously. Read *every* window
                    // before hiding the first one or a sibling can appear front twice.
                    let since=Instant::now();
                    self.deferred.retained=self.hosts.iter().enumerate().filter_map(|(i,h)|{
                        let h=h.as_ref()?;(!h.finished()).then(||(i,h.quit_reveal(),since))
                    }).collect();
                    for &(i,_,_) in &self.deferred.retained{if let Some(h)=self.hosts[i].as_mut(){h.suspend_quit(el);}}
                    crate::marks::mark("quit.hidden");
                    (hooks.begin)(k);
                    self.deferred_step(el, Event::Prepared(k));
                }
                Effect::Status(k) => {
                    self.deferred.status = Some((hooks.overdue)(k).key().to_owned());
                    self.open_requested(el);
                }
                Effect::CheckRevision(k, revision) => {
                    // No event-loop turn between closing admission and committing the quit.
                    if (hooks.try_seal)(k, revision, seal) {
                        #[cfg(windows)] if late{self.deferred_step(el,Event::LateSystemSeal(k));continue;}
                        self.deferred_step(el, Event::Sealed(k,revision));
                    }
                }
                Effect::CheckEmpty(k,r) => {
                    if (hooks.try_seal)(k,r,seal) {
                        #[cfg(windows)] if late{self.deferred_step(el,Event::LateSystemSeal(k));continue;}
                        self.deferred_step(el,Event::EmptySealed(k,r));
                    }
                }
                Effect::ExitEmpty => {
                    crate::deferred_quit::commit_exit();
                    crate::marks::mark("quit.no_persistent_changes"); el.exit();
                }
                Effect::Restore(k) => {
                    self.deferred.status = None;
                    #[cfg(target_os = "windows")]
                    crate::deferred_quit::session::clear();
                    (hooks.resumed)(k);
                    #[cfg(target_os = "macos")]
                    {
                        crate::quit::mac::reply(false);
                        crate::platform::reopen::unhide();
                    }
                    self.deferred.retained.sort_by_key(|(_, mode,_)| matches!(mode, Reveal::Front));
                    for (i, mode,since) in self.deferred.retained.drain(..) {
                        if let Some(Some(h)) = self.hosts.get_mut(i) {
                            h.restore_quit(el, mode,since.elapsed());
                        }
                    }
                    for (i,event) in self.deferred.requests.drain(..){
                        if let Some(Some(h))=self.hosts.get_mut(i).filter(|s|s.as_ref().is_some_and(|h|!h.finished())){h.user_event(el,event);}
                    }
                    crate::marks::mark("quit.resumed");
                }
                Effect::Finalize(k, after) => {
                    crate::deferred_quit::commit_exit();
                    #[cfg(target_os = "windows")]
                    crate::deferred_quit::session::clear();
                    crate::marks::mark("quit.confirmed");
                    (hooks.confirmed)(k, match after { crate::quit::After::Exit => crate::deferred_quit::Then::Exit, crate::quit::After::Relaunch => crate::deferred_quit::Then::Relaunch });
                    for h in self.hosts.iter_mut().flatten(){h.committed_quit();}
                    let _ = crate::quit::send(crate::quit::Event::Start { ending: false, after });
                    #[cfg(target_os = "macos")]
                    crate::quit::mac::reply(true);
                    el.exit();
                }
                Effect::ForceExit => {
                    crate::deferred_quit::commit_exit();
                    crate::marks::mark("quit.forced");
                    std::process::exit(2);
                }
            }
        }
    }
}

#[cfg(windows)]
pub(super) fn resume_surface(w:&mut Win,paused:Duration){
    use super::cpu_frame::{GpuJob,SurfaceMode};
    // The UI could not draw a hand-over frame while parked. Don't count that
    // time as a device timeout and tear down an otherwise healthy surface.
    match &mut w.native.mode {
        SurfaceMode::Cpu{job:GpuJob::Waiting{retry_at:Some(at),..},..}=>*at+=paused,
        SurfaceMode::Cpu{job:GpuJob::Building{since,ready,..},..}=>{*since+=paused;if let Some(at)=ready{*at+=paused;}}
        SurfaceMode::HandingOver{since,nudged,done,..}=>{*since+=paused;if let Some(at)=nudged{*at+=paused;}if let Some((_,at))=done{*at+=paused;}}
        _=>{}
    }
}
