//! Shared XDG desktop portal helpers for Linux D-Bus clients (`portal_appearance`,
//! `shortcuts::wayland`, `file_dialog::portal`): monotonic request tokens, Request object path
//! construction, portal availability classification, subscribe-before-call Request/Response
//! round-trips, and a shared background worker runtime for one-shot portal calls.

use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const PATH: &str = "/org/freedesktop/portal/desktop";

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

/// Returns a process-unique XDG portal `handle_token` / `session_handle_token` so repeated or
/// concurrent portal requests never collide on `/org/freedesktop/portal/desktop/request/...`.
pub(crate) fn next_token(prefix: &str) -> String {
    let n = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    format!("rune_{prefix}_{n}")
}

/// Builds the `org.freedesktop.portal.Request` object path from the D-Bus connection's unique
/// name (`:1.42`) and a request `token`. Returns `None` if either component is empty or contains
/// characters outside `[A-Za-z0-9_]` after sanitizing the sender name.
pub(crate) fn request_path(unique_name: &str, token: &str) -> Option<String> {
    let sender = unique_name.trim_start_matches(':');
    if sender.is_empty()
        || token.is_empty()
        || !token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return None;
    }
    let sender = sender.replace('.', "_");
    if !sender.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    Some(format!("{PATH}/request/{sender}/{token}"))
}

/// Whether the Settings portal watcher should keep its thread alive listening for
/// `SettingChanged`: keep watching when the initial read succeeded or failed for a reason other
/// than the portal being absent; exit cleanly when the portal service itself is unavailable.
pub(crate) fn should_watch_settings(read_ok: bool, unavailable: bool) -> bool {
    read_ok || !unavailable
}

#[cfg(all(target_os = "linux", feature = "dbus"))]
mod dbus {
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::{Pin, pin};
    use std::sync::OnceLock;
    use std::time::Duration;

    use async_executor::LocalExecutor;
    use async_io::Timer;
    use futures_lite::{FutureExt, StreamExt};
    use zbus::{
        Connection, Proxy, fdo,
        zvariant::{OwnedObjectPath, OwnedValue, Type},
    };

    use super::request_path;

    pub(crate) const DEST: &str = "org.freedesktop.portal.Desktop";

    /// Timeouts for an XDG portal `Request`/`Response` round-trip.
    pub(crate) struct RequestTimeouts {
        pub(crate) rpc: Duration,
        pub(crate) response: Option<Duration>,
    }

    /// Failure from [`request`].
    pub(crate) enum RequestError {
        Dbus(zbus::Error),
        Unavailable,
        TimedOut,
        Disconnected,
        Cancelled,
    }

    /// Awaits `future`, returning `None` if `limit` elapses first.
    pub(crate) async fn timeout<T>(limit: Duration, future: impl Future<Output = T>) -> Option<T> {
        async { Some(future.await) }
            .or(async {
                Timer::after(limit).await;
                None
            })
            .await
    }

    async fn or_cancel<T, C: Future<Output = ()>>(
        cancel: Pin<&mut C>,
        future: impl Future<Output = Result<T, RequestError>>,
    ) -> Result<T, RequestError> {
        async {
            cancel.await;
            Err(RequestError::Cancelled)
        }
        .or(future)
        .await
    }

    /// Whether `e` indicates that the XDG desktop portal service or interface is absent or
    /// failed D-Bus activation on this desktop.
    pub(crate) fn is_unavailable(e: &zbus::Error) -> bool {
        matches!(
            e,
            zbus::Error::FDO(fdo)
                if matches!(
                    **fdo,
                    fdo::Error::ServiceUnknown(_)
                        | fdo::Error::NameHasNoOwner(_)
                        | fdo::Error::UnknownInterface(_)
                        | fdo::Error::UnknownMethod(_)
                        | fdo::Error::UnknownObject(_)
                        | fdo::Error::SpawnExecFailed(_)
                        | fdo::Error::SpawnForkFailed(_)
                        | fdo::Error::SpawnChildExited(_)
                        | fdo::Error::SpawnChildSignaled(_)
                        | fdo::Error::SpawnFailed(_)
                        | fdo::Error::SpawnServiceNotFound(_)
                )
        )
    }

    /// Runs a standard XDG portal `Request`/`Response` round-trip: subscribes to the predicted
    /// `org.freedesktop.portal.Request::Response` signal before invoking `method`, falls back to
    /// the returned path on pre-`handle_token` portals, honours `cancel` at every step, and calls
    /// `Request::Close` on cancellation or timeout.
    pub(crate) async fn request<B, C>(
        conn: &Connection,
        portal: &Proxy<'_>,
        token: &str,
        method: &str,
        body: &B,
        timeouts: RequestTimeouts,
        cancel: C,
    ) -> Result<(u32, HashMap<String, OwnedValue>), RequestError>
    where
        B: serde::Serialize + Type,
        C: Future<Output = ()>,
    {
        let mut cancel = pin!(cancel);
        let unique = conn.unique_name().ok_or(RequestError::Unavailable)?;
        let path = request_path(unique.as_str(), token).ok_or(RequestError::Unavailable)?;
        let (request, mut responses) = timeout(timeouts.rpc, async {
            let mut request = Proxy::new(conn, DEST, path.clone(), "org.freedesktop.portal.Request")
                .await
                .map_err(RequestError::Dbus)?;
            // Subscribe before calling: a portal can emit `Response` before the method returns.
            let mut responses = or_cancel(cancel.as_mut(), async {
                request.receive_signal("Response").await.map_err(RequestError::Dbus)
            })
            .await?;
            let handle: OwnedObjectPath = match or_cancel(cancel.as_mut(), async {
                portal.call::<_, _, OwnedObjectPath>(method, body).await.map_err(RequestError::Dbus)
            })
            .await
            {
                Ok(handle) => handle,
                Err(RequestError::Cancelled) => {
                    let _ = timeout(timeouts.rpc, request.call::<_, _, ()>("Close", &())).await;
                    return Err(RequestError::Cancelled);
                }
                Err(e) => return Err(e),
            };
            if handle.as_str() != path {
                // Portals older than `handle_token` support pick their own request object path.
                request = Proxy::new(conn, DEST, handle.as_str().to_owned(), "org.freedesktop.portal.Request")
                    .await
                    .map_err(RequestError::Dbus)?;
                responses = match or_cancel(cancel.as_mut(), async {
                    request.receive_signal("Response").await.map_err(RequestError::Dbus)
                })
                .await
                {
                    Ok(responses) => responses,
                    Err(RequestError::Cancelled) => {
                        let _ = timeout(timeouts.rpc, request.call::<_, _, ()>("Close", &())).await;
                        return Err(RequestError::Cancelled);
                    }
                    Err(e) => return Err(e),
                };
            }
            Ok::<_, RequestError>((request, responses))
        })
        .await
        .unwrap_or(Err(RequestError::TimedOut))?;

        let wait = or_cancel(cancel.as_mut(), async {
            match responses.next().await {
                Some(message) => message
                    .body()
                    .deserialize::<(u32, HashMap<String, OwnedValue>)>()
                    .map_err(RequestError::Dbus),
                None => Err(RequestError::Disconnected),
            }
        });
        let result = match timeouts.response {
            Some(limit) => timeout(limit, wait).await.unwrap_or(Err(RequestError::TimedOut)),
            None => wait.await,
        };
        if result.is_err() {
            let _ = timeout(timeouts.rpc, request.call::<_, _, ()>("Close", &())).await;
        }
        result
    }

    /// Runs a one-shot portal future on a single lazily-started background `LocalExecutor`
    /// thread instead of spawning an unjoined thread per call.
    pub(crate) fn spawn<F, Fut>(make: F) -> Result<(), String>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        type Job = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()>>> + Send>;
        static WORKER: OnceLock<Result<async_channel::Sender<Job>, String>> = OnceLock::new();
        let tx = WORKER
            .get_or_init(|| {
                let (tx, rx) = async_channel::unbounded::<Job>();
                std::thread::Builder::new()
                    .name("rune-portal-worker".into())
                    .spawn(move || {
                        let local = LocalExecutor::new();
                        async_io::block_on(local.run(async {
                            while let Ok(job) = rx.recv().await {
                                local.spawn(job()).detach();
                            }
                        }));
                    })
                    .map_err(|e| e.to_string())?;
                Ok(tx)
            })
            .as_ref()
            .map_err(Clone::clone)?;
        tx.try_send(Box::new(move || Box::pin(make()))).map_err(|_| "portal worker stopped".to_owned())
    }
}

#[cfg(all(target_os = "linux", feature = "dbus"))]
pub(crate) use dbus::{DEST, RequestError, RequestTimeouts, is_unavailable, request, spawn, timeout};

#[cfg(test)]
#[path = "portal_tests.rs"]
mod tests;
