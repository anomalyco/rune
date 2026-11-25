//! The freedesktop Secret Service over D-Bus (GNOME Keyring, KWallet, KeePassXC): one item per
//! service in the default collection, with the attributes `xdg:schema` ([`SCHEMA`]), `service`
//! and `account`, so `secret-tool lookup xdg:schema dev.rune.Credential service <service>` finds
//! it. The schema keeps Rune's items apart from other apps' that also key by `service` (the
//! `keyring` crate's, say). Secrets travel over the session bus in the `plain` session, as
//! libsecret's do without its optional encryption. A locked collection or item is unlocked
//! through the service's own prompt.

use std::collections::HashSet;

use zeroize::Zeroizing;

use super::{Credential, CredentialError};

/// The `xdg:schema` attribute of every item Rune stores, as libsecret names its schemas.
pub(super) const SCHEMA: &str = "dev.rune.Credential";

/// Plaintext secret bytes that zero themselves on drop, including across early returns, future
/// cancellation, and panics.
pub(super) struct SecretPayload(Zeroizing<Vec<u8>>);

impl SecretPayload {
    pub(super) fn new(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }

    pub(super) fn as_slice(&self) -> &[u8] {
        self.0.as_slice()
    }

    pub(super) fn into_zeroizing(self) -> Zeroizing<Vec<u8>> {
        self.0
    }
}

/// Minimal seam over the freedesktop Secret Service D-Bus protocol so unit tests can drive unlock
/// verification, prompt results, cross-collection tidy-up, and secret zeroing without a live
/// session bus or keyring daemon.
pub(super) trait SecretOps {
    async fn search_items(&self, service: &str) -> Result<(Vec<String>, Vec<String>), CredentialError>;
    async fn unlock(&self, objects: &[String]) -> Result<(Vec<String>, String), CredentialError>;
    async fn prompt_unlock(&self, prompt: &str) -> Result<Vec<String>, CredentialError>;
    async fn read_alias(&self, alias: &str) -> Result<String, CredentialError>;
    async fn create_collection(&self, label: &str, alias: &str) -> Result<(String, String), CredentialError>;
    async fn prompt_object_path(&self, prompt: &str) -> Result<String, CredentialError>;
    async fn create_item(
        &self,
        collection: &str,
        service: &str,
        account: &str,
        label: &str,
        secret: &SecretPayload,
    ) -> Result<(String, String), CredentialError>;
    async fn item_modified(&self, item: &str) -> Result<u64, CredentialError>;
    async fn item_account(&self, item: &str) -> Result<String, CredentialError>;
    async fn get_secret(&self, item: &str) -> Result<SecretPayload, CredentialError>;
    async fn delete_item(&self, item: &str) -> Result<String, CredentialError>;
    async fn prompt_unit(&self, prompt: &str) -> Result<(), CredentialError>;
}

/// Unlocks every object path in `objects` per Secret Service 0.2 `Service.Unlock`: checks both
/// the immediately-unlocked list and the prompt's `Completed` result so a partial or refused
/// unlock returns [`CredentialError::Denied`].
async fn unlock_with<O: SecretOps>(ops: &O, objects: &[String]) -> Result<(), CredentialError> {
    if objects.is_empty() {
        return Ok(());
    }
    let (already_unlocked, prompt) = ops.unlock(objects).await?;
    let mut unlocked: HashSet<String> = already_unlocked.into_iter().collect();
    if prompt != "/" {
        unlocked.extend(ops.prompt_unlock(&prompt).await?);
    }
    if objects.iter().all(|path| unlocked.contains(path)) {
        Ok(())
    } else {
        Err(CredentialError::Denied)
    }
}

/// The items stored for `service`: every one, unlocked (with the service's prompt), or with
/// `unlock` false, only those already unlocked.
async fn items_with<O: SecretOps>(ops: &O, service: &str, unlock: bool) -> Result<Vec<String>, CredentialError> {
    let (mut unlocked, locked) = ops.search_items(service).await?;
    if unlock {
        unlock_with(ops, &locked).await?;
        unlocked.extend(locked);
    }
    Ok(unlocked)
}

/// The default collection, unlocked; made (with the service's prompt) when there is none.
async fn collection_with<O: SecretOps>(ops: &O) -> Result<String, CredentialError> {
    let mut path = ops.read_alias("default").await?;
    if path == "/" {
        let (made, prompt) = ops.create_collection("Login", "default").await?;
        let from_prompt = if prompt == "/" {
            None
        } else {
            let p = ops.prompt_object_path(&prompt).await?;
            (p != "/").then_some(p)
        };
        path = if let Some(p) = from_prompt {
            p
        } else if made != "/" {
            made
        } else {
            ops.read_alias("default").await?
        };
        if path == "/" {
            return Err(CredentialError::Denied);
        }
    }
    unlock_with(ops, std::slice::from_ref(&path)).await?;
    Ok(path)
}

/// Deletes the service's items, or with `keep_path`, every unlocked item for the service whose
/// object path is not `keep_path` (a write's tidy-up, which removes duplicates in other
/// collections even when they carry the same account, without prompting to unlock locked ones).
async fn delete_with<O: SecretOps>(
    ops: &O,
    service: &str,
    keep_path: Option<&str>,
) -> Result<(), CredentialError> {
    for item in items_with(ops, service, keep_path.is_none()).await? {
        if keep_path == Some(item.as_str()) {
            continue;
        }
        let prompt = ops.delete_item(&item).await?;
        if prompt != "/" {
            ops.prompt_unit(&prompt).await?;
        }
    }
    Ok(())
}

pub(super) async fn write_with<O: SecretOps>(
    ops: &O,
    service: &str,
    account: &str,
    secret: &[u8],
) -> Result<(), CredentialError> {
    let collection = collection_with(ops).await?;
    let label = if account.is_empty() { service.to_owned() } else { format!("{account} @ {service}") };
    let payload = SecretPayload::new(secret.to_vec());
    let (made, prompt) = ops.create_item(&collection, service, account, &label, &payload).await?;
    drop(payload);
    let created_path = if prompt == "/" {
        made
    } else {
        let from_prompt = ops.prompt_object_path(&prompt).await?;
        if from_prompt != "/" { from_prompt } else { made }
    };
    // Committed: tidy up any other unlocked items for this service (other accounts or duplicates
    // in other collections). If a daemon did not return the created item's path, skip tidy-up
    // rather than risk deleting the item just stored.
    if created_path != "/" {
        delete_with(ops, service, Some(&created_path)).await.ok();
    }
    Ok(())
}

pub(super) async fn read_with<O: SecretOps>(ops: &O, service: &str) -> Result<Option<Credential>, CredentialError> {
    // The most recently modified item, then the greatest path, so several items (a write's
    // tidy-up that failed, or items in more than one collection) always give the same answer.
    let mut newest: Option<(u64, String)> = None;
    for path in items_with(ops, service, true).await? {
        let modified = ops.item_modified(&path).await?;
        if newest.as_ref().is_none_or(|(m, p)| (modified, path.as_str()) > (*m, p.as_str())) {
            newest = Some((modified, path));
        }
    }
    let Some((_, path)) = newest else { return Ok(None) };
    let account = ops.item_account(&path).await?;
    // Last, so the secret goes straight into `Credential`, which keeps it zeroed on drop.
    let secret = ops.get_secret(&path).await?;
    Ok(Some(Credential::from_zeroizing(account, secret.into_zeroizing())))
}

pub(super) async fn delete_all_with<O: SecretOps>(ops: &O, service: &str) -> Result<(), CredentialError> {
    delete_with(ops, service, None).await
}

#[cfg(all(target_os = "linux", feature = "dbus"))]
mod dbus {
    use std::collections::HashMap;
    use std::time::Duration;

    use futures_lite::StreamExt;
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
    use zbus::{Connection, Proxy};

    use super::{
        SCHEMA, SecretOps, SecretPayload, delete_all_with, read_with, write_with,
    };
    use crate::credentials::{Credential, CredentialError};
    use crate::platform::portal::timeout;

    const DEST: &str = "org.freedesktop.secrets";
    const SERVICE_PATH: &str = "/org/freedesktop/secrets";
    const SERVICE: &str = "org.freedesktop.Secret.Service";
    const COLLECTION: &str = "org.freedesktop.Secret.Collection";
    const ITEM: &str = "org.freedesktop.Secret.Item";
    const PROMPT: &str = "org.freedesktop.Secret.Prompt";
    /// One D-Bus round trip.
    const RPC: Duration = Duration::from_secs(25);
    /// A prompt the person answers (an unlock password).
    const ANSWER: Duration = Duration::from_secs(300);

    /// `(session, parameters, value, content type)`, the Secret Service's `Secret` struct.
    type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

    fn map_dbus(e: zbus::Error) -> CredentialError {
        if crate::platform::portal::is_unavailable(&e) {
            return CredentialError::Unsupported;
        }
        CredentialError::Failed(e.to_string())
    }

    async fn rpc<T>(future: impl Future<Output = zbus::Result<T>>) -> Result<T, CredentialError> {
        timeout(RPC, future)
            .await
            .ok_or_else(|| CredentialError::Failed("the Secret Service didn't answer".into()))?
            .map_err(map_dbus)
    }

    struct Session<'a> {
        conn: &'a Connection,
        service: Proxy<'a>,
        path: OwnedObjectPath,
    }

    impl<'a> Session<'a> {
        async fn open(conn: &'a Connection) -> Result<Session<'a>, CredentialError> {
            let service = rpc(Proxy::new(conn, DEST, SERVICE_PATH, SERVICE)).await?;
            let (_, path): (OwnedValue, OwnedObjectPath) =
                rpc(service.call("OpenSession", &("plain", Value::from("")))).await?;
            Ok(Session { conn, service, path })
        }

        async fn proxy(&self, path: &str, interface: &'static str) -> Result<Proxy<'a>, CredentialError> {
            rpc(Proxy::new(self.conn, DEST, path.to_owned(), interface)).await
        }

        /// Shows the prompt at `path` unless it is `/`, and waits for the person's answer: the
        /// prompt's result, or `None` when no prompt was needed.
        async fn prompt(&self, path: &str) -> Result<Option<OwnedValue>, CredentialError> {
            if path == "/" {
                return Ok(None);
            }
            let prompt = self.proxy(path, PROMPT).await?;
            let mut completed = rpc(prompt.receive_signal("Completed")).await?;
            rpc(prompt.call::<_, _, ()>("Prompt", &("",))).await?;
            let message = timeout(ANSWER, completed.next()).await.flatten().ok_or(CredentialError::Denied)?;
            let (dismissed, result): (bool, OwnedValue) = message.body().deserialize().map_err(map_dbus)?;
            if dismissed { Err(CredentialError::Denied) } else { Ok(Some(result)) }
        }

        async fn close(self) {
            if let Ok(session) = Proxy::new(
                self.conn,
                DEST,
                self.path.as_str().to_owned(),
                "org.freedesktop.Secret.Session",
            )
            .await
            {
                let _ = timeout(RPC, session.call::<_, _, ()>("Close", &())).await;
            }
        }
    }

    impl SecretOps for Session<'_> {
        async fn search_items(&self, service: &str) -> Result<(Vec<String>, Vec<String>), CredentialError> {
            let attributes = HashMap::from([("xdg:schema", SCHEMA), ("service", service)]);
            let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) =
                rpc(self.service.call("SearchItems", &(attributes,))).await?;
            Ok((
                unlocked.into_iter().map(|p| p.to_string()).collect(),
                locked.into_iter().map(|p| p.to_string()).collect(),
            ))
        }

        async fn unlock(&self, objects: &[String]) -> Result<(Vec<String>, String), CredentialError> {
            let paths: Vec<OwnedObjectPath> = objects
                .iter()
                .map(|p| OwnedObjectPath::try_from(p.as_str()).map_err(|e| CredentialError::Failed(e.to_string())))
                .collect::<Result<_, _>>()?;
            let (unlocked, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) =
                rpc(self.service.call("Unlock", &(paths,))).await?;
            Ok((unlocked.into_iter().map(|p| p.to_string()).collect(), prompt.to_string()))
        }

        async fn prompt_unlock(&self, prompt: &str) -> Result<Vec<String>, CredentialError> {
            let Some(result) = self.prompt(prompt).await? else { return Ok(Vec::new()) };
            Vec::<OwnedObjectPath>::try_from(result)
                .map(|paths| paths.into_iter().map(|p| p.to_string()).collect())
                .map_err(|e| CredentialError::Failed(format!("Secret Service unlock prompt returned unexpected result: {e}")))
        }

        async fn read_alias(&self, alias: &str) -> Result<String, CredentialError> {
            let path: OwnedObjectPath = rpc(self.service.call("ReadAlias", &(alias,))).await?;
            Ok(path.to_string())
        }

        async fn create_collection(&self, label: &str, alias: &str) -> Result<(String, String), CredentialError> {
            let props = HashMap::from([("org.freedesktop.Secret.Collection.Label", Value::from(label))]);
            let (made, prompt): (OwnedObjectPath, OwnedObjectPath) =
                rpc(self.service.call("CreateCollection", &(props, alias))).await?;
            Ok((made.to_string(), prompt.to_string()))
        }

        async fn prompt_object_path(&self, prompt: &str) -> Result<String, CredentialError> {
            let Some(result) = self.prompt(prompt).await? else { return Ok("/".into()) };
            OwnedObjectPath::try_from(result)
                .map(|p| p.to_string())
                .map_err(|e| CredentialError::Failed(format!("Secret Service prompt returned unexpected result: {e}")))
        }

        async fn create_item(
            &self,
            collection: &str,
            service: &str,
            account: &str,
            label: &str,
            secret: &SecretPayload,
        ) -> Result<(String, String), CredentialError> {
            let collection = self.proxy(collection, COLLECTION).await?;
            let attributes = HashMap::from([("xdg:schema", SCHEMA), ("service", service), ("account", account)]);
            let properties = HashMap::from([
                ("org.freedesktop.Secret.Item.Label", Value::from(label)),
                ("org.freedesktop.Secret.Item.Attributes", Value::from(attributes)),
            ]);
            let empty: &[u8] = &[];
            let value = (&self.path, empty, secret.as_slice(), "application/octet-stream");
            // `replace`: an item with the same attributes in this collection is updated in place.
            let (made, prompt): (OwnedObjectPath, OwnedObjectPath) =
                rpc(collection.call("CreateItem", &(properties, &value, true))).await?;
            Ok((made.to_string(), prompt.to_string()))
        }

        async fn item_modified(&self, item: &str) -> Result<u64, CredentialError> {
            let item = self.proxy(item, ITEM).await?;
            rpc(item.get_property("Modified")).await
        }

        async fn item_account(&self, item: &str) -> Result<String, CredentialError> {
            let item = self.proxy(item, ITEM).await?;
            let attributes: HashMap<String, String> = rpc(item.get_property("Attributes")).await?;
            Ok(attributes.get("account").cloned().unwrap_or_default())
        }

        async fn get_secret(&self, item: &str) -> Result<SecretPayload, CredentialError> {
            let item = self.proxy(item, ITEM).await?;
            let (_, _, secret, _): Secret = rpc(item.call("GetSecret", &(&self.path,))).await?;
            Ok(SecretPayload::new(secret))
        }

        async fn delete_item(&self, item: &str) -> Result<String, CredentialError> {
            let item = self.proxy(item, ITEM).await?;
            let prompt: OwnedObjectPath = rpc(item.call("Delete", &())).await?;
            Ok(prompt.to_string())
        }

        async fn prompt_unit(&self, prompt: &str) -> Result<(), CredentialError> {
            self.prompt(prompt).await.map(drop)
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

    /// Unlocks (or makes) the default collection and stores the new item, so a dismissed prompt or
    /// a failed `CreateItem` leaves the old one. Once `CreateItem` succeeds the write is done:
    /// removing other unlocked items for the service afterwards is a tidy-up whose failure is
    /// ignored, and [`read`] picks the newest item if one is left.
    pub(in super::super) fn write(service: &str, account: &str, secret: &[u8]) -> Result<(), CredentialError> {
        run(async |s| write_with(s, service, account, secret).await)
    }

    pub(in super::super) fn read(service: &str) -> Result<Option<Credential>, CredentialError> {
        run(async |s| read_with(s, service).await)
    }

    pub(in super::super) fn delete(service: &str) -> Result<(), CredentialError> {
        run(async |s| delete_all_with(s, service).await)
    }
}

#[cfg(all(target_os = "linux", feature = "dbus"))]
pub(super) use dbus::{delete, read, write};
