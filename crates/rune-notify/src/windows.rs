//! Toast notifications (`Windows.UI.Notifications`) for an app without a package: the process
//! takes its `AppUserModelID`, and `HKEY_CURRENT_USER\Software\Classes\AppUserModelId\<id>`
//! names it, so its toasts show the app's name and icon. A click raises the toast's `Activated`
//! event in the process that posted it, which hands it to `on_click`'s handler.
//!
//! With an activation scheme the toast is protocol-activated instead: a click opens
//! `<scheme>://notification/<id>?<user info>` through the shell, which starts the app (or, with
//! `rune::single_instance`, reaches the running one) with the link, whether or not the process
//! that posted it still runs. That is the documented way for an app without a package to be
//! started from its toasts without a COM activator. `click_from_url` reads the link back.

use std::sync::Mutex;

use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{ToastActivatedEventArgs, ToastNotification, ToastNotificationManager};
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::Com::{CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED, CoInitializeEx, CoRegisterClassObject, IClassFactory, IClassFactory_Impl, REGCLS_MULTIPLEUSE};
use windows::Win32::UI::Notifications::{INotificationActivationCallback, INotificationActivationCallback_Impl, NOTIFICATION_USER_INPUT_DATA};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GW_OWNER, GWL_EXSTYLE, GetForegroundWindow, GetWindow, GetWindowLongW, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, HSTRING, IInspectable, Interface, Ref};

use super::{Click, Identity, Notification};

/// Every toast this process posts goes in one group, so `remove` finds it by its tag.
const GROUP: &str = "rune";
/// Toasts kept alive for their click handlers; the oldest goes first.
const KEEP: usize = 64;
/// Windows caps a toast's tag at 64 characters.
const TAG_MAX: usize = 64;

static APP_ID: Mutex<Option<String>> = Mutex::new(None);
/// `Identity::activation`'s scheme, lowercased.
static SCHEME: Mutex<Option<String>> = Mutex::new(None);
/// The host of a notification link.
const HOST: &str = "notification";
/// The user-info key a button's arguments carry the notification's id under.
const ID: &str = "rune.id";

struct Posted {
    id: String,
    tag: String,
    /// Held so its `Activated` handler stays wired.
    _toast: ToastNotification,
}

static POSTED: Mutex<Vec<Posted>> = Mutex::new(Vec::new());

fn app_id() -> Option<String> {
    APP_ID.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub(super) fn registered() -> bool {
    app_id().is_some()
}

pub(super) fn register(identity: &Identity) -> Result<(), String> {
    check_id(&identity.id)?;
    let key = windows_registry::CURRENT_USER
        .create(format!("Software\\Classes\\AppUserModelId\\{}", identity.id))
        .map_err(|e| format!("AppUserModelId key: {e}"))?;
    let name = if identity.name.is_empty() { &identity.id } else { &identity.name };
    key.set_string("DisplayName", name).map_err(|e| format!("DisplayName: {e}"))?;
    match &identity.icon {
        Some(icon) => key.set_string("IconUri", icon.to_string_lossy()).map_err(|e| format!("IconUri: {e}"))?,
        None => {
            _ = key.remove_value("IconUri");
        }
    }
    if let Some(scheme) = &identity.scheme {
        register_scheme(scheme, name)?;
    }
    adopt(identity)
}

/// Takes the identity in this process only: the `AppUserModelID` (set once per id, so a second
/// call with the same id leaves the taskbar alone) and the activation scheme.
pub(super) fn adopt(identity: &Identity) -> Result<(), String> {
    check_id(&identity.id)?;
    let scheme = identity.scheme.as_deref().map(valid_scheme).transpose()?;
    if app_id().as_deref() != Some(identity.id.as_str()) {
        // SAFETY: the HSTRING outlives the call, which copies it.
        unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(&identity.id)) }.map_err(|e| format!("SetCurrentProcessExplicitAppUserModelID: {e}"))?;
    }
    *SCHEME.lock().unwrap_or_else(|e| e.into_inner()) = scheme;
    *APP_ID.lock().unwrap_or_else(|e| e.into_inner()) = Some(identity.id.clone());
    Ok(())
}

fn check_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.contains('\\') {
        return Err(format!("not an app id: {id:?}"));
    }
    Ok(())
}

/// `scheme` lowercased, when it's a URL scheme.
fn valid_scheme(scheme: &str) -> Result<String, String> {
    let valid = scheme.len() >= 2
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !valid {
        return Err(format!("not a URL scheme: {scheme:?}"));
    }
    Ok(scheme.to_ascii_lowercase())
}

/// Points `scheme` at this executable for the user: what the shell runs when a toast's link opens.
fn register_scheme(scheme: &str, name: &str) -> Result<(), String> {
    let scheme = valid_scheme(scheme)?;
    let exe = std::env::current_exe().map_err(|e| format!("current exe: {e}"))?;
    let exe = exe.to_string_lossy();
    let classes = windows_registry::CURRENT_USER.create(format!("Software\\Classes\\{scheme}")).map_err(|e| format!("{scheme} key: {e}"))?;
    classes.set_string("", format!("URL:{name}")).map_err(|e| format!("{scheme}: {e}"))?;
    classes.set_string("URL Protocol", "").map_err(|e| format!("{scheme} URL Protocol: {e}"))?;
    let set = |path: &str, value: String| {
        classes.create(path).and_then(|k| k.set_string("", value)).map_err(|e| format!("{scheme}\\{path}: {e}"))
    };
    set("DefaultIcon", format!("\"{exe}\",0"))?;
    set("shell\\open\\command", format!("\"{exe}\" \"%1\""))?;
    Ok(())
}

/// The scheme's link for `n`: its id in the path, its user info in the query.
fn link(scheme: &str, n: &Notification) -> String {
    format!("{scheme}://{HOST}/{}?{}", percent(&n.id), encode(&n.info))
}

pub(super) fn click_from_url(url: &str) -> Option<Click> {
    let scheme = SCHEME.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    let (head, rest) = url.trim().split_once("://")?;
    let rest = rest.strip_prefix(HOST)?.strip_prefix('/')?;
    if !head.eq_ignore_ascii_case(&scheme) {
        return None;
    }
    let (id, query) = rest.split_once('?').unwrap_or((rest, ""));
    Some(Click { id: unpercent(id.trim_end_matches('/')), info: decode(query) })
}

pub(super) fn post(n: &Notification) -> bool {
    let Some(app) = app_id() else { return false };
    let tag = tag(&n.id);
    match show(&app, n, &tag) {
        Ok(toast) => {
            let mut posted = POSTED.lock().unwrap_or_else(|e| e.into_inner());
            posted.retain(|p| p.tag != tag);
            if posted.len() >= KEEP {
                posted.remove(0);
            }
            posted.push(Posted { id: n.id.clone(), tag, _toast: toast });
            true
        }
        Err(_) => false,
    }
}

fn show(app: &str, n: &Notification, tag: &str) -> windows::core::Result<ToastNotification> {
    let scheme = SCHEME.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml(n, scheme.as_deref())))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    toast.SetTag(&HSTRING::from(tag))?;
    toast.SetGroup(&HSTRING::from(GROUP))?;
    // With a scheme the body's click comes back as a link; a button's still comes here, unless
    // the install's activator takes it (`serve`).
    let links = scheme.is_some();
    toast.Activated(&TypedEventHandler::new(move |sender: Ref<ToastNotification>, args: Ref<IInspectable>| {
        let tag = sender.ok()?.Tag()?.to_string_lossy();
        let args = args.ok()?.cast::<ToastActivatedEventArgs>()?;
        let mut info = decode(&args.Arguments()?.to_string_lossy());
        if links && !info.iter().any(|(k, _)| k == super::ACTION) { return Ok(()); }
        info.extend(typed(&args));
        let id = POSTED.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|p| p.tag == tag).map_or(tag, |p| p.id.clone());
        super::deliver(Click { id, info });
        Ok(())
    }))?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(app))?.Show(&toast)?;
    Ok(toast)
}

/// What the toast's text boxes held, under `rune.input.<id>`.
fn typed(args: &ToastActivatedEventArgs) -> Vec<(String, String)> {
    let Ok(set) = args.UserInput() else { return Vec::new() };
    let Ok(all) = set.First() else { return Vec::new() };
    let mut out = Vec::new();
    while all.HasCurrent().unwrap_or(false) {
        if let Ok(pair) = all.Current()
            && let (Ok(key), Ok(value)) = (pair.Key(), pair.Value())
            && let Ok(text) = value.cast::<windows::Foundation::IPropertyValue>().and_then(|v| v.GetString())
        {
            out.push((format!("{}{}", super::INPUT, key.to_string_lossy()), text.to_string_lossy()));
        }
        if !all.MoveNext().unwrap_or(false) { break; }
    }
    out
}

/// The toast activator the install registered for the app (`CustomActivator` under its
/// `AppUserModelId` key), read once, on the first toast.
fn activator() -> Option<String> {
    static FOUND: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    FOUND.get_or_init(|| {
        let app = app_id()?;
        windows_registry::CURRENT_USER.open(format!("Software\\Classes\\AppUserModelId\\{app}")).ok()?.get_string("CustomActivator").ok().filter(|c| !c.is_empty())
    }).clone()
}

/// The activator's class: each object it makes takes one toast activation.
#[windows::core::implement(IClassFactory)]
struct Factory {
    /// Where a wait (`wait`) takes its one activation; none when they go to the handler.
    once: Option<std::sync::mpsc::Sender<Click>>,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<windows::core::IUnknown>, iid: *const windows::core::GUID, out: *mut *mut std::ffi::c_void) -> windows::core::Result<()> {
        if outer.is_some() { return Err(windows::Win32::Foundation::CLASS_E_NOAGGREGATION.into()); }
        let callback: INotificationActivationCallback = Callback { once: self.once.clone() }.into();
        // SAFETY: COM's out parameter for the interface `iid` names.
        unsafe { callback.query(iid, out).ok() }
    }

    fn LockServer(&self, _: BOOL) -> windows::core::Result<()> { Ok(()) }
}

#[windows::core::implement(INotificationActivationCallback)]
struct Callback {
    once: Option<std::sync::mpsc::Sender<Click>>,
}

impl INotificationActivationCallback_Impl for Callback_Impl {
    fn Activate(&self, _: &windows::core::PCWSTR, args: &windows::core::PCWSTR, data: *const NOTIFICATION_USER_INPUT_DATA, count: u32) -> windows::core::Result<()> {
        // SAFETY: COM hands a NUL-terminated string and `count` key/value pairs, alive for the call.
        let (args, inputs) = unsafe {
            let args = if args.is_null() { String::new() } else { args.to_string().unwrap_or_default() };
            let inputs: Vec<(String, String)> = if data.is_null() { Vec::new() } else {
                std::slice::from_raw_parts(data, count as usize).iter()
                    .map(|d| (format!("{}{}", super::INPUT, d.Key.to_string().unwrap_or_default()), d.Value.to_string().unwrap_or_default()))
                    .collect()
            };
            (args, inputs)
        };
        let mut info = decode(&args);
        info.extend(inputs);
        let id = info.iter().find(|(k, _)| k == ID).map(|(_, v)| v.clone()).unwrap_or_default();
        let click = Click { id, info };
        match &self.once {
            Some(once) => { let _ = once.send(click); }
            None => super::deliver(click),
        }
        Ok(())
    }
}

/// Registers the activator's class `clsid` on a thread of its own, in the multithreaded
/// apartment, so its calls arrive on COM's threads and nothing has to pump messages for them.
pub(super) fn serve(clsid: &str, once: Option<std::sync::mpsc::Sender<Click>>) -> Result<(), String> {
    let clsid = windows::core::GUID::try_from(clsid.trim_matches(|c| c == '{' || c == '}')).map_err(|_| format!("not a CLSID: {clsid}"))?;
    let (ready, done) = std::sync::mpsc::channel();
    std::thread::Builder::new().name("rune-toast-activator".into()).spawn(move || {
        // SAFETY: this thread joins the multithreaded apartment and registers a class object it
        // keeps for the life of the process (or of the wait).
        let registered = unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().and_then(|()| {
                let factory: IClassFactory = Factory { once }.into();
                CoRegisterClassObject(&clsid, &factory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE)
            })
        };
        let _ = ready.send(registered.map(|_| ()).map_err(|e| e.to_string()));
        // The registration lasts while this thread's apartment does: park it.
        loop { std::thread::park(); }
    }).map_err(|e| e.to_string())?;
    done.recv().map_err(|e| e.to_string())?
}

pub(super) fn wait(clsid: &str, timeout: std::time::Duration) -> Option<Click> {
    let (to, from) = std::sync::mpsc::channel();
    serve(clsid, Some(to)).ok()?;
    from.recv_timeout(timeout).ok()
}

pub(super) fn remove(ids: &[&str]) {
    let Some(app) = app_id() else { return };
    let Ok(history) = ToastNotificationManager::History() else { return };
    let mut posted = POSTED.lock().unwrap_or_else(|e| e.into_inner());
    for id in ids {
        let tag = tag(id);
        _ = history.RemoveGroupedTagWithId(&HSTRING::from(&tag), &HSTRING::from(GROUP), &HSTRING::from(&app));
        posted.retain(|p| p.tag != tag);
    }
}

pub(super) fn app_focused() -> bool {
    // SAFETY: plain reads of the foreground window and its owner.
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() {
            return false;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        pid == GetCurrentProcessId()
    }
}

pub(super) fn bring_to_front() {
    let Some(hwnd) = main_window() else { return };
    // SAFETY: `hwnd` is one of this process's top-level windows, read just now.
    unsafe {
        _ = ShowWindow(hwnd, if IsIconic(hwnd).as_bool() { SW_RESTORE } else { SW_SHOW });
        if SetForegroundWindow(hwnd).as_bool() {
            return;
        }
        // Windows keeps the foreground from a process that doesn't hold it; joining the
        // foreground window's input queue for the call lets a clicked toast's app come forward.
        let them = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let us = GetCurrentThreadId();
        if them != 0 && them != us && AttachThreadInput(us, them, true).as_bool() {
            _ = BringWindowToTop(hwnd);
            _ = SetForegroundWindow(hwnd);
            _ = AttachThreadInput(us, them, false);
        }
    }
}

/// This process's frontmost visible, unowned app window (not a tool window).
fn main_window() -> Option<HWND> {
    unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> BOOL {
        // SAFETY: `found` points at the `Option<HWND>` `main_window` passed, alive for the walk.
        unsafe {
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let tool = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0;
            let owned = GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.is_invalid());
            if pid == GetCurrentProcessId() && IsWindowVisible(hwnd).as_bool() && !tool && !owned {
                *(found.0 as *mut Option<HWND>) = Some(hwnd);
                return BOOL(0);
            }
            BOOL(1)
        }
    }
    let mut found: Option<HWND> = None;
    // SAFETY: the callback only writes `found`, which outlives the enumeration. EnumWindows walks
    // the top-level windows front to back, so the first match is the frontmost.
    _ = unsafe { EnumWindows(Some(each), LPARAM(&mut found as *mut Option<HWND> as isize)) };
    found
}

/// A tag Windows accepts for `id`: the id itself, or a hash of a longer one.
fn tag(id: &str) -> String {
    if id.chars().count() <= TAG_MAX {
        return id.to_owned();
    }
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// The toast's XML: a generic toast with the title and body, its buttons and text boxes, and no
/// sound. Its launch arguments are the user info, or with an activation scheme the link a click
/// opens. A button's arguments are the user info with its action and the notification's id;
/// it's a background activation when the install registered the app's activator (it reaches
/// `serve`, or starts the app for `wait`), else a foreground one the toast's own event takes in
/// the running app, which doesn't bring itself forward for it.
pub(super) fn xml(n: &Notification, scheme: Option<&str>) -> String {
    let body = if n.body.is_empty() { String::new() } else { format!("<text>{}</text>", escape(&n.body)) };
    let launch = match scheme {
        Some(scheme) => format!("activationType=\"protocol\" launch=\"{}\"", escape(&link(scheme, n))),
        None => format!("launch=\"{}\"", escape(&encode(&n.info))),
    };
    let activation = if activator().is_some() { "background" } else { "foreground" };
    let arguments = |action: &str| {
        let mut info = n.info.clone();
        info.push((super::ACTION.to_owned(), action.to_owned()));
        info.push((ID.to_owned(), n.id.clone()));
        escape(&encode(&info))
    };
    let actions: String = n.actions.iter().take(5).map(|a| match a {
        super::Action::Button { id, label } => format!("<action content=\"{}\" arguments=\"{}\" activationType=\"{activation}\"/>", escape(label), arguments(id)),
        super::Action::Reply { id, placeholder, label } => format!(
            "<input id=\"{0}\" type=\"text\" placeHolderContent=\"{1}\"/><action content=\"{2}\" arguments=\"{3}\" activationType=\"{activation}\" hint-inputId=\"{0}\"/>",
            escape(id), escape(placeholder), escape(label), arguments(id),
        ),
    }).collect();
    // Inputs come before buttons in the schema.
    let (inputs, buttons): (Vec<&str>, Vec<&str>) = actions.split_inclusive("/>").partition(|part| part.starts_with("<input"));
    let actions = if actions.is_empty() { String::new() } else { format!("<actions>{}{}</actions>", inputs.concat(), buttons.concat()) };
    format!(
        "<toast {launch}><visual><binding template=\"ToastGeneric\"><text>{}</text>{body}</binding></visual>{actions}<audio silent=\"true\"/></toast>",
        escape(&n.title),
    )
}

fn escape(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .fold(String::with_capacity(s.len()), |mut out, c| {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&apos;"),
                c => out.push(c),
            }
            out
        })
}

/// User info as `key=value&key=value`, each part percent-encoded.
pub(super) fn encode(info: &[(String, String)]) -> String {
    info.iter().map(|(k, v)| format!("{}={}", percent(k), percent(v))).collect::<Vec<_>>().join("&")
}

pub(super) fn decode(args: &str) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = args
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (unpercent(k), unpercent(v))
        })
        .collect();
    pairs.sort();
    pairs
}

fn percent(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn unpercent(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%' && i + 2 < b.len()).then(|| std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())).flatten();
        match hex {
            Some(v) => {
                out.push(v);
                i += 3;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
