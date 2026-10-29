//! Single instance per app id on Linux: the first process owns a bus name on the session
//! bus; a second launch forwards its URLs/files to the owner and exits. The owner surfaces
//! them through `App::on_open` (or `on_reopen` when there is nothing to open), like a macOS
//! reopen or file-open.
//!
//! Opt in with `App::single_instance(app_id)` (reverse-DNS, e.g. `com.example.App`):
//! without it nothing claims a bus name and every launch runs. Headless runs
//! (`--snap`, `--tree`, scripts) never claim, so goldens and CI stay hermetic.

use std::sync::{Arc, Mutex};

/// A file/URL the instance owner should open, forwarded from a second launch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenRequest {
    /// `Open` URIs, in order. Empty for a bare `Activate` (a Dock-click reopen).
    pub uris: Vec<String>,
    /// `XDG_ACTIVATION_TOKEN`/`DESKTOP_STARTUP_ID` the second launch carried, if any.
    pub activation_token: Option<String>,
}

pub(crate) type Queue = Arc<Mutex<Vec<OpenRequest>>>;

/// The bus name `app_id` is served as.
pub(crate) fn bus_name(app_id: &str) -> String { format!("rune.{}", app_id.replace(['.', '-'], "_")) }

#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) enum Claim {
    /// This process owns the name; drain the queue on the event thread.
    Owner(Queue),
    /// A second launch; the owner has the URIs.
    Forwarded,
}

/// Claim `app_id` on a session-bus connection, or forward `uris` to its owner.
/// `Err` means no session bus (or the claim failed): run unclaimed, as without
/// `single_instance`, so a bus-less box still launches.
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn claim(app_id: &str, uris: Vec<String>) -> Result<Claim, String> {
    use std::collections::HashMap;
    use zbus::OwnedVariant as OwnedValue;

    struct Application {
        queue: Queue,
        wake: Mutex<Option<crate::runtime::Waker>>,
    }

    #[zbus::interface(name = "org.freedesktop.Application")]
    impl Application {
        /// Bare activation (a Dock click): a reopen with nothing to open.
        fn activate(&self, _platform_data: HashMap<String, OwnedValue>) {
            self.queue.lock().unwrap().push(OpenRequest::default());
            if let Some(wake) = self.wake.lock().unwrap().as_ref() { wake(); }
        }

        /// Open URIs in the running instance.
        fn open(&self, uris: Vec<String>, _platform_data: HashMap<String, OwnedValue>) {
            self.queue.lock().unwrap().push(OpenRequest { uris, activation_token: None });
            if let Some(wake) = self.wake.lock().unwrap().as_ref() { wake(); }
        }

        fn activate_action(
            &self,
            _action: String,
            _parameter: OwnedValue,
            _platform_data: HashMap<String, OwnedValue>,
        ) {
        }
    }

    const PATH: &str = "/rune/Application";
    const INTERFACE: &str = "org.freedesktop.Application";
    let name = bus_name(app_id);
    let task = async move {
        let conn = zbus::Connection::session().await.map_err(|e| e.to_string())?;
        // Owned already? Ask the bus (no method call, so no stray reopen gets queued),
        // then forward through the owner's proxy and exit.
        let has_owner: bool = conn
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "NameHasOwner",
                &name,
            )
            .await
            .map_err(|e| e.to_string())?
            .body()
            .deserialize::<bool>()
            .map_err(|e| e.to_string())?;
        if has_owner {
            let probe = zbus::Proxy::new(&conn, name.as_str(), PATH, INTERFACE)
                .await
                .map_err(|e| e.to_string())?;
            if uris.is_empty() {
                let _: () = probe
                    .call("Activate", &HashMap::<String, OwnedValue>::new())
                    .await
                    .map_err(|e| e.to_string())?;
            } else {
                let _: () = probe
                    .call("Open", &(uris, HashMap::<String, OwnedValue>::new()))
                    .await
                    .map_err(|e| e.to_string())?;
            }
            return Ok(Claim::Forwarded);
        }
        // Nobody home: serve on a dedicated bus thread that outlives this call, and hold
        // the name on its connection for the app's life. (A leaked connection alone is not
        // enough: with no executor left, the owner's method calls would never dispatch.)
        let queue: Queue = Arc::new(Mutex::new(Vec::new()));
        let serve = {
            let queue = queue.clone();
            let name = name.clone();
            std::thread::Builder::new().name("rune-instance-bus".into()).spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())?;
                let serve = async move {
                    let conn = zbus::Connection::session().await.map_err(|e| e.to_string())?;
                    let app = Application { queue, wake: Mutex::new(None) };
                    conn.object_server().at(PATH, app).await.map_err(|e| e.to_string())?;
                    conn.request_name_with_flags(
                        name.as_str(),
                        zbus::fdo::RequestNameFlags::REPLACE_EXISTING.into(),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                    // Serve until the process ends; dropping the connection releases the name.
                    std::future::pending::<()>().await;
                    #[allow(unreachable_code)]
                    Ok::<(), String>(())
                };
                runtime.block_on(serve)
            })
        };
        let serve = serve.map_err(|e| e.to_string())?;
        // Wait until the server owns the name (or fails) instead of racing the second launch.
        let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let ready = ready.clone();
            let name = name.clone();
            std::thread::Builder::new().name("rune-instance-ready".into()).spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(r) => r,
                    Err(_) => return,
                };
                let owned = runtime.block_on(async move {
                    // Poll NameHasOwner briefly; the server thread claims within milliseconds.
                    for _ in 0..100 {
                        let has: bool = match zbus::Connection::session().await {
                            Ok(conn) => {
                                let reply = conn
                                    .call_method(
                                        Some("org.freedesktop.DBus"),
                                        "/org/freedesktop/DBus",
                                        Some("org.freedesktop.DBus"),
                                        "NameHasOwner",
                                        &name,
                                    )
                                    .await;
                                match reply {
                                    Ok(msg) => match msg.body().deserialize::<bool>() {
                                        Ok(has) => has,
                                        Err(_) => return false,
                                    },
                                    Err(_) => return false,
                                }
                            }
                            Err(_) => return false,
                        };
                        if has {
                            return true;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                    false
                });
                ready.store(owned, std::sync::atomic::Ordering::Release);
            }).map_err(|e| e.to_string())?;
        }
        // Park briefly for the ready probe; a slow bus still resolves on the next launch.
        for _ in 0..100 {
            if ready.load(std::sync::atomic::Ordering::Acquire) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // If the server thread died, surface it instead of claiming a nameless queue.
        if serve.is_finished() {
            match serve.join() {
                Ok(Err(e)) => return Err(e),
                Ok(Ok(_)) => return Err("instance server stopped".into()),
                Err(_) => return Err("instance server stopped".into()),
            }
        } else {
            std::mem::forget(serve);
        }
        Ok(Claim::Owner(queue))
    };
    match std::thread::Builder::new().name("rune-instance".into()).spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        runtime.block_on(task)
    }) {
        Ok(join) => join.join().map_err(|_| "instance thread stopped".to_owned())?,
        Err(e) => Err(e.to_string()),
    }
}

/// Files/URLs this launch should open: the app's own extra args that name existing
/// paths, plus `file://` URIs (as paths). Bare `http(s)://` and other URIs pass through
/// as URIs; argv flags are ignored.
pub(crate) fn launch_uris(_args: &crate::tooling::Args) -> Vec<String> {
    crate::tooling::extra_args()
        .iter()
        .filter_map(|arg| {
            if arg.starts_with("file://") {
                Some(arg["file://".len()..].to_owned())
            } else if arg.starts_with('-') {
                None
            } else if std::path::Path::new(arg).exists() {
                Some(arg.clone())
            } else if arg.contains("://") {
                Some(arg.clone())
            } else {
                None
            }
        })
        .collect()
}

#[cfg(all(unix, not(target_os = "macos")))]
#[cfg(test)]
mod live_tests {
    use super::*;

    fn bus_available() -> bool {
        std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
            || std::path::Path::new("/run/dbus/system_bus_socket").exists()
    }

    #[test]
    fn second_claim_forwards_uris_and_exits_without_owning() {
        if !bus_available() {
            eprintln!("skipping: no session bus");
            return;
        }
        let id = format!("rune-test-{}-{}", std::process::id(), std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0));
        let queue = match claim(&id, Vec::new()) {
            Ok(Claim::Owner(queue)) => queue,
            Ok(Claim::Forwarded) => panic!("first claim must own the name"),
            Err(e) => { eprintln!("skipping: {e}"); return; }
        };
        // A second launch with files forwards them; it must not own the name.
        match claim(&id, vec!["file:///tmp/a.txt".into(), "https://example.com".into()]) {
            Ok(Claim::Forwarded) => {}
            Ok(Claim::Owner(_)) => panic!("second claim must forward, not own"),
            Err(e) => { eprintln!("skipping: {e}"); return; }
        }
        // The owner drains both URIs.
        let open = std::mem::take(&mut *queue.lock().unwrap());
        assert_eq!(open.len(), 1, "one Open call, both URIs in it");
        assert_eq!(open[0].uris, vec!["file:///tmp/a.txt".to_owned(), "https://example.com".to_owned()]);
    }

    #[test]
    fn bare_second_claim_forwards_an_empty_activate() {
        if !bus_available() {
            eprintln!("skipping: no session bus");
            return;
        }
        let id = format!("rune-activate-{}-{}", std::process::id(), std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0));
        let queue = match claim(&id, Vec::new()) {
            Ok(Claim::Owner(queue)) => queue,
            Ok(Claim::Forwarded) => panic!("first claim must own the name"),
            Err(e) => { eprintln!("skipping: {e}"); return; }
        };
        match claim(&id, Vec::new()) {
            Ok(Claim::Forwarded) => {}
            Ok(Claim::Owner(_)) => panic!("second claim must forward, not own"),
            Err(e) => { eprintln!("skipping: {e}"); return; }
        }
        let open = std::mem::take(&mut *queue.lock().unwrap());
        assert_eq!(open, vec![OpenRequest::default()], "bare Activate is a reopen with nothing to open");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bus_names_replace_dots_and_dashes() {
        assert_eq!(bus_name("com.example.My-App"), "rune.com_example_My_App");
    }
}
