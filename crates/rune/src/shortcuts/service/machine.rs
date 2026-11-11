//! The service's state: whether an App has a host, and what its engine holds. These are two
//! enums because they live in different places. The attachment is read from any thread, while
//! a macOS or Windows engine never leaves its own, and a Linux engine sits in its own slot, whose
//! lock a closing backend holds so that nothing reaches the next engine until its chords are free.

use super::super::{Shortcut, ShortcutError, ShortcutId};
use crate::{machine::Effects, runtime::Waker};
use std::{collections::HashMap, mem, thread::ThreadId};

pub(super) type Registrations = HashMap<ShortcutId, Shortcut>;

/// Whether an App has a host. `J` is a call waiting for the owner thread.
pub(super) enum Attachment<J> {
    /// No host has attached yet.
    Detached,
    Attached { owner: ThreadId, wake: Waker, jobs: Vec<J> },
    /// The host closed. Calls are refused, and a later host can still attach.
    Closed,
}

// Linux calls reach the engine directly, so nothing posts or drains there.
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(super) enum AttachEvent<J> {
    Attach { owner: ThreadId, wake: Waker },
    /// A call from `from`. When `shared`, any thread can reach the engine, so none waits.
    Call { from: ThreadId, shared: bool },
    /// The job of a call told to `Wait`, sent under the same lock.
    Post(J),
    Drain,
    Close,
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(super) enum AttachEffect<J> {
    /// Nothing is attached: the call fails with `NotRunning`.
    Refuse,
    /// Run the call on this thread.
    Run { wake: Waker },
    /// Post a job and wait for the owner to run it.
    Wait,
    Wake(Waker),
    /// Run these on the owner thread.
    Jobs(Vec<J>),
    /// The host closed: release its waiting callers.
    Release(Vec<J>),
}

pub(super) fn step_attachment<J>(state: &mut Attachment<J>, event: AttachEvent<J>) -> Effects<AttachEffect<J>> {
    match event {
        AttachEvent::Attach { owner, wake } => {
            // Callers already waiting stay queued for the new host.
            let jobs = match state { Attachment::Attached { jobs, .. } => mem::take(jobs), _ => Vec::new() };
            *state = Attachment::Attached { owner, wake, jobs };
            Effects::none()
        }
        AttachEvent::Call { from, shared } => match state {
            Attachment::Attached { owner, wake, .. } if shared || *owner == from => Effects::one(AttachEffect::Run { wake: wake.clone() }),
            Attachment::Attached { .. } => Effects::one(AttachEffect::Wait),
            _ => Effects::one(AttachEffect::Refuse),
        },
        AttachEvent::Post(job) => match state {
            Attachment::Attached { wake, jobs, .. } => {
                jobs.push(job);
                Effects::one(AttachEffect::Wake(wake.clone()))
            }
            _ => Effects::none(),
        },
        AttachEvent::Drain => match state {
            Attachment::Attached { jobs, .. } if !jobs.is_empty() => Effects::one(AttachEffect::Jobs(mem::take(jobs))),
            _ => Effects::none(),
        },
        AttachEvent::Close => match mem::replace(state, Attachment::Closed) {
            Attachment::Attached { jobs, .. } => Effects::one(AttachEffect::Release(jobs)),
            _ => Effects::none(),
        },
    }
}

/// What an engine holds. `N` is the native backend.
pub(super) enum Bindings<N> {
    /// Headless: registrations are virtual and no backend ever connects.
    Virtual(Registrations),
    /// No backend. Nothing is registered, and nothing connects until something is.
    Disconnected,
    /// A backend and what it holds, which is nothing only until a first registration settles.
    Connected { native: N, registrations: Registrations },
}

impl<N> Bindings<N> {
    /// The registration of `shortcut`, if this engine holds one.
    pub(super) fn find(&self, shortcut: &Shortcut) -> Option<ShortcutId> {
        let (Self::Virtual(registrations) | Self::Connected { registrations, .. }) = self else { return None };
        find(registrations, shortcut)
    }
}

fn find(registrations: &Registrations, shortcut: &Shortcut) -> Option<ShortcutId> {
    registrations.iter().find(|(_, s)| *s == shortcut).map(|(id, _)| *id)
}

pub(super) enum BindEvent<N> {
    /// `claimed`: another App in this process holds the chord.
    Register { id: ShortcutId, shortcut: Shortcut, claimed: bool },
    /// The backend asked for by `Connect` opened.
    Opened { native: N, id: ShortcutId, shortcut: Shortcut },
    /// The backend accepted a `Bind`.
    Bound { id: ShortcutId, shortcut: Shortcut },
    /// The backend refused a `Bind`.
    Rejected(ShortcutError),
    Unregister(ShortcutId),
    /// The backend carried out an `Unbind`.
    Unbound(ShortcutId),
    Release,
}

pub(super) enum BindEffect<N> {
    /// The operation fails with this error.
    Refuse(ShortcutError),
    /// Open a backend, then answer `Opened`.
    Connect { id: ShortcutId, shortcut: Shortcut },
    /// Register with the backend, then answer `Bound` or `Rejected`.
    Bind { id: ShortcutId, shortcut: Shortcut },
    /// Unregister from the backend, then answer `Unbound`.
    Unbind(ShortcutId),
    /// Record the chord against this App for the whole process.
    Claim { id: ShortcutId, shortcut: Shortcut },
    Disclaim(ShortcutId),
    DisclaimAll(Registrations),
    /// Close this backend.
    Disconnect(N),
}

pub(super) fn step_bindings<N>(state: &mut Bindings<N>, event: BindEvent<N>) -> Effects<BindEffect<N>> {
    match event {
        BindEvent::Register { id, shortcut, claimed } => match state {
            Bindings::Virtual(registrations) => {
                if find(registrations, &shortcut).is_some() { return Effects::one(BindEffect::Refuse(ShortcutError::Conflict)); }
                registrations.insert(id, shortcut);
                Effects::none()
            }
            Bindings::Disconnected if claimed => Effects::one(BindEffect::Refuse(ShortcutError::Conflict)),
            Bindings::Disconnected => Effects::one(BindEffect::Connect { id, shortcut }),
            Bindings::Connected { registrations, .. } => {
                if claimed || find(registrations, &shortcut).is_some() { return Effects::one(BindEffect::Refuse(ShortcutError::Conflict)); }
                Effects::one(BindEffect::Bind { id, shortcut })
            }
        },
        BindEvent::Opened { native, id, shortcut } => {
            *state = Bindings::Connected { native, registrations: HashMap::new() };
            Effects::one(BindEffect::Bind { id, shortcut })
        }
        BindEvent::Bound { id, shortcut } => {
            let Bindings::Connected { registrations, .. } = state else { return Effects::none() };
            registrations.insert(id, shortcut.clone());
            Effects::one(BindEffect::Claim { id, shortcut })
        }
        BindEvent::Rejected(error) => {
            let mut effects = Effects::none();
            effects.extend(disconnect_if_empty(state));
            effects.push(BindEffect::Refuse(error));
            effects
        }
        BindEvent::Unregister(id) => match state {
            Bindings::Virtual(registrations) => match registrations.remove(&id) {
                Some(_) => Effects::none(),
                None => Effects::one(BindEffect::Refuse(ShortcutError::UnknownId)),
            },
            Bindings::Connected { registrations, .. } if registrations.contains_key(&id) => Effects::one(BindEffect::Unbind(id)),
            _ => Effects::one(BindEffect::Refuse(ShortcutError::UnknownId)),
        },
        BindEvent::Unbound(id) => {
            let Bindings::Connected { registrations, .. } = state else { return Effects::none() };
            registrations.remove(&id);
            let mut effects = Effects::one(BindEffect::Disclaim(id));
            effects.extend(disconnect_if_empty(state));
            effects
        }
        BindEvent::Release => match state {
            Bindings::Virtual(registrations) => {
                registrations.clear();
                Effects::none()
            }
            Bindings::Disconnected => Effects::none(),
            Bindings::Connected { .. } => {
                let Bindings::Connected { native, registrations } = mem::replace(state, Bindings::Disconnected) else { return Effects::none() };
                Effects::two(BindEffect::Disconnect(native), BindEffect::DisclaimAll(registrations))
            }
        },
    }
}

/// Close the connection when nothing is left on it.
fn disconnect_if_empty<N>(state: &mut Bindings<N>) -> Option<BindEffect<N>> {
    if !matches!(state, Bindings::Connected { registrations, .. } if registrations.is_empty()) { return None; }
    match mem::replace(state, Bindings::Disconnected) {
        Bindings::Connected { native, .. } => Some(BindEffect::Disconnect(native)),
        _ => None,
    }
}
