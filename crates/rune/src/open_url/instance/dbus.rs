//! `org.freedesktop.Application` on the session bus, for the first Linux process of an app
//! (`single_instance`). A desktop entry with `DBusActivatable=true`, `gio launch` and `gio open`
//! call it on the app's id instead of starting a process.
//!
//! The process owns the id as a bus name (without queueing: a process that already has it keeps
//! it) and serves the interface at `/` plus the id with its dots as slashes, on its own thread and
//! connection. Each call is a launch handed to this process, as one down the socket is
//! (`handoff::deliver`): `Activate` and `ActivateAction` reopen the app, `Open` sends links to
//! `App::on_open_url` and `file://` URIs to `App::on_open_files`. The platform data's
//! `activation-token` (or `desktop-startup-id`) is kept for the window it brings forward.
//!
//! A call that arrives before the loop has started is the one that started this process (D-Bus
//! activation): the launch opens its windows anyway, so an `Activate` only leaves its token for
//! the first window, and an `Open`'s links and files wait for it as launch links do. Once the
//! loop has ended, a call starts the app again, as a socket launch does (`deliver`).
//!
//! The name is requested without queueing: a process that finds it still held (an earlier
//! process of the app that hasn't exited or called `release_instance`, or a hung one whose
//! socket this process took over) doesn't serve the interface; launches still reach it through
//! the socket.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::zvariant::OwnedValue;

use super::handoff::{Launch, deliver, keep_token, loop_started};

/// How long the bus connection and the name may take before the process goes on without them.
const SETUP_WITHIN: Duration = Duration::from_secs(5);
/// How long releasing the name may take; `release` waits for that and the close.
const RELEASE_WITHIN: Duration = Duration::from_secs(1);

/// What stops the serving thread, and what it says once it has let go of the name.
static SERVING: Mutex<Option<(async_channel::Sender<()>, std::sync::mpsc::Receiver<()>)>> = Mutex::new(None);

/// Owns `id` on the session bus and serves `org.freedesktop.Application`, on a thread of its own;
/// nothing when `id` isn't a valid bus name or the bus can't be reached.
pub(super) fn serve(id: &str) {
    let Some(path) = object_path(id) else { return };
    let name = id.to_owned();
    let (stop, stopped) = async_channel::bounded::<()>(1);
    let (done, finished) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("rune-instance-dbus".into()).spawn(move || {
        async_io::block_on(async move {
            let setup = async {
                let conn = zbus::connection::Builder::session()?.serve_at(path.as_str(), Application)?.build().await?;
                let reply = conn.request_name_with_flags(name.as_str(), RequestNameFlags::DoNotQueue.into()).await?;
                Ok::<_, zbus::Error>((conn, reply))
            };
            match crate::platform::portal::timeout(SETUP_WITHIN, setup).await {
                Some(Ok((conn, RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner))) => {
                    stopped.recv().await.ok();
                    // Released before the close returns, so a next process (`Tx::relaunch`)
                    // that asks once `release_instance` has returned finds the name free.
                    crate::platform::portal::timeout(RELEASE_WITHIN, conn.release_name(name.as_str())).await;
                    conn.close().await.ok();
                }
                Some(Ok((conn, _))) => {
                    conn.close().await.ok();
                }
                Some(Err(_)) | None => {}
            }
        });
        done.send(()).ok();
    });
    if spawned.is_ok() {
        *SERVING.lock().unwrap_or_else(|e| e.into_inner()) = Some((stop, finished));
    }
}

/// Lets go of the bus name: returns once it has, or after twice `RELEASE_WITHIN`.
pub(super) fn release() {
    let Some((stop, finished)) = SERVING.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
    stop.close();
    finished.recv_timeout(RELEASE_WITHIN * 2).ok();
}

struct Application;

#[zbus::interface(name = "org.freedesktop.Application")]
impl Application {
    fn activate(&self, platform_data: HashMap<String, OwnedValue>) {
        reopen(token(&platform_data));
    }

    fn open(&self, uris: Vec<String>, platform_data: HashMap<String, OwnedValue>) {
        // As a launch's arguments: local `file://` URIs are files, and links only those on the
        // app's own schemes once it named them (`url_schemes`).
        let (links, files) = super::sort_arguments(&uris);
        let token = token(&platform_data);
        if links.is_empty() && files.is_empty() {
            reopen(token);
            return;
        }
        if !loop_started() {
            keep_token(token);
            for link in links { super::super::incoming::receive(link); }
            if !files.is_empty() { super::super::incoming::receive_files(files); }
            return;
        }
        deliver(Launch { args: restart_arguments(&uris), links, files, token });
    }

    /// Rune has no named app actions: an action reopens the app, as a jump list entry's launch
    /// without a link does on Windows.
    fn activate_action(&self, _action_name: String, _parameter: Vec<OwnedValue>, platform_data: HashMap<String, OwnedValue>) {
        reopen(token(&platform_data));
    }
}

/// A reopen; before the loop has started, only the token, for the window the launch opens.
fn reopen(token: Option<String>) {
    if !loop_started() {
        keep_token(token);
        return;
    }
    deliver(Launch { args: Vec::new(), links: Vec::new(), files: Vec::new(), token });
}

/// The activation token in a call's platform data.
fn token(platform_data: &HashMap<String, OwnedValue>) -> Option<String> {
    ["activation-token", "desktop-startup-id"]
        .into_iter()
        .find_map(|key| platform_data.get(key).and_then(|v| <&str>::try_from(v).ok()).filter(|t| !t.is_empty()).map(str::to_owned))
}

/// What starts the app again with an `Open` (while it quits, or once its loop has ended): the
/// URIs that sorted as links or files, as received, so the new process sorts them the same way.
fn restart_arguments(uris: &[String]) -> Vec<String> {
    uris.iter().filter(|uri| { let (links, files) = super::sort_arguments(std::slice::from_ref(uri)); !links.is_empty() || !files.is_empty() }).cloned().collect()
}

/// The object path the interface is served at: `/` plus `id` with its dots as slashes and its
/// dashes as underscores. None when `id` isn't a well-known bus name of at least two elements
/// that also makes a valid object path.
pub(super) fn object_path(id: &str) -> Option<String> {
    if id.is_empty() || id.len() > 255 || id.starts_with('.') {
        return None;
    }
    let elements: Vec<&str> = id.split('.').collect();
    let valid = elements.len() >= 2
        && elements.iter().all(|e| {
            !e.is_empty()
                && !e.starts_with(|c: char| c.is_ascii_digit())
                && e.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        });
    valid.then(|| format!("/{}", id.replace('.', "/").replace('-', "_")))
}

#[cfg(test)]
#[path = "dbus_tests.rs"]
mod tests;
