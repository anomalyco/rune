//! Starting the app again after it quits (`Tx::relaunch`, `AppHandle::relaunch`), as Electron's
//! `app.relaunch()` does. Once the event loop has ended for a relaunch (`crate::quit`), this
//! process starts its own executable again with its arguments and working folder, and the new
//! process waits for this one to exit before it goes on ([`wait_for_previous`], from
//! [`single_instance`](crate::single_instance) and the event loop's start), so it finds the
//! app's single-instance claim, files and windows free, as Electron's relauncher waits for the
//! old process. macOS starts an app bundle again through Launch Services (`open -n`), as the
//! Finder does, so it comes forward as a launch does.

/// The variable that tells a relaunched process the pid it waits for.
const AFTER: &str = "RUNE_RELAUNCH_AFTER";
/// The variable that tells a launch handed on by a quitting process (`spawn_with`) that process's
/// pid: it claims the instance once that one lets go of it, not once it has exited.
#[cfg(any(target_os = "windows", target_os = "linux"))]
const HANDED: &str = "RUNE_INSTANCE_HANDED";
/// The longest a relaunched process waits for the previous one.
const WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Starts this app again, to go on once this process has exited. Errors are logged: the app
/// is quitting either way.
pub(crate) fn spawn() {
    #[cfg(not(target_arch = "wasm32"))]
    if let Err(e) = start(std::env::args_os().skip(1).collect(), AFTER) {
        eprintln!("rune: couldn't start the app again: {e}");
    }
}

/// Starts this app with another launch's arguments: a launch that reached this process while it
/// quits (`single_instance`). The new process claims the instance as soon as this one lets go of
/// it (`release_instance`), and starts meanwhile.
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) fn spawn_with(args: Vec<String>) {
    if let Err(e) = start(args.into_iter().map(Into::into).collect(), HANDED) {
        eprintln!("rune: couldn't start the app for a launch that came while it quit: {e}");
    }
}

/// The quitting process that handed this launch on (`spawn_with`), once.
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) fn handed_from() -> Option<u32> {
    std::env::var_os(HANDED).and_then(|v| v.to_str().and_then(|v| v.parse::<u32>().ok())).filter(|pid| *pid != std::process::id())
}

#[cfg(not(target_arch = "wasm32"))]
fn start(args: Vec<std::ffi::OsString>, wait_var: &str) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let exe = std::env::current_exe()?;
    let pid = std::process::id().to_string();
    #[cfg(target_os = "macos")]
    if let Some(bundle) = bundle_of(&exe) {
        let status = Command::new("/usr/bin/open").arg("-n").arg(&bundle).arg("--env").arg(format!("{wait_var}={pid}")).arg("--args").args(&args).status()?;
        return if status.success() { Ok(()) } else { Err(std::io::Error::other(format!("open -n exited with {status}"))) };
    }
    let mut cmd = Command::new(&exe);
    // Only this launch's wait goes to the new process, not one this process was started with.
    cmd.args(&args).env_remove(AFTER);
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    cmd.env_remove(HANDED);
    cmd.env(wait_var, &pid).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Ok(dir) = std::env::current_dir() { cmd.current_dir(dir); }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        // Out of any job this process runs in, so the new one outlives it; a job that refuses
        // the breakaway still starts it inside.
        let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        if cmd.creation_flags(flags | CREATE_BREAKAWAY_FROM_JOB).spawn().is_ok() { return Ok(()); }
        cmd.creation_flags(flags);
    }
    cmd.spawn().map(drop)
}

/// The `.app` an executable in `Contents/MacOS` belongs to.
#[cfg(target_os = "macos")]
fn bundle_of(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let macos = exe.parent().filter(|p| p.file_name().is_some_and(|n| n == "MacOS"))?;
    let bundle = macos.parent().filter(|p| p.file_name().is_some_and(|n| n == "Contents"))?.parent()?;
    bundle.extension().is_some_and(|e| e == "app").then(|| bundle.to_path_buf())
}

/// A relaunched process waits here, once, for the process that started it to exit, for at most
/// ten seconds. Any other launch goes straight on.
pub(crate) fn wait_for_previous() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let Some(pid) = std::env::var_os(AFTER).and_then(|v| v.to_str().and_then(|v| v.parse::<u32>().ok())) else { return };
        if pid != std::process::id() { wait(pid); }
    });
}

#[cfg(target_os = "windows")]
fn wait(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    // SAFETY: a plain handle request; a process that already exited answers null. The handle is
    // closed once.
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() { return; }
        WaitForSingleObject(handle, WAIT.as_millis() as u32);
        CloseHandle(handle);
    }
}

/// Whether process `pid` still runs: it hasn't exited, and it isn't ending (a process that was
/// terminated has its exit code at once, while its last threads and kernel objects can linger
/// for seconds).
#[cfg(target_os = "windows")]
pub(crate) fn alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    // SAFETY: a plain handle request; a process that already exited answers null. The handle is
    // closed once; the exit code is written into `code`.
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0;
        let running = WaitForSingleObject(handle, 0) == WAIT_TIMEOUT && GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE as u32;
        CloseHandle(handle);
        running
    }
}

#[cfg(not(target_os = "windows"))]
fn wait(pid: u32) {
    let deadline = std::time::Instant::now() + WAIT;
    while alive(pid) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(target_os = "linux")]
pub(crate) fn alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(not(any(unix, target_os = "windows")))]
fn alive(_pid: u32) -> bool {
    false
}
