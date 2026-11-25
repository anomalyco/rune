//! Secrets kept in the platform's credential store between launches: an API token, a password,
//! a refresh token.
//!
//! [`Credentials::system`] is the store the platform keeps for the person: the Keychain on macOS
//! and iOS (generic passwords), Credential Manager on Windows (generic credentials) and the
//! Secret Service over D-Bus on Linux (GNOME Keyring, KWallet; needs Rune's `dbus` feature).
//! The web and Android have no store and return [`CredentialError::Unsupported`], so an app can
//! fall back to asking each launch.
//!
//! A credential is keyed by a service name (a URL or a reverse-DNS name such as
//! `com.example.notes.api`) and holds one account name and a secret as bytes. Writing a
//! service's credential replaces the one it had, whatever its account.
//!
//! Every call blocks: the store can take a while and can show a system prompt (an unlock dialog,
//! a Keychain access request). Never call it on the UI thread; run it with
//! [`Latest::request`](crate::Latest::request) or [`task::spawn`](crate::task::spawn) and land
//! the answer in `poll`:
//!
//! ```no_run
//! use rune::credentials::Credentials;
//!
//! // Off the UI thread, in a `Latest::request` closure, say.
//! let store = Credentials::system();
//! store.write("com.example.images.api", "me", b"token").ok();
//! if let Ok(Some(found)) = store.read("com.example.images.api") {
//!     let _token: &[u8] = found.secret();
//! }
//! ```
//!
//! Headless work never touches the person's store: [`Credentials::system`] hands it one
//! in-memory store shared by the process, so a scripted run, an agent session or a test can
//! write and read without a prompt and leaves nothing behind. That is the case once the process
//! has a headless run (`--snap`, `--script`, `--tree`, `--html`; [`crate::tooling::headless_run`]
//! or [`crate::is_headless`]), once it has built an app with `App::headless_runtime`, and under
//! `RUNE_CREDENTIALS=memory`. Windows, overlays and an app's own [`Runtime::new`](crate::Runtime::new)
//! (for thumbnails or an export, say) keep the platform's store. So a test that drives a
//! `Runtime::new` itself, or makes no runtime at all, can't be told apart from the app: use
//! [`Credentials::memory`], a store of its own, or set `RUNE_CREDENTIALS=memory`.
//!
//! A failed `write` leaves what the service held before. A [`Credential`]'s `Debug` output never
//! shows its secret, and the copy it holds is zeroed when it is dropped; copies the platform's
//! own API or the D-Bus connection make are outside Rune's reach.
//!
//! Verification status: the Windows backend has been exercised against the real Credential
//! Manager through an opt-in ignored test. The macOS/iOS Keychain and Linux Secret Service
//! backends have been compiled and unit-tested behind internal seams, without live calls to a
//! real Keychain or Secret Service daemon.

use std::collections::BTreeMap;
use std::fmt;
#[cfg(not(test))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use zeroize::Zeroizing;

#[cfg(any(test, target_vendor = "apple"))]
mod apple;
#[cfg(any(test, all(target_os = "linux", feature = "dbus")))]
mod secret_service;
#[cfg(test)]
mod tests;
#[cfg(target_os = "windows")]
mod windows;

/// The environment variable that, set to `memory`, gives [`Credentials::system`] the process's
/// in-memory store instead of the platform's.
pub const MEMORY_ENV: &str = "RUNE_CREDENTIALS";

/// Why a credential couldn't be read, written or deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialError {
    /// The service name was empty.
    Empty,
    /// The service or account name can't be stored (it holds a NUL character or exceeds the
    /// store's length limit), or the secret is larger than the store keeps (Credential Manager
    /// keeps 2,560 bytes).
    Invalid,
    /// This platform has no credential store (the web, Android, Linux without the `dbus`
    /// feature).
    Unsupported,
    /// The person declined a prompt, or the store stayed locked.
    Denied,
    /// The store failed; the message says how.
    Failed(String),
}

impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("empty service name"),
            Self::Invalid => f.write_str("the credential can't be stored"),
            Self::Unsupported => f.write_str("this platform has no credential store"),
            Self::Denied => f.write_str("access to the credential store was denied"),
            Self::Failed(why) => write!(f, "credential store failed: {why}"),
        }
    }
}
impl std::error::Error for CredentialError {}

/// A stored credential: its account name and secret. `Debug` never shows the secret.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    account: String,
    secret: Zeroizing<Vec<u8>>,
}

impl Credential {
    pub(crate) fn new(account: String, secret: Vec<u8>) -> Self {
        Self::from_zeroizing(account, Zeroizing::new(secret))
    }

    pub(crate) fn from_zeroizing(account: String, secret: Zeroizing<Vec<u8>>) -> Self {
        Self { account, secret }
    }

    /// The account name it was written with.
    pub fn account(&self) -> &str { &self.account }

    /// The secret's bytes.
    pub fn secret(&self) -> &[u8] { &self.secret }
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credential").field("account", &self.account).field("secret", &"<redacted>").finish()
    }
}

type Memory = Arc<Mutex<BTreeMap<String, Credential>>>;

#[derive(Clone)]
enum Store {
    System,
    Memory(Memory),
}

/// A credential store. See the [module docs](self).
#[derive(Clone)]
pub struct Credentials {
    store: Store,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.store {
            Store::System => "Credentials::system",
            Store::Memory(_) => "Credentials::memory",
        })
    }
}

impl Credentials {
    /// The platform's credential store; for headless work (see the [module docs](self)) and under
    /// `RUNE_CREDENTIALS=memory`, one in-memory store shared by the process.
    pub fn system() -> Self {
        if !headless() { return Self { store: Store::System } }
        static SHARED: OnceLock<Memory> = OnceLock::new();
        Self { store: Store::Memory(SHARED.get_or_init(Memory::default).clone()) }
    }

    /// A store that lives only as long as this value and its clones, for tests.
    pub fn memory() -> Self { Self { store: Store::Memory(Memory::default()) } }

    /// Whether this store outlives the process: false for in-memory stores.
    pub fn is_persistent(&self) -> bool { matches!(self.store, Store::System) }

    /// Stores `secret` for `service` under `account`, replacing what `service` held; a failed write
    /// leaves it as it was. Blocks; call it off the UI thread.
    ///
    /// On Linux the write is done once the new item is stored; removing other unlocked items for
    /// the service afterwards (including duplicates in other collections) is a tidy-up, and one
    /// it couldn't remove (in a locked collection, say) is older, so [`read`](Self::read) returns
    /// the new one.
    ///
    /// Rune keeps one item per service. On macOS and iOS, when another tool left several Keychain
    /// items for the service under different accounts, this fails with
    /// [`CredentialError::Failed`] and changes nothing; [`delete`](Self::delete) clears them.
    pub fn write(&self, service: &str, account: &str, secret: &[u8]) -> Result<(), CredentialError> {
        check(service)?;
        if account.contains('\0') { return Err(CredentialError::Invalid) }
        match &self.store {
            Store::Memory(map) => {
                lock(map).insert(service.to_owned(), Credential::new(account.to_owned(), secret.to_vec()));
                Ok(())
            }
            Store::System => system::write(service, account, secret),
        }
    }

    /// The credential stored for `service`, or `None` when there is none. Blocks; call it off
    /// the UI thread.
    pub fn read(&self, service: &str) -> Result<Option<Credential>, CredentialError> {
        check(service)?;
        match &self.store {
            Store::Memory(map) => Ok(lock(map).get(service).cloned()),
            Store::System => system::read(service),
        }
    }

    /// Removes the credential stored for `service`; nothing stored is not an error. Blocks; call
    /// it off the UI thread.
    pub fn delete(&self, service: &str) -> Result<(), CredentialError> {
        check(service)?;
        match &self.store {
            Store::Memory(map) => {
                lock(map).remove(service);
                Ok(())
            }
            Store::System => system::delete(service),
        }
    }
}

#[cfg(not(test))]
static HEADLESS_RUNTIME: AtomicBool = AtomicBool::new(false);
// Per test thread, so tests that start headless runs don't change what other tests see.
#[cfg(test)]
thread_local!(static HEADLESS_RUNTIME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });

/// Called by Rune's headless entry points (a scripted or snapshot run, the HTML export,
/// `App::headless_runtime`) before the app's state is made: from then on
/// [`Credentials::system`] is the in-memory store. Never called for a window, an overlay or an
/// app's own `Runtime::new`.
pub(crate) fn note_headless_runtime() {
    #[cfg(not(test))]
    HEADLESS_RUNTIME.store(true, Ordering::Relaxed);
    #[cfg(test)]
    HEADLESS_RUNTIME.with(|h| h.set(true));
}

/// Whether [`note_headless_runtime`] ran (in tests, on this thread).
#[cfg(not(test))]
pub(crate) fn headless_runtime_noted() -> bool { HEADLESS_RUNTIME.load(Ordering::Relaxed) }
#[cfg(test)]
pub(crate) fn headless_runtime_noted() -> bool { HEADLESS_RUNTIME.with(std::cell::Cell::get) }

fn headless() -> bool {
    cfg!(test)
        || headless_runtime_noted()
        || rune_core::env::var(MEMORY_ENV).is_some_and(|v| v == "memory")
        || crate::tooling::headless_run().is_some()
        || crate::is_headless()
}

fn check(service: &str) -> Result<(), CredentialError> {
    if service.is_empty() { return Err(CredentialError::Empty) }
    if service.contains('\0') { return Err(CredentialError::Invalid) }
    Ok(())
}

fn lock(map: &Memory) -> std::sync::MutexGuard<'_, BTreeMap<String, Credential>> {
    map.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(target_vendor = "apple")]
use apple as system;
#[cfg(all(target_os = "linux", feature = "dbus"))]
use secret_service as system;
#[cfg(target_os = "windows")]
use windows as system;

#[cfg(not(any(target_vendor = "apple", target_os = "windows", all(target_os = "linux", feature = "dbus"))))]
mod system {
    use super::{Credential, CredentialError};

    pub(super) fn write(_: &str, _: &str, _: &[u8]) -> Result<(), CredentialError> { Err(CredentialError::Unsupported) }
    pub(super) fn read(_: &str) -> Result<Option<Credential>, CredentialError> { Err(CredentialError::Unsupported) }
    pub(super) fn delete(_: &str) -> Result<(), CredentialError> { Err(CredentialError::Unsupported) }
}
