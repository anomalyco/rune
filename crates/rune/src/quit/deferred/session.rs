//! A Windows query is permission to end the session, not the end itself. The
//! window procedure and Rune's loop run on one thread, sometimes nested. Mark
//! the intent here before posting it so a queued user receipt cannot race it.
use std::cell::Cell;
use std::time::Instant;
use super::{Message, post};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Query { Idle, Asking, Blocked, Allowed, Cancelled }
thread_local! { static CURRENT: Cell<(u64,Query)> = const { Cell::new((0,Query::Idle)) }; }

pub(crate) fn current() -> Query { CURRENT.get().1 }
pub(crate) fn matches(id:u64) -> bool { CURRENT.get().0 == id }
fn set(next:Query) { CURRENT.set((CURRENT.get().0,next)); }

pub(crate) fn ask() {
    let id=CURRENT.get().0.checked_add(1).expect("session query exhausted");
    CURRENT.set((id,Query::Asking));
    post(Message::SessionQuery(id,Instant::now()));
}
pub(crate) fn answered() -> bool {
    match current(){Query::Allowed=>true,Query::Asking=>{set(Query::Blocked);false},_=>false}
}
pub(crate) fn consent(){set(Query::Allowed);super::super::win32::clear_reason();}
pub(crate) fn cancel(){let id=CURRENT.get().0;set(Query::Cancelled);post(Message::SessionCancel(id));}
pub(crate) fn clear(){set(Query::Idle);super::super::win32::clear_reason();}
