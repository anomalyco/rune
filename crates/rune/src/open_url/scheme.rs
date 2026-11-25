//! Make the running app the system's default handler for a URL scheme:
//! [`AppHandle::register_url_scheme`](crate::AppHandle::register_url_scheme).
//!
//! Declaring a scheme (`CFBundleURLTypes` in `Info.plist`, or the registry entry an installer
//! writes) doesn't take it from an app that already handles it. This asks the system for it
//! while the app runs. The request runs on the UI thread as an `AppHandle::update`, so headless
//! runs and tests can record it instead of changing the user's handlers
//! ([`crate::Runtime::take_url_scheme_requests`]).
//!
//! macOS calls `NSWorkspace`'s `setDefaultApplicationAtURL:toOpenURLsWithScheme:completionHandler:`
//! (macOS 12 and later) for the running `.app` bundle. Windows writes the per-user
//! `HKEY_CURRENT_USER\Software\Classes\<scheme>` entry pointing at the running executable, off
//! the UI thread. Elsewhere the answer is [`UrlSchemeError::Unsupported`].

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

/// Why the app couldn't become a scheme's default handler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrlSchemeError {
    /// Not a URL scheme: it must be a letter followed by letters, digits, `+`, `-` or `.`,
    /// without `:` or `//`.
    Invalid,
    /// This platform or build can't claim schemes: Linux, web and iOS, a macOS app not running
    /// from an `.app` bundle, or macOS before 12.
    Unsupported(String),
    /// The system refused or failed, or the app ended before it answered.
    Failed(String),
}

impl std::fmt::Display for UrlSchemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid => f.write_str("not a URL scheme"),
            Self::Unsupported(why) => write!(f, "can't claim a URL scheme here: {why}"),
            Self::Failed(why) => write!(f, "couldn't claim the URL scheme: {why}"),
        }
    }
}
impl std::error::Error for UrlSchemeError {}

const MAX_LEN: usize = 256;

/// `scheme` lowercased, when it's a URL scheme (RFC 3986: `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`).
pub(crate) fn check(scheme: &str) -> Result<String, UrlSchemeError> {
    let valid = !scheme.is_empty()
        && scheme.len() <= MAX_LEN
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if valid { Ok(scheme.to_ascii_lowercase()) } else { Err(UrlSchemeError::Invalid) }
}

#[derive(Default)]
struct Slot {
    result: Option<Result<(), UrlSchemeError>>,
    waker: Option<Waker>,
}

/// Where the answer goes; any thread may send it, once.
#[derive(Clone)]
pub(crate) struct Reply(Arc<Mutex<Slot>>);

impl Reply {
    pub(crate) fn send(&self, result: Result<(), UrlSchemeError>) {
        let waker = {
            let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if slot.result.is_some() { return; }
            slot.result = Some(result);
            slot.waker.take()
        };
        if let Some(waker) = waker { waker.wake(); }
    }
}

/// A scheme an `AppHandle` asked to claim, carried by the `Tx` of the update that asks.
#[derive(Clone)]
pub(crate) struct Claim {
    pub scheme: String,
    pub reply: Reply,
}

impl std::fmt::Debug for Claim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Claim").field("scheme", &self.scheme).finish_non_exhaustive()
    }
}

/// Holds the update's closure's claim; answers `Failed` if the update is dropped without running.
struct Pending(Option<Claim>);

impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(claim) = self.0.take() {
            claim.reply.send(Err(UrlSchemeError::Failed("the app ended before the request ran".into())));
        }
    }
}

/// The answer to [`AppHandle::register_url_scheme`](crate::AppHandle::register_url_scheme).
/// Await it from any executor; it doesn't need the UI thread.
#[must_use = "the scheme is claimed either way; await this to learn whether it worked"]
pub struct RegisterUrlScheme {
    slot: Arc<Mutex<Slot>>,
}

impl Future for RegisterUrlScheme {
    type Output = Result<(), UrlSchemeError>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        match slot.result.take() {
            Some(result) => Poll::Ready(result),
            None => {
                slot.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

impl<S: 'static> crate::AppHandle<S> {
    /// Make this app the system's default handler for `scheme` (`"myapp"`, without `:` or `//`),
    /// so `myapp://` links reach [`App::on_open_url`](crate::App::on_open_url) even when another
    /// app handled the scheme before: for an app replacing another under the same scheme, or a
    /// "make this my default" setting. Callable from any thread; the request runs on the UI
    /// thread as an [`AppHandle::update`](crate::AppHandle::update) does, and the returned future
    /// answers it.
    ///
    /// - macOS 12 and later: `NSWorkspace` makes the running `.app` bundle the scheme's default
    ///   handler. The system may ask the user to confirm; the answer arrives once it's settled.
    ///   A binary not running from a bundle, or an older macOS, gets
    ///   [`UrlSchemeError::Unsupported`]. Keep declaring the scheme in `Info.plist`.
    /// - Windows: writes `HKEY_CURRENT_USER\Software\Classes\<scheme>` (`URL Protocol`, and
    ///   `shell\open\command` running this executable with the link), replacing what another app
    ///   wrote there for the user.
    /// - Linux, web and iOS: [`UrlSchemeError::Unsupported`], so the app can fall back.
    ///
    /// Headless runs and tests don't touch the system: they record the scheme for
    /// [`Runtime::take_url_scheme_requests`](crate::Runtime::take_url_scheme_requests) and answer
    /// `Ok(())`.
    ///
    /// ```no_run
    /// use rune::{AppHandle, UrlSchemeError};
    ///
    /// struct Companion { default_handler: Option<bool> }
    ///
    /// // From the app's own executor, after launch (`App::on_ready`).
    /// async fn claim_links(handle: AppHandle<Companion>) {
    ///     let claimed = match handle.register_url_scheme("rork-companion").await {
    ///         Ok(()) => Some(true),
    ///         // Linux, web, or a dev build without a bundle: say how to set it up instead.
    ///         Err(UrlSchemeError::Unsupported(_)) => None,
    ///         Err(_) => Some(false),
    ///     };
    ///     handle.update(move |s, _| s.default_handler = claimed);
    /// }
    /// ```
    pub fn register_url_scheme(&self, scheme: &str) -> RegisterUrlScheme {
        let slot = Arc::new(Mutex::new(Slot::default()));
        let reply = Reply(slot.clone());
        match check(scheme) {
            Err(e) => reply.send(Err(e)),
            Ok(scheme) => {
                let mut pending = Pending(Some(Claim { scheme, reply }));
                self.update_if(move |_, tx| {
                    if let Some(claim) = pending.0.take() { tx.url_schemes.push(claim); }
                    false
                });
            }
        }
        RegisterUrlScheme { slot }
    }
}

impl crate::runtime::HostState {
    /// Claims each scheme with the system in a live window, or records it in a headless run.
    pub(crate) fn claim_url_schemes(&mut self, claims: Vec<Claim>) {
        for Claim { scheme, reply } in claims {
            if self.open_urls_natively {
                register(scheme, reply);
            } else {
                self.push_url_scheme_request(scheme);
                reply.send(Ok(()));
            }
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn register(scheme: String, reply: Reply) {
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{class, msg_send, sel};
    use objc2_foundation::{NSError, NSString};
    // SAFETY: runs on the UI (main) thread, which AppKit's NSWorkspace wants. `mainBundle`,
    // `bundlePath`, `bundleIdentifier` and `bundleURL` return autoreleased objects or nil;
    // `setDefaultApplicationAtURL:toOpenURLsWithScheme:completionHandler:` (macOS 12) is only
    // sent once `respondsToSelector:` says it exists, and copies the block.
    autoreleasepool(|_| unsafe {
        let unsupported = |why: &str| reply.send(Err(UrlSchemeError::Unsupported(why.into())));
        let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
        if bundle.is_null() { return unsupported("no main bundle"); }
        let path: Option<Retained<NSString>> = msg_send![bundle, bundlePath];
        let id: Option<Retained<NSString>> = msg_send![bundle, bundleIdentifier];
        let url: *mut AnyObject = msg_send![bundle, bundleURL];
        if !path.is_some_and(|p| p.to_string().ends_with(".app")) || id.is_none() || url.is_null() {
            return unsupported("the app isn't running from an .app bundle with a bundle identifier");
        }
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        if workspace.is_null() { return unsupported("NSWorkspace unavailable"); }
        let responds: Bool = msg_send![workspace, respondsToSelector: sel!(setDefaultApplicationAtURL:toOpenURLsWithScheme:completionHandler:)];
        if !responds.as_bool() { return unsupported("needs macOS 12 or later"); }
        let ns_scheme = NSString::from_str(&scheme);
        let answer = reply.clone();
        let done = block2::RcBlock::new(move |error: *mut NSError| {
            answer.send(match error.as_ref() {
                Some(e) => Err(UrlSchemeError::Failed(e.localizedDescription().to_string())),
                None => Ok(()),
            });
        });
        let _: () = msg_send![
            workspace,
            setDefaultApplicationAtURL: url,
            toOpenURLsWithScheme: &*ns_scheme,
            completionHandler: &*done
        ];
    });
}

#[cfg(target_os = "windows")]
fn register(scheme: String, reply: Reply) {
    // Registry writes go through antivirus filters; keep them off the UI thread.
    let answer = reply.clone();
    let spawned = rune_core::task::spawn("rune url scheme", move || answer.send(write_registry(&scheme)));
    if let Err(e) = spawned { reply.send(Err(UrlSchemeError::Failed(format!("thread: {e}")))); }
}

/// Points `scheme` at this executable for the user, as an installer would.
#[cfg(target_os = "windows")]
fn write_registry(scheme: &str) -> Result<(), UrlSchemeError> {
    fn failed(what: &str, e: impl std::fmt::Display) -> UrlSchemeError { UrlSchemeError::Failed(format!("{what}: {e}")) }
    let exe = std::env::current_exe().map_err(|e| UrlSchemeError::Failed(format!("current exe: {e}")))?;
    let exe = exe.to_string_lossy();
    let classes = windows_registry::CURRENT_USER.create(format!("Software\\Classes\\{scheme}")).map_err(|e| failed(scheme, e))?;
    classes.set_string("", format!("URL:{scheme}")).map_err(|e| failed(scheme, e))?;
    classes.set_string("URL Protocol", "").map_err(|e| failed("URL Protocol", e))?;
    let set = |path: &str, value: String| classes.create(path).and_then(|k| k.set_string("", value)).map_err(|e| failed(path, e));
    set("DefaultIcon", format!("\"{exe}\",0"))?;
    set("shell\\open\\command", format!("\"{exe}\" \"%1\""))?;
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn register(_: String, reply: Reply) {
    reply.send(Err(UrlSchemeError::Unsupported("this platform has no per-app default URL scheme handler Rune can set".into())));
}

#[cfg(test)]
#[path = "scheme_tests.rs"]
mod tests;
