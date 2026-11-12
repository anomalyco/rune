//! The taskbar's jump list (`crate::jump_list`), through the shell's `ICustomDestinationList`, on
//! a thread of its own: made on the first list, asleep on its channel in between, its own COM
//! apartment. Each entry is a shell link to this executable with the entry's argument.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::UI::Shell::Common::{IObjectArray, IObjectCollection};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{DestinationList, EnumerableObjectCollection, ICustomDestinationList, IShellLinkW, ShellLink};
use windows::core::{HSTRING, Interface};

use crate::jump_list::{JumpItem, JumpList};

/// `PKEY_Title`: an entry's title (a shell link's own name is its target's).
const TITLE: PROPERTYKEY = PROPERTYKEY { fmtid: windows::core::GUID::from_u128(0xf29f85e0_4ff9_1068_ab91_08002b27b3d9), pid: 2 };

struct Worker {
    to: Sender<JumpList>,
    last: Option<JumpList>,
}

static WORKER: OnceLock<Mutex<Worker>> = OnceLock::new();

pub(crate) fn set(list: JumpList) {
    let worker = WORKER.get_or_init(|| {
        let (to, from) = channel();
        let spawned = std::thread::Builder::new().name("rune-jump-list".into()).spawn(move || run(from));
        if let Err(e) = spawned { eprintln!("rune: jump list: {e}"); }
        Mutex::new(Worker { to, last: None })
    });
    let mut w = worker.lock().unwrap_or_else(|e| e.into_inner());
    if w.last.as_ref() == Some(&list) { return; }
    w.last = Some(list.clone());
    let _ = w.to.send(list);
}

fn run(from: Receiver<JumpList>) {
    // SAFETY: this thread's own single-threaded apartment, for the shell's objects.
    if unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_err() { return; }
    let Some(exe) = std::env::current_exe().ok() else { return };
    let exe = HSTRING::from(exe.as_os_str());
    while let Ok(mut list) = from.recv() {
        // Only the newest of lists sent meanwhile.
        while let Ok(newer) = from.try_recv() { list = newer; }
        let started = std::time::Instant::now();
        // SAFETY: shell COM calls in this thread's apartment.
        match unsafe { commit(&list, &exe) } {
            Ok(n) => eprintln!("rune: jump list: {n} entries committed in {} µs off the UI thread", started.elapsed().as_micros()),
            Err(e) => eprintln!("rune: jump list: {e}"),
        }
    }
}

unsafe fn commit(list: &JumpList, exe: &HSTRING) -> windows::core::Result<usize> {
    // SAFETY: shell COM calls in this thread's apartment, on objects made here.
    unsafe {
        let dest: ICustomDestinationList = CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER)?;
        let mut slots = 0u32;
        let removed: IObjectArray = dest.BeginList(&mut slots)?;
        let removed = removed_arguments(&removed);
        let mut n = 0;
        if !list.tasks.is_empty() {
            let tasks = collection(&list.tasks, list.lead.as_deref(), exe, &HashSet::new())?;
            dest.AddUserTasks(&tasks)?;
            n += list.tasks.len();
        }
        for (name, items) in &list.categories {
            let kept: Vec<&JumpItem> = items.iter().filter(|i| !removed.contains(&i.argument)).collect();
            if kept.is_empty() { continue; }
            let entries = collection(items, list.lead.as_deref(), exe, &removed)?;
            // A category the person removed entries from, or that policy hides, is skipped.
            if dest.AppendCategory(&HSTRING::from(name.as_str()), &entries).is_ok() { n += kept.len(); }
        }
        dest.CommitList()?;
        Ok(n)
    }
}

/// The arguments of the entries the person removed: Windows refuses a list that adds them back.
unsafe fn removed_arguments(removed: &IObjectArray) -> HashSet<String> {
    // SAFETY: reads the shell's array of links.
    unsafe {
        let count = removed.GetCount().unwrap_or(0);
        (0..count).filter_map(|i| {
            let link: IShellLinkW = removed.GetAt(i).ok()?;
            let mut buf = [0u16; 2048];
            link.GetArguments(&mut buf).ok()?;
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(last_argument(&String::from_utf16_lossy(&buf[..len])))
        }).collect()
    }
}

unsafe fn collection(items: &[JumpItem], lead: Option<&str>, exe: &HSTRING, removed: &HashSet<String>) -> windows::core::Result<IObjectArray> {
    // SAFETY: shell COM calls on objects made here.
    unsafe {
        let all: IObjectCollection = CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)?;
        for item in items.iter().filter(|i| !removed.contains(&i.argument)) {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(exe)?;
            let arguments = match lead { Some(lead) => format!("\"{lead}\" \"{}\"", item.argument), None => format!("\"{}\"", item.argument) };
            link.SetArguments(&HSTRING::from(arguments))?;
            link.SetIconLocation(exe, 0)?;
            if !item.description.is_empty() { link.SetDescription(&HSTRING::from(item.description.as_str()))?; }
            let props: IPropertyStore = link.cast()?;
            props.SetValue(&TITLE, &PROPVARIANT::from(item.title.as_str()))?;
            props.Commit()?;
            all.AddObject(&link)?;
        }
        all.cast()
    }
}

/// An entry's own argument: the last one on its link's command line, after any `lead`.
fn last_argument(s: &str) -> String {
    let s = s.trim();
    match s.strip_suffix('"').and_then(|rest| rest.rfind('"').map(|at| &rest[at + 1..])) {
        Some(quoted) => quoted.to_owned(),
        None => s.rsplit(' ').next().unwrap_or_default().to_owned(),
    }
}
