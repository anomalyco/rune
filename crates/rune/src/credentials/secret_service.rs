//! The freedesktop Secret Service over D-Bus (GNOME Keyring, KWallet, KeePassXC): one item per
//! service in the default collection, with the attributes `xdg:schema` ([`SCHEMA`]), `service`
//! and `account`, so `secret-tool lookup xdg:schema dev.rune.Credential service <service>` finds
//! it. The schema keeps Rune's items apart from other apps' that also key by `service` (the
//! `keyring` crate's, say). Secrets travel over the session bus in the
//! `plain` session, as libsecret's do without its optional encryption. A locked collection or item
//! is unlocked through the service's own prompt.

use std::collections::HashMap;
use std::time::Duration;

use futures_lite::StreamExt;
use zeroize::Zeroize;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, Proxy};

use super::{Credential, CredentialError};
use crate::platform::portal::timeout;

const DEST: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE: &str = "org.freedesktop.Secret.Service";
const COLLECTION: &str = "org.freedesktop.Secret.Collection";
const ITEM: &str = "org.freedesktop.Secret.Item";
const PROMPT: &str = "org.freedesktop.Secret.Prompt";
/// The `xdg:schema` attribute of every item Rune stores, as libsecret names its schemas.
pub(super) const SCHEMA: &str = "dev.rune.Credential";
/// One D-Bus round trip.
const RPC: Duration = Duration::from_secs(25);
/// A prompt the person answers (an unlock password).
const ANSWER: Duration = Duration::from_secs(300);

/// `(session, parameters, value, content type)`, the Secret Service's `Secret` struct.
type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

fn dbus(e: zbus::Error) -> CredentialError {
    if crate::platform::portal::is_unavailable(&e) { return CredentialError::Unsupported }
    CredentialError::Failed(e.to_string())
}

async fn rpc<T>(future: impl Future<Output = zbus::Result<T>>) -> Result<T, CredentialError> {
    timeout(RPC, future).await.ok_or_else(|| CredentialError::Failed("the Secret Service didn't answer".into()))?.map_err(dbus)
}

struct Session<'a> {
    conn: &'a Connection,
    service: Proxy<'a>,
    path: OwnedObjectPath,
}

impl<'a> Session<'a> {
    async fn open(conn: &'a Connection) -> Result<Session<'a>, CredentialError> {
        let service = rpc(Proxy::new(conn, DEST, SERVICE_PATH, SERVICE)).await?;
        let (_, path): (OwnedValue, OwnedObjectPath) = rpc(service.call("OpenSession", &("plain", Value::from("")))).await?;
        Ok(Session { conn, service, path })
    }

    async fn proxy(&self, path: &OwnedObjectPath, interface: &'static str) -> Result<Proxy<'a>, CredentialError> {
        rpc(Proxy::new(self.conn, DEST, path.as_str().to_owned(), interface)).await
    }

    /// Shows the prompt at `path` unless it is `/`, and waits for the person's answer: the
    /// prompt's result, or `None` when no prompt was needed.
    async fn prompt(&self, path: OwnedObjectPath) -> Result<Option<OwnedValue>, CredentialError> {
        if path.as_str() == "/" { return Ok(None) }
        let prompt = self.proxy(&path, PROMPT).await?;
        let mut completed = rpc(prompt.receive_signal("Completed")).await?;
        rpc(prompt.call::<_, _, ()>("Prompt", &("",))).await?;
        let message = timeout(ANSWER, completed.next()).await.flatten().ok_or(CredentialError::Denied)?;
        let (dismissed, result): (bool, OwnedValue) = message.body().deserialize().map_err(dbus)?;
        if dismissed { Err(CredentialError::Denied) } else { Ok(Some(result)) }
    }

    async fn unlock(&self, objects: Vec<OwnedObjectPath>) -> Result<(), CredentialError> {
        if objects.is_empty() { return Ok(()) }
        let (_, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = rpc(self.service.call("Unlock", &(objects,))).await?;
        self.prompt(prompt).await.map(drop)
    }

    /// The items stored for `service`: every one, unlocked (with the service's prompt), or with
    /// `unlock` false, only those already unlocked.
    async fn items(&self, service: &str, unlock: bool) -> Result<Vec<OwnedObjectPath>, CredentialError> {
        let attributes = HashMap::from([("xdg:schema", SCHEMA), ("service", service)]);
        let (mut unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) =
            rpc(self.service.call("SearchItems", &(attributes,))).await?;
        if unlock {
            unlocked.extend(locked.iter().cloned());
            self.unlock(locked).await?;
        }
        Ok(unlocked)
    }

    /// The default collection, unlocked; made (with the service's prompt) when there is none.
    async fn collection(&self) -> Result<OwnedObjectPath, CredentialError> {
        let mut path: OwnedObjectPath = rpc(self.service.call("ReadAlias", &("default",))).await?;
        if path.as_str() == "/" {
            let label = HashMap::from([("org.freedesktop.Secret.Collection.Label", Value::from("Login"))]);
            let (made, prompt): (OwnedObjectPath, OwnedObjectPath) =
                rpc(self.service.call("CreateCollection", &(label, "default"))).await?;
            self.prompt(prompt).await?;
            path = if made.as_str() == "/" { rpc(self.service.call("ReadAlias", &("default",))).await? } else { made };
            if path.as_str() == "/" { return Err(CredentialError::Denied) }
        }
        self.unlock(vec![path.clone()]).await?;
        Ok(path)
    }

    /// Deletes the service's items, or with `keep`, the unlocked ones whose account isn't `keep`
    /// (a write's tidy-up, which asks nobody to unlock anything).
    async fn delete(&self, service: &str, keep: Option<&str>) -> Result<(), CredentialError> {
        for item in self.items(service, keep.is_none()).await? {
            let item = self.proxy(&item, ITEM).await?;
            if let Some(keep) = keep {
                let attributes: HashMap<String, String> = rpc(item.get_property("Attributes")).await?;
                if attributes.get("account").map(String::as_str) == Some(keep) { continue }
            }
            let prompt: OwnedObjectPath = rpc(item.call("Delete", &())).await?;
            self.prompt(prompt).await?;
        }
        Ok(())
    }

    async fn close(self) {
        if let Ok(session) = Proxy::new(self.conn, DEST, self.path.as_str().to_owned(), "org.freedesktop.Secret.Session").await {
            let _ = timeout(RPC, session.call::<_, _, ()>("Close", &())).await;
        }
    }
}

/// Runs `work` on a session of its own, on this (worker) thread.
fn run<T>(work: impl AsyncFnOnce(&Session<'_>) -> Result<T, CredentialError>) -> Result<T, CredentialError> {
    async_io::block_on(async {
        // No session bus means no Secret Service to ask.
        let conn = match timeout(RPC, Connection::session()).await {
            Some(Ok(conn)) => conn,
            Some(Err(_)) => return Err(CredentialError::Unsupported),
            None => return Err(CredentialError::Failed("the session bus didn't answer".into())),
        };
        let session = Session::open(&conn).await?;
        let result = work(&session).await;
        session.close().await;
        conn.close().await.ok();
        result
    })
}

/// Unlocks (or makes) the default collection and stores the new item, so a dismissed prompt or a
/// failed `CreateItem` leaves the old one. Once `CreateItem` succeeds the write is done: removing
/// the service's items for other accounts afterwards is a tidy-up whose failure is ignored, and
/// [`read`] picks the newest item if one is left.
pub(super) fn write(service: &str, account: &str, secret: &[u8]) -> Result<(), CredentialError> {
    run(async |s| {
        let collection = s.proxy(&s.collection().await?, COLLECTION).await?;
        let attributes = HashMap::from([("xdg:schema", SCHEMA), ("service", service), ("account", account)]);
        let label = if account.is_empty() { service.to_owned() } else { format!("{account} @ {service}") };
        let properties = HashMap::from([
            ("org.freedesktop.Secret.Item.Label", Value::from(label)),
            ("org.freedesktop.Secret.Item.Attributes", Value::from(attributes)),
        ]);
        let mut value: Secret = (s.path.clone(), Vec::new(), secret.to_vec(), "application/octet-stream".into());
        // `replace`: an item with the same attributes (this service and account) is updated in place.
        let created = rpc(collection.call::<_, _, (OwnedObjectPath, OwnedObjectPath)>("CreateItem", &(properties, &value, true))).await;
        value.2.zeroize();
        let (_, prompt) = created?;
        s.prompt(prompt).await?;
        // Committed: a leftover item for another account is older, and `read` passes over it.
        s.delete(service, Some(account)).await.ok();
        Ok(())
    })
}

pub(super) fn read(service: &str) -> Result<Option<Credential>, CredentialError> {
    run(async |s| {
        // The most recently modified item, then the greatest path, so several items (a write's
        // tidy-up that failed, or items in more than one collection) always give the same answer.
        let mut newest: Option<(u64, OwnedObjectPath, Proxy<'_>)> = None;
        for path in s.items(service, true).await? {
            let item = s.proxy(&path, ITEM).await?;
            let modified: u64 = rpc(item.get_property("Modified")).await?;
            if newest.as_ref().is_none_or(|(m, p, _)| (modified, path.as_str()) > (*m, p.as_str())) {
                newest = Some((modified, path, item));
            }
        }
        let Some((_, _, item)) = newest else { return Ok(None) };
        let attributes: HashMap<String, String> = rpc(item.get_property("Attributes")).await?;
        let account = attributes.get("account").cloned().unwrap_or_default();
        // Last, so the secret goes straight into the `Credential`, which zeroes it.
        let (_, _, secret, _): Secret = rpc(item.call("GetSecret", &(&s.path,))).await?;
        Ok(Some(Credential::new(account, secret)))
    })
}

pub(super) fn delete(service: &str) -> Result<(), CredentialError> {
    run(async |s| s.delete(service, None).await)
}
