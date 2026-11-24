//! xdg-desktop-portal GlobalShortcuts. One portal session holds the app's entire active set of
//! shortcuts: startup shortcuts bind together in one `BindShortcuts` call, runtime changes close
//! the current session and bind the new full set in a fresh session, and removing the last
//! shortcut closes the session without rebinding.
use super::{
    Backend, Shortcut, ShortcutDetails, ShortcutError, ShortcutId, Sink,
    wayland_session::{self, BindOutcome, Effect, Event, PortalShortcut, State},
};
use crate::{
    machine::{Attempt, Effects, Seq},
    platform::portal::{self, DEST, PATH, RequestError, RequestTimeouts},
};
use async_channel::{Receiver, Sender, unbounded};
use async_executor::{LocalExecutor, Task};
use futures_lite::{FutureExt, StreamExt};
use std::{collections::HashMap, sync::mpsc, thread::JoinHandle, time::Duration};
use zbus::{
    Connection, Proxy,
    proxy::SignalStream,
    zvariant::{OwnedObjectPath, OwnedValue, Type, Value},
};

const INTERFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
const RPC_TIMEOUT: Duration = Duration::from_secs(5);
const CONSENT_TIMEOUT: Duration = Duration::from_secs(120);
type BatchReply = mpsc::SyncSender<Vec<Result<(), ShortcutError>>>;
type UnitReply = mpsc::SyncSender<Result<(), ShortcutError>>;
type BindDone = (Attempt, Result<BindOutcome<OwnedObjectPath>, ShortcutError>);
enum Command {
    Register(Vec<PortalShortcut>, BatchReply),
    Unregister(ShortcutId, UnitReply),
}
pub(super) struct Native { commands: Option<Sender<Command>>, worker: Option<JoinHandle<()>> }

fn dbus_error(e: zbus::Error) -> ShortcutError {
    if portal::is_unavailable(&e) {
        return ShortcutError::Unsupported;
    }
    if let zbus::Error::MethodError(name, _, _) = &e {
        return match name.as_str() {
            "org.freedesktop.portal.Error.Exist" => ShortcutError::Conflict,
            "org.freedesktop.portal.Error.NotAllowed" | "org.freedesktop.portal.Error.Cancelled" => ShortcutError::PermissionDenied,
            "org.freedesktop.portal.Error.InvalidArgument" => ShortcutError::InvalidAccelerator(e.to_string()),
            _ => ShortcutError::Backend(e.to_string()),
        };
    }
    ShortcutError::Backend(e.to_string())
}
fn response_status(status: u32) -> Result<(), ShortcutError> {
    match status { 0 => Ok(()), 1 => Err(ShortcutError::PermissionDenied), n => Err(ShortcutError::Backend(format!("portal response {n}"))) }
}
fn timed_out() -> ShortcutError { ShortcutError::Backend("portal request timed out".into()) }

async fn request<B: serde::Serialize + Type>(
    conn: &Connection,
    portal_proxy: &Proxy<'_>,
    token: &str,
    method: &str,
    body: &B,
    cancel: &Receiver<()>,
) -> Result<HashMap<String, OwnedValue>, ShortcutError> {
    let (status, values) = portal::request(
        conn,
        portal_proxy,
        token,
        method,
        body,
        RequestTimeouts { rpc: RPC_TIMEOUT, response: Some(CONSENT_TIMEOUT) },
        async { cancel.recv().await.ok(); },
    ).await.map_err(|err| match err {
        RequestError::Dbus(e) => dbus_error(e),
        RequestError::Unavailable => ShortcutError::Unsupported,
        RequestError::TimedOut => timed_out(),
        RequestError::Disconnected => ShortcutError::Backend("portal disconnected".into()),
        RequestError::Cancelled => ShortcutError::NotRunning,
    })?;
    response_status(status)?;
    Ok(values)
}

async fn close(conn: &Connection, session: &OwnedObjectPath) -> Result<(), ShortcutError> {
    portal::timeout(RPC_TIMEOUT, async {
        let proxy = Proxy::new(conn, DEST, session.as_str(), "org.freedesktop.portal.Session").await.map_err(dbus_error)?;
        proxy.call::<_, _, ()>("Close", &()).await.map_err(dbus_error)
    }).await.unwrap_or_else(|| Err(timed_out()))
}

async fn bind_set(
    conn: &Connection,
    portal_proxy: &Proxy<'_>,
    shortcuts: &[PortalShortcut],
    cancel: Receiver<()>,
) -> Result<BindOutcome<OwnedObjectPath>, ShortcutError> {
    let create = portal::next_token("shortcuts_create");
    let session_token = portal::next_token("shortcuts_session");
    let options = HashMap::from([
        ("handle_token", Value::from(create.as_str())),
        ("session_handle_token", Value::from(session_token.as_str())),
    ]);
    let response = request(conn, portal_proxy, &create, "CreateSession", &(options,), &cancel).await?;
    let value = response.get("session_handle").ok_or_else(|| ShortcutError::Backend("portal omitted session_handle".into()))?;
    let session = if let Ok(path) = <&str>::try_from(value) {
        OwnedObjectPath::try_from(path.to_owned()).map_err(|e| ShortcutError::Backend(e.to_string()))?
    } else {
        OwnedObjectPath::try_from(value.try_clone().map_err(|e| ShortcutError::Backend(e.to_string()))?)
            .map_err(|e| ShortcutError::Backend(e.to_string()))?
    };
    let bind_token = portal::next_token("shortcuts_bind");
    let options = HashMap::from([("handle_token", Value::from(bind_token.as_str()))]);
    let entries: Vec<(&str, HashMap<&str, Value<'_>>)> = shortcuts.iter().map(|s| {
        let info = HashMap::from([
            ("description", Value::from(s.description.as_str())),
            ("preferred_trigger", Value::from(s.preferred_trigger.as_str())),
        ]);
        (s.portal_id.as_str(), info)
    }).collect();
    match request(conn, portal_proxy, &bind_token, "BindShortcuts", &(&session, entries, "", options), &cancel).await {
        Ok(response) => {
            let bound_portal_ids = response.get("shortcuts").and_then(|v| v.try_clone().ok())
                .and_then(|v| Vec::<(String, HashMap<String, OwnedValue>)>::try_from(v).ok())
                .map(|v| v.into_iter().map(|(key, _)| key).collect())
                .unwrap_or_default();
            Ok(BindOutcome { session, bound_portal_ids })
        }
        Err(e) => {
            close(conn, &session).await.ok();
            Err(e)
        }
    }
}

fn to_portal_shortcut(id: ShortcutId, shortcut: &Shortcut, details: &ShortcutDetails) -> PortalShortcut {
    PortalShortcut {
        id,
        portal_id: details.portal_id(shortcut),
        description: details.portal_description(shortcut),
        preferred_trigger: shortcut.portal_trigger(),
    }
}

async fn apply_effects(
    effects: Effects<Effect<OwnedObjectPath, BatchReply, UnitReply>>,
    conn: &Connection,
    portal: &Proxy<'static>,
    local: &LocalExecutor<'_>,
    done_tx: &Sender<BindDone>,
    in_flight: &mut Option<(Attempt, Sender<()>, Task<()>)>,
    sink: &Sink,
) {
    for effect in effects {
        match effect {
            Effect::CloseSession(session) => { close(conn, &session).await.ok(); }
            Effect::Bind { attempt, shortcuts } => {
                let (cancel_tx, cancel_rx) = async_channel::bounded::<()>(1);
                let (conn, portal, done_tx) = (conn.clone(), portal.clone(), done_tx.clone());
                let task = local.spawn(async move {
                    let result = bind_set(&conn, &portal, &shortcuts, cancel_rx).await;
                    done_tx.try_send((attempt, result)).ok();
                });
                *in_flight = Some((attempt, cancel_tx, task));
            }
            Effect::CancelBind(attempt) => {
                if let Some((current, cancel_tx, task)) = in_flight.take() {
                    if current == attempt {
                        drop(cancel_tx);
                        task.await;
                    } else {
                        *in_flight = Some((current, cancel_tx, task));
                    }
                }
            }
            Effect::ReplyRegister { reply, results } => { reply.send(results).ok(); }
            Effect::ReplyUnregister { reply, result } => { reply.send(result).ok(); }
            Effect::Activate { id, token } => sink(id, token),
        }
    }
}

async fn watch_session_closed(
    conn: &Connection,
    state: &State<OwnedObjectPath, BatchReply, UnitReply>,
    session_closed: &mut Option<(OwnedObjectPath, SignalStream<'static>)>,
) {
    match state {
        State::Bound { session, .. } => {
            if session_closed.as_ref().is_some_and(|(path, _)| path == session) { return; }
            *session_closed = async {
                let proxy = Proxy::new(conn, DEST, session.clone(), "org.freedesktop.portal.Session").await.ok()?;
                let stream = proxy.receive_signal("Closed").await.ok()?;
                Some((session.clone(), stream))
            }.await;
        }
        _ => { *session_closed = None; }
    }
}

impl Native {
    pub(super) fn new(sink: Sink) -> Result<Self, ShortcutError> {
        let (commands, receiver) = unbounded();
        let (ready, result) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new().name("rune-shortcuts-portal".into()).spawn(move || {
            let local = LocalExecutor::new();
            async_io::block_on(local.run(async {
                let setup = portal::timeout(RPC_TIMEOUT, async {
                    // No session bus means this desktop cannot supply the portal.
                    let conn = Connection::session().await.map_err(|_| ShortcutError::Unsupported)?;
                    portal::register_app_id(&conn).await;
                    let portal = Proxy::new(&conn, DEST, PATH, INTERFACE).await.map_err(dbus_error)?;
                    // Some portal frontends report InvalidArgs rather than UnknownInterface
                    // when a backend for this interface is absent. The property is fixed here.
                    let _: u32 = portal.get_property("version").await.map_err(|_| ShortcutError::Unsupported)?;
                    let events = portal.receive_signal("Activated").await.map_err(dbus_error)?;
                    Ok::<_, ShortcutError>((conn, portal, events))
                }).await.unwrap_or(Err(ShortcutError::Unsupported));
                let (conn, portal, mut events) = match setup {
                    Ok(v) => { ready.send(Ok(())).ok(); v }
                    Err(e) => { ready.send(Err(e)).ok(); return; }
                };
                let (done_tx, done_rx) = unbounded::<BindDone>();
                let mut seq = Seq::default();
                let mut state: State<OwnedObjectPath, BatchReply, UnitReply> = State::default();
                let mut in_flight: Option<(Attempt, Sender<()>, Task<()>)> = None;
                let mut session_closed: Option<(OwnedObjectPath, SignalStream<'static>)> = None;
                enum Wake {
                    Command(Option<Command>),
                    BindDone(Option<BindDone>),
                    Activated(Option<zbus::Message>),
                    SessionClosed(OwnedObjectPath),
                }
                loop {
                    let wait_closed = async {
                        match session_closed.as_mut() {
                            Some((path, stream)) => match stream.next().await {
                                Some(_) => Wake::SessionClosed(path.clone()),
                                None => std::future::pending().await,
                            },
                            None => std::future::pending().await,
                        }
                    };
                    let next = async { Wake::Command(receiver.recv().await.ok()) }
                        .or(async { Wake::BindDone(done_rx.recv().await.ok()) })
                        .or(async { Wake::Activated(events.next().await) })
                        .or(wait_closed)
                        .await;
                    let event = match next {
                        Wake::Command(Some(Command::Register(items, reply))) => Event::Register { items, reply },
                        Wake::Command(Some(Command::Unregister(id, reply))) => Event::Unregister { id, reply },
                        Wake::BindDone(Some((attempt, result))) => {
                            if in_flight.as_ref().is_some_and(|(a, ..)| *a == attempt) {
                                in_flight = None;
                            }
                            Event::BindFinished { attempt, result }
                        }
                        Wake::Activated(Some(event)) => {
                            let Ok((session, portal_id, _, options)) = event.body().deserialize::<(OwnedObjectPath, String, u64, HashMap<String, OwnedValue>)>() else {
                                continue;
                            };
                            let token = options.get("activation_token").and_then(|v| <&str>::try_from(v).ok()).map(str::to_owned);
                            Event::Activated { session, portal_id, token }
                        }
                        Wake::SessionClosed(session) => {
                            session_closed = None;
                            Event::SessionClosed { session }
                        }
                        Wake::Command(None) | Wake::Activated(None) | Wake::BindDone(None) => {
                            session_closed = None;
                            let effects = wayland_session::step(&mut seq, &mut state, Event::Shutdown);
                            apply_effects(effects, &conn, &portal, &local, &done_tx, &mut in_flight, &sink).await;
                            break;
                        }
                    };
                    let effects = wayland_session::step(&mut seq, &mut state, event);
                    apply_effects(effects, &conn, &portal, &local, &done_tx, &mut in_flight, &sink).await;
                    watch_session_closed(&conn, &state, &mut session_closed).await;
                }
                conn.close().await.ok();
            }));
        }).map_err(|e| ShortcutError::Backend(e.to_string()))?;
        let mut native = Self { commands: Some(commands), worker: Some(worker) };
        if let Err(e) = result.recv().unwrap_or(Err(ShortcutError::Unsupported)) {
            native.commands.take();
            return Err(e);
        }
        Ok(native)
    }

    fn send_batch(&self, items: Vec<PortalShortcut>) -> Vec<Result<(), ShortcutError>> {
        let count = items.len();
        let Some(commands) = self.commands.as_ref() else { return vec![Err(ShortcutError::NotRunning); count]; };
        let (reply, result) = mpsc::sync_channel(1);
        if commands.try_send(Command::Register(items, reply)).is_err() {
            return vec![Err(ShortcutError::Backend("portal disconnected".into())); count];
        }
        result.recv().unwrap_or_else(|_| vec![Err(ShortcutError::Backend("portal disconnected".into())); count])
    }

    fn send_unregister(&self, id: ShortcutId) -> Result<(), ShortcutError> {
        let (reply, result) = mpsc::sync_channel(1);
        self.commands.as_ref().ok_or(ShortcutError::NotRunning)?
            .try_send(Command::Unregister(id, reply))
            .map_err(|_| ShortcutError::Backend("portal disconnected".into()))?;
        result.recv().unwrap_or_else(|_| Err(ShortcutError::Backend("portal disconnected".into())))
    }
}

impl Backend for Native {
    fn register(&mut self, id: ShortcutId, shortcut: &Shortcut) -> Result<(), ShortcutError> {
        self.register_with(id, shortcut, &ShortcutDetails::default())
    }
    fn register_with(&mut self, id: ShortcutId, shortcut: &Shortcut, details: &ShortcutDetails) -> Result<(), ShortcutError> {
        self.send_batch(vec![to_portal_shortcut(id, shortcut, details)]).into_iter().next().unwrap_or(Ok(()))
    }
    fn register_batch(&mut self, items: &[(ShortcutId, Shortcut, ShortcutDetails)]) -> Vec<Result<(), ShortcutError>> {
        let shortcuts = items.iter().map(|(id, shortcut, details)| to_portal_shortcut(*id, shortcut, details)).collect();
        self.send_batch(shortcuts)
    }
    fn unregister(&mut self, id: ShortcutId) -> Result<(), ShortcutError> { self.send_unregister(id) }
}

impl Drop for Native {
    fn drop(&mut self) {
        self.commands.take();
        if let Some(worker) = self.worker.take() { worker.join().ok(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::fdo;
    #[test]
    fn portal_cancellation_is_not_a_conflict() {
        assert_eq!(response_status(1), Err(ShortcutError::PermissionDenied));
        assert!(matches!(response_status(2), Err(ShortcutError::Backend(_))));
    }
    #[test]
    fn missing_portal_is_unsupported() {
        let fdo = |e| zbus::Error::FDO(Box::new(e));
        assert_eq!(dbus_error(fdo(fdo::Error::ServiceUnknown("portal".into()))), ShortcutError::Unsupported);
        assert_eq!(dbus_error(fdo(fdo::Error::UnknownMethod("BindShortcuts".into()))), ShortcutError::Unsupported);
        assert_eq!(dbus_error(fdo(fdo::Error::SpawnChildExited("portal".into()))), ShortcutError::Unsupported);
        assert!(matches!(dbus_error(fdo(fdo::Error::Failed("x".into()))), ShortcutError::Backend(_)));
    }
}
