//! Linux notifications over D-Bus on a dedicated connection and worker thread:
//! - Inside a Flatpak sandbox (`FLATPAK_ID` or `/.flatpak-info`), uses the XDG Notification
//!   portal (`org.freedesktop.portal.Notification`), gating v2-only keys such as `sound` on
//!   `version >= 2`, and falls back to `org.freedesktop.Notifications` if the portal is
//!   unavailable.
//! - Outside a sandbox, talks directly to `org.freedesktop.Notifications`.
//!
//! The first call connects and probes the active backend; `available` is whether one answered.

use super::linux_format::{
    Caps, FdoSound, PortalShown, actions_for, body_for, click_for, fdo_sound, in_flatpak,
    portal_action_unwire, portal_payload,
};
use super::{Notification, deliver};
use async_channel::{Receiver, Sender, unbounded};
use futures_lite::{FutureExt, StreamExt};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::Duration;
use zbus::proxy::SignalStream;
use zbus::zvariant::{OwnedValue, Value};
use zbus::{Connection, MatchRule, MessageStream, Proxy, message::Type};

const FDO_NAME: &str = "org.freedesktop.Notifications";
const FDO_PATH: &str = "/org/freedesktop/Notifications";
const PORTAL_NAME: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const PORTAL_INTERFACE: &str = "org.freedesktop.portal.Notification";
/// How long the first call waits for the session bus and a notification backend (which the bus
/// may start on demand) before it decides there is none.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const RPC_TIMEOUT: Duration = Duration::from_secs(5);

/// Who posts, from `register`: the FDO server shows the name and finds the app's desktop entry
/// (`<id>.desktop`) for its icon.
static IDENTITY: Mutex<Option<(String, String)>> = Mutex::new(None);

pub(crate) fn register(id: &str, name: &str) {
    *IDENTITY.lock().unwrap_or_else(|e| e.into_inner()) = Some((id.to_owned(), name.to_owned()));
}

struct Post {
    n: Notification,
    app_name: String,
    desktop_entry: Option<String>,
}

enum Command {
    Post(Post),
    Remove(Vec<String>),
}

fn backend() -> Option<&'static Sender<Command>> {
    static BACKEND: OnceLock<Option<Sender<Command>>> = OnceLock::new();
    BACKEND.get_or_init(start).as_ref()
}

/// Starts the worker and waits for it to reach a backend, or not.
fn start() -> Option<Sender<Command>> {
    let (commands, receiver) = unbounded();
    let (ready, result) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("rune-notify".into())
        .spawn(move || async_io::block_on(run(receiver, ready)))
        .ok()?;
    match result.recv_timeout(PROBE_TIMEOUT + Duration::from_millis(500)) {
        Ok(true) => Some(commands),
        _ => {
            commands.close();
            None
        }
    }
}

/// The worker drops its end when the connection is gone, which closes the channel.
pub(crate) fn available() -> bool {
    backend().is_some_and(|commands| !commands.is_closed())
}

pub(crate) fn post(n: &Notification) -> bool {
    let Some(commands) = backend() else { return false };
    let identity = IDENTITY.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let (app_name, desktop_entry) = match identity {
        Some((id, name)) => (name, Some(id)),
        None => (program_name(), None),
    };
    commands.try_send(Command::Post(Post { n: n.clone(), app_name, desktop_entry })).is_ok()
}

pub(crate) fn remove(ids: &[&str]) {
    if let Some(commands) = backend() {
        commands.try_send(Command::Remove(ids.iter().map(|id| (*id).to_owned()).collect())).ok();
    }
}

/// The executable's file name, for a process that never called `register`.
fn program_name() -> String {
    std::env::args_os()
        .next()
        .and_then(|arg| std::path::Path::new(&arg).file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

async fn timeout<T>(limit: Duration, future: impl Future<Output = T>) -> Option<T> {
    async { Some(future.await) }
        .or(async {
            async_io::Timer::after(limit).await;
            None
        })
        .await
}

/// One posted FDO notification the server still shows, with the activation token it sent for the
/// click about to come.
struct Shown {
    server: u32,
    info: Vec<(String, String)>,
    token: Option<String>,
}

enum Connected {
    Portal { proxy: Proxy<'static>, signals: Box<SignalStream<'static>>, version: u32 },
    Fdo { proxy: Proxy<'static>, signals: Box<MessageStream>, caps: Caps },
}

async fn connect_portal(conn: &Connection) -> zbus::Result<Connected> {
    let proxy = Proxy::new(conn, PORTAL_NAME, PORTAL_PATH, PORTAL_INTERFACE).await?;
    let version: u32 = proxy.get_property("version").await?;
    if version == 0 {
        return Err(zbus::Error::Failure("notification portal reported version 0".into()));
    }
    let signals = Box::new(proxy.receive_signal("ActionInvoked").await?);
    Ok(Connected::Portal { proxy, signals, version })
}

async fn connect_fdo(conn: &Connection) -> zbus::Result<Connected> {
    let proxy = Proxy::new(conn, FDO_NAME, FDO_PATH, FDO_NAME).await?;
    // One stream for every signal, so `ActivationToken` stays ahead of its `ActionInvoked`;
    // only the server's, so no other peer on the bus can fake a click.
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(FDO_NAME)?
        .interface(FDO_NAME)?
        .path(FDO_PATH)?
        .build();
    let signals = Box::new(MessageStream::for_match_rule(rule, conn, None).await?);
    let caps: Vec<String> = proxy.call("GetCapabilities", &()).await?;
    Ok(Connected::Fdo { proxy, signals, caps: Caps::from_list(&caps) })
}

async fn run(commands: Receiver<Command>, ready: mpsc::SyncSender<bool>) {
    let sandboxed = in_flatpak(
        std::env::var_os("FLATPAK_ID").as_deref(),
        std::path::Path::new("/.flatpak-info").exists(),
    );
    let setup = async {
        let conn = Connection::session().await?;
        let connected = if sandboxed {
            match connect_portal(&conn).await {
                Ok(portal) => portal,
                Err(_) => connect_fdo(&conn).await?,
            }
        } else {
            connect_fdo(&conn).await?
        };
        Ok::<_, zbus::Error>((conn, connected))
    };
    let Some(Ok((_conn, connected))) = timeout(PROBE_TIMEOUT, setup).await else {
        ready.send(false).ok();
        return;
    };
    ready.send(true).ok();
    match connected {
        Connected::Portal { proxy, signals, version } => run_portal(commands, proxy, *signals, version).await,
        Connected::Fdo { proxy, signals, caps } => run_fdo(commands, proxy, *signals, caps).await,
    }
}

async fn run_portal(
    commands: Receiver<Command>,
    proxy: Proxy<'static>,
    mut signals: SignalStream<'static>,
    version: u32,
) {
    enum Event {
        Command(Option<Command>),
        Signal(Option<zbus::Message>),
    }

    let mut shown = PortalShown::new();
    loop {
        let event = async { Event::Command(commands.recv().await.ok()) }
            .or(async { Event::Signal(signals.next().await) })
            .await;
        match event {
            Event::Command(None) | Event::Signal(None) => return,
            Event::Command(Some(Command::Post(post))) => {
                let notification = portal_notification(&post.n, version);
                let args = (post.n.id.as_str(), notification);
                if let Some(Ok(())) = timeout(RPC_TIMEOUT, proxy.call::<_, _, ()>("AddNotification", &args)).await {
                    shown.record(post.n.id, post.n.info);
                }
            }
            Event::Command(Some(Command::Remove(ids))) => {
                for id in ids {
                    shown.remove(&id);
                    let _ = timeout(RPC_TIMEOUT, proxy.call::<_, _, ()>("RemoveNotification", &(id.as_str(),))).await;
                }
            }
            Event::Signal(Some(message)) => {
                let Ok((id, action, params)) =
                    message.body().deserialize::<(String, String, Vec<OwnedValue>)>()
                else {
                    continue;
                };
                let key = portal_action_unwire(&action);
                let token = portal_activation_token(&params);
                deliver(click_for(&id, shown.info(&id), key, token));
            }
        }
    }
}

async fn run_fdo(
    commands: Receiver<Command>,
    proxy: Proxy<'static>,
    mut signals: MessageStream,
    caps: Caps,
) {
    enum Event {
        Command(Option<Command>),
        /// One of the server's signals, or the stream's error for a message it couldn't read; None
        /// when the connection ends.
        Signal(Option<zbus::Result<zbus::Message>>),
    }

    let mut shown: HashMap<String, Shown> = HashMap::new();
    loop {
        let event = async { Event::Command(commands.recv().await.ok()) }
            .or(async { Event::Signal(signals.next().await) })
            .await;
        match event {
            Event::Command(None) => return,
            Event::Command(Some(Command::Post(post))) => {
                let replaces = shown.get(&post.n.id).map_or(0, |s| s.server);
                let actions = actions_for(&post.n, caps);
                let body = body_for(&post.n.body, caps);
                let hints = fdo_hints(&post.n, post.desktop_entry.as_deref());
                let args = (
                    post.app_name.as_str(),
                    replaces,
                    "",
                    post.n.title.as_str(),
                    body.as_str(),
                    actions,
                    hints,
                    -1i32,
                );
                if let Some(Ok(server)) = timeout(RPC_TIMEOUT, proxy.call::<_, _, u32>("Notify", &args)).await {
                    shown.insert(post.n.id, Shown { server, info: post.n.info, token: None });
                }
            }
            Event::Command(Some(Command::Remove(ids))) => {
                for id in ids {
                    if let Some(s) = shown.remove(&id) {
                        let _ = timeout(RPC_TIMEOUT, proxy.call::<_, _, ()>("CloseNotification", &(s.server,))).await;
                    }
                }
            }
            // A message the stream couldn't read; the connection carries on.
            Event::Signal(Some(Err(_))) => {}
            Event::Signal(Some(Ok(message))) => {
                let header = message.header();
                let body = message.body();
                match header.member().map(|m| m.as_str()) {
                    // It comes just before the `ActionInvoked` it belongs to; the server sends
                    // other apps' too, which match none of ours.
                    Some("ActivationToken") => {
                        let Ok((server, token)) = body.deserialize::<(u32, String)>() else { continue };
                        if let Some(s) = shown.values_mut().find(|s| s.server == server) {
                            s.token = Some(token);
                        }
                    }
                    Some("ActionInvoked") => {
                        let Ok((server, key)) = body.deserialize::<(u32, String)>() else { continue };
                        if let Some((id, s)) = shown.iter_mut().find(|(_, s)| s.server == server) {
                            deliver(click_for(id, &s.info, &key, s.token.take()));
                        }
                    }
                    Some("NotificationClosed") => {
                        if let Ok((server, _reason)) = body.deserialize::<(u32, u32)>() {
                            shown.retain(|_, s| s.server != server);
                        }
                    }
                    _ => {}
                }
            }
            // The stream ends only with the connection; returning drops `commands`, so
            // `available` turns false.
            Event::Signal(None) => return,
        }
    }
}

/// Builds the FDO `Notify` hints dictionary for `n`.
pub(crate) fn fdo_hints<'a>(n: &Notification, desktop_entry: Option<&'a str>) -> HashMap<&'static str, Value<'a>> {
    let mut hints: HashMap<&'static str, Value<'a>> = HashMap::new();
    match fdo_sound(n.sound) {
        FdoSound::Suppress => {
            hints.insert("suppress-sound", Value::from(true));
        }
        FdoSound::Name(name) => {
            hints.insert("sound-name", Value::from(name));
        }
    }
    if let Some(entry) = desktop_entry {
        hints.insert("desktop-entry", Value::from(entry));
    }
    hints
}

/// Builds the `org.freedesktop.portal.Notification.AddNotification` vardict for `n` at
/// `version`, omitting v2-only keys when `version < 2`.
pub(crate) fn portal_notification<'a>(n: &'a Notification, version: u32) -> HashMap<&'static str, Value<'a>> {
    let payload = portal_payload(n, version);
    let mut map: HashMap<&'static str, Value<'a>> = HashMap::new();
    map.insert("title", Value::from(payload.title));
    if let Some(body) = payload.body {
        map.insert("body", Value::from(body));
    }
    map.insert("default-action", Value::from(payload.default_action));
    if !payload.buttons.is_empty() {
        let buttons: Vec<HashMap<&'static str, Value<'a>>> = payload
            .buttons
            .into_iter()
            .map(|b| {
                let mut button: HashMap<&'static str, Value<'a>> = HashMap::new();
                button.insert("label", Value::from(b.label));
                let action = match b.action {
                    Cow::Borrowed(s) => Value::from(s),
                    Cow::Owned(s) => Value::from(s),
                };
                button.insert("action", action);
                button
            })
            .collect();
        map.insert("buttons", Value::from(buttons));
    }
    if let Some(sound) = payload.sound {
        map.insert("sound", Value::from(sound));
    }
    map
}

/// Extracts the XDG activation token from `org.freedesktop.portal.Notification::ActionInvoked`'s
/// `parameter` (`av`) array, where portal v2 places `platform-data` (`a{sv}`) containing
/// `activation-token` (`s`).
pub(crate) fn portal_activation_token(params: &[OwnedValue]) -> Option<String> {
    for param in params {
        let Ok(cloned) = param.try_clone() else { continue };
        let Ok(map) = HashMap::<String, OwnedValue>::try_from(cloned) else { continue };
        if let Some(token) = map.get("activation-token").and_then(|v| <&str>::try_from(v).ok())
            && !token.is_empty()
        {
            return Some(token.to_owned());
        }
    }
    None
}
