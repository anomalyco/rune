//! xdg-desktop-portal GlobalShortcuts. Each binding owns a session so unregister works on
//! portal v1 (which cannot remove a binding from an existing session).
use super::{Backend, Shortcut, ShortcutError, ShortcutId, Sink};
use crate::platform::portal::{self, DEST, PATH, RequestError, RequestTimeouts};
use async_channel::{Sender, unbounded};
use futures_lite::{FutureExt, StreamExt};
use std::{collections::HashMap, sync::mpsc, thread::JoinHandle, time::Duration};
use zbus::{Connection, Proxy, zvariant::{OwnedObjectPath, OwnedValue, Type, Value}};

const INTERFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
const RPC_TIMEOUT: Duration = Duration::from_secs(5);
const CONSENT_TIMEOUT: Duration = Duration::from_secs(120);
type Reply = mpsc::SyncSender<Result<(), ShortcutError>>;
enum Command { Register(ShortcutId, Shortcut, Reply), Unregister(ShortcutId, Reply) }
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

async fn request<B: serde::Serialize + Type>(conn: &Connection, portal_proxy: &Proxy<'_>, token: &str,
    method: &str, body: &B) -> Result<HashMap<String, OwnedValue>, ShortcutError> {
    let (status, values) = portal::request(
        conn,
        portal_proxy,
        token,
        method,
        body,
        RequestTimeouts { rpc: RPC_TIMEOUT, response: Some(CONSENT_TIMEOUT) },
        std::future::pending(),
    ).await.map_err(|err| match err {
        RequestError::Dbus(e) => dbus_error(e),
        RequestError::Unavailable => ShortcutError::Unsupported,
        RequestError::TimedOut => timed_out(),
        RequestError::Disconnected | RequestError::Cancelled => ShortcutError::Backend("portal disconnected".into()),
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

async fn bind(conn: &Connection, portal_proxy: &Proxy<'_>, id: ShortcutId, shortcut: &Shortcut) -> Result<OwnedObjectPath, ShortcutError> {
    let create = portal::next_token("shortcuts_create");
    let session_token = portal::next_token("shortcuts_session");
    let options = HashMap::from([("handle_token", Value::from(create.as_str())), ("session_handle_token", Value::from(session_token.as_str()))]);
    let response = request(conn, portal_proxy, &create, "CreateSession", &(options,)).await?;
    let value = response.get("session_handle").ok_or_else(|| ShortcutError::Backend("portal omitted session_handle".into()))?;
    let session = if let Ok(path) = <&str>::try_from(value) {
        OwnedObjectPath::try_from(path.to_owned()).map_err(|e| ShortcutError::Backend(e.to_string()))?
    } else { OwnedObjectPath::try_from(value.try_clone().map_err(|e| ShortcutError::Backend(e.to_string()))?)
        .map_err(|e| ShortcutError::Backend(e.to_string()))? };
    let bind_token = portal::next_token("shortcuts_bind");
    let options = HashMap::from([("handle_token", Value::from(bind_token.as_str()))]);
    let trigger = shortcut.portal_trigger();
    let description = format!("Shortcut {shortcut}");
    let info = HashMap::from([("description", Value::from(description.as_str())), ("preferred_trigger", Value::from(trigger.as_str()))]);
    let shortcuts = vec![(id.0.to_string(), info)];
    match request(conn, portal_proxy, &bind_token, "BindShortcuts", &(&session, shortcuts, "", options)).await {
        Ok(response) => {
            // Success with no matching binding is not a registration. The compositor chooses
            // the actual trigger; a preferred-trigger conflict is not standardized by the portal.
            let bindings = response.get("shortcuts").and_then(|v| v.try_clone().ok())
                .and_then(|v| Vec::<(String, HashMap<String, OwnedValue>)>::try_from(v).ok());
            if bindings.as_ref().is_some_and(|v| v.iter().any(|(key, _)| key == &id.0.to_string())) { Ok(session) }
            else { close(conn, &session).await.ok(); Err(ShortcutError::PermissionDenied) }
        }
        Err(e) => { close(conn, &session).await.ok(); Err(e) }
    }
}

impl Native {
    pub(super) fn new(sink: Sink) -> Result<Self, ShortcutError> {
        let (commands, receiver) = unbounded();
        let (ready, result) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new().name("rune-shortcuts-portal".into()).spawn(move || {
            async_io::block_on(async move {
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
                    Ok(v) => { ready.send(Ok(())).ok(); v },
                    Err(e) => { ready.send(Err(e)).ok(); return; }
                };
                let mut sessions: HashMap<ShortcutId, OwnedObjectPath> = HashMap::new();
                enum Wake {
                    Command(Option<Command>),
                    Event(Option<zbus::Message>),
                }
                loop {
                    let next = async { Wake::Command(receiver.recv().await.ok()) }
                        .or(async { Wake::Event(events.next().await) })
                        .await;
                    match next {
                        Wake::Command(Some(Command::Register(id, shortcut, reply))) => {
                            let result = bind(&conn, &portal, id, &shortcut).await.map(|session| { sessions.insert(id, session); });
                            reply.send(result).ok();
                        }
                        Wake::Command(Some(Command::Unregister(id, reply))) => {
                            let result = match sessions.get(&id) {
                                Some(session) => close(&conn, session).await,
                                None => Err(ShortcutError::UnknownId),
                            };
                            if result.is_ok() { sessions.remove(&id); }
                            reply.send(result).ok();
                        }
                        Wake::Command(None) | Wake::Event(None) => break,
                        Wake::Event(Some(event)) => {
                            if let Ok((session, key, _, options)) = event.body().deserialize::<(OwnedObjectPath, String, u64, HashMap<String, OwnedValue>)>()
                                && let Ok(id) = key.parse::<u32>().map(ShortcutId)
                                && sessions.get(&id) == Some(&session) {
                                let token = options.get("activation_token").and_then(|v| <&str>::try_from(v).ok()).map(str::to_owned);
                                sink(id, token);
                            }
                        }
                    }
                }
                for session in sessions.values() { close(&conn, session).await.ok(); }
                conn.close().await.ok();
            });
        }).map_err(|e| ShortcutError::Backend(e.to_string()))?;
        let mut native = Self { commands: Some(commands), worker: Some(worker) };
        if let Err(e) = result.recv().unwrap_or(Err(ShortcutError::Unsupported)) {
            native.commands.take();
            return Err(e);
        }
        Ok(native)
    }
    fn send(&self, command: impl FnOnce(Reply) -> Command) -> Result<(), ShortcutError> {
        let (reply, result) = mpsc::sync_channel(1);
        self.commands.as_ref().ok_or(ShortcutError::NotRunning)?.try_send(command(reply)).map_err(|_| ShortcutError::Backend("portal disconnected".into()))?;
        result.recv().unwrap_or_else(|_| Err(ShortcutError::Backend("portal disconnected".into())))
    }
}
impl Backend for Native {
    fn register(&mut self, id: ShortcutId, shortcut: &Shortcut) -> Result<(), ShortcutError> { self.send(|reply| Command::Register(id, shortcut.clone(), reply)) }
    fn unregister(&mut self, id: ShortcutId) -> Result<(), ShortcutError> { self.send(|reply| Command::Unregister(id, reply)) }
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
