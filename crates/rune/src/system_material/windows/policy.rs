//! Event-driven Windows system effects policy watcher for `El::system_material`.
//!
//! Lazily initialized on the first frame that mounts an `El::system_material` element (zero
//! startup cost, zero DLL loads, and zero polling for apps without materials). Subscribes to
//! documented WinRT and Win32 notifications so Rune tears down host-backdrop visuals and paints
//! fallback fills whenever DWM will not render `HostBackdropBrush`:
//! - `Windows.UI.ViewManagement.UISettings::AdvancedEffectsEnabled` +
//!   `AdvancedEffectsEnabledChanged` (Settings -> Personalization -> Colors / Accessibility ->
//!   Visual effects -> Transparency effects; also refreshed on `WM_SETTINGCHANGE`);
//! - `PowerSettingRegisterNotification` (`powrprof.dll`, `DEVICE_NOTIFY_CALLBACK`) for both
//!   `GUID_POWER_SAVING_STATUS` (legacy Battery Saver) and `GUID_ENERGY_SAVER_STATUS`
//!   (Windows 11 24H2+ Standard and High Savings Energy Saver), seeded from `GetSystemPowerStatus`;
//! - `Windows.UI.Composition.CompositionCapabilities::GetForCurrentView`
//!   (`AreEffectsSupported` + `AreEffectsFast` + `Changed`, when available on the thread);
//! - High Contrast (`SystemParametersInfoW(SPI_GETHIGHCONTRAST)` via `preferences::contrast`);
//! - Remote Desktop (`GetSystemMetrics(SM_REMOTESESSION)`).

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, Once, OnceLock};

use windows::Foundation::TypedEventHandler;
use windows::UI::Composition::CompositionCapabilities;
use windows::UI::ViewManagement::UISettings;
use windows::core::{GUID, IInspectable};

use super::{
    EffectsPolicy, GUID_ENERGY_SAVER_STATUS_PARTS, GUID_POWER_SAVING_STATUS_PARTS,
    PowerSettingUpdate, parse_power_setting,
};

/// `DEVICE_NOTIFY_CALLBACK` (`winuser.h`): deliver power notifications via callback.
const DEVICE_NOTIFY_CALLBACK: u32 = 2;
/// `PBT_POWERSETTINGCHANGE` (`winuser.h`): power setting payload in `setting`.
const PBT_POWERSETTINGCHANGE: u32 = 0x8013;

static STARTED: AtomicBool = AtomicBool::new(false);
static INIT: Once = Once::new();
static ADVANCED_EFFECTS: AtomicBool = AtomicBool::new(true);
static POWER_SAVING_STATUS: AtomicBool = AtomicBool::new(false);
static ENERGY_SAVER_STATUS: AtomicBool = AtomicBool::new(false);
static COMPOSITION_EFFECTS: AtomicBool = AtomicBool::new(true);
static HIGH_CONTRAST: AtomicBool = AtomicBool::new(false);
static REMOTE_SESSION: AtomicBool = AtomicBool::new(false);
static LAST_PUBLISHED: AtomicU8 = AtomicU8::new(u8::MAX);
static WAKER: Mutex<Option<crate::Waker>> = Mutex::new(None);

/// Holds live WinRT / Win32 notification subscriptions for the lifetime of the process so
/// callbacks can never outlive their sources.
struct Subscriptions {
    _ui_settings: Option<UISettings>,
    _capabilities: Option<CompositionCapabilities>,
    _power_saving_handle: usize,
    _energy_saver_handle: usize,
}

static SUBSCRIPTIONS: OnceLock<Subscriptions> = OnceLock::new();

#[repr(C)]
struct SystemPowerStatus {
    ac_line_status: u8,
    battery_flag: u8,
    battery_life_percent: u8,
    system_status_flag: u8,
    battery_life_time: u32,
    battery_full_life_time: u32,
}

type DeviceNotifyCallback = unsafe extern "system" fn(
    context: *mut c_void,
    event_type: u32,
    setting: *mut c_void,
) -> u32;

#[repr(C)]
struct DeviceNotifySubscribeParameters {
    callback: DeviceNotifyCallback,
    context: *mut c_void,
}

// SAFETY: `POWER_PARAMS` contains only a function pointer and a null context pointer.
unsafe impl Sync for DeviceNotifySubscribeParameters {}

static POWER_PARAMS: DeviceNotifySubscribeParameters = DeviceNotifySubscribeParameters {
    callback: power_setting_callback,
    context: std::ptr::null_mut(),
};

fn guid_from_parts((d1, d2, d3, d4): (u32, u16, u16, [u8; 8])) -> GUID {
    GUID::from_values(d1, d2, d3, d4)
}

/// Registers the event-loop waker that delivers `WakeEvent::MaterialPolicyChanged` to the UI
/// thread when any Windows effects policy notification fires. Performs zero OS calls.
pub(crate) fn set_policy_waker(waker: crate::Waker) {
    *WAKER.lock().unwrap_or_else(|e| e.into_inner()) = Some(waker);
}

/// Returns the current Windows effects policy, lazily starting event subscriptions on first call.
pub(crate) fn current_policy() -> EffectsPolicy {
    ensure_started();
    snapshot()
}

/// Refreshes settings that can arrive through Win32 broadcasts (`WM_SETTINGCHANGE`,
/// `WM_SYSCOLORCHANGE`, `WM_THEMECHANGED`, `WM_WTSSESSION_CHANGE`), including direct
/// `EnableTransparency` registry updates accompanied by `WM_SETTINGCHANGE`.
///
/// Instant no-op when the watcher has never been started (apps with no `El::system_material`).
pub(crate) fn notify_wm_setting_change() {
    if !STARTED.load(Ordering::Acquire) {
        return;
    }
    if let Ok(settings) = UISettings::new()
        && let Ok(enabled) = settings.AdvancedEffectsEnabled()
    {
        ADVANCED_EFFECTS.store(enabled, Ordering::Release);
    }
    HIGH_CONTRAST.store(crate::platform::preferences::contrast().is_some(), Ordering::Release);
    REMOTE_SESSION.store(is_remote_session(), Ordering::Release);
    maybe_wake();
}

fn snapshot() -> EffectsPolicy {
    EffectsPolicy {
        advanced_effects: ADVANCED_EFFECTS.load(Ordering::Acquire),
        energy_saver: POWER_SAVING_STATUS.load(Ordering::Acquire)
            || ENERGY_SAVER_STATUS.load(Ordering::Acquire),
        composition_effects: COMPOSITION_EFFECTS.load(Ordering::Acquire),
        high_contrast: HIGH_CONTRAST.load(Ordering::Acquire),
        remote_session: REMOTE_SESSION.load(Ordering::Acquire),
    }
}

fn maybe_wake() {
    if !STARTED.load(Ordering::Acquire) {
        return;
    }
    let next = snapshot().bits();
    let prev = LAST_PUBLISHED.swap(next, Ordering::AcqRel);
    if prev != next && prev != u8::MAX {
        let waker = WAKER.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(wake) = waker {
            wake();
        }
    }
}

fn ensure_started() {
    if STARTED.load(Ordering::Acquire) {
        return;
    }
    INIT.call_once(|| {
        HIGH_CONTRAST.store(crate::platform::preferences::contrast().is_some(), Ordering::Release);
        REMOTE_SESSION.store(is_remote_session(), Ordering::Release);
        if let Some(saving) = query_system_power_saving() {
            POWER_SAVING_STATUS.store(saving, Ordering::Release);
        }

        let ui_settings = init_ui_settings();
        let capabilities = init_composition_capabilities();
        let (power_saving_handle, energy_saver_handle) = register_power_notifications();

        // Initialized inside Once::call_once, so set() always succeeds on the single init path.
        SUBSCRIPTIONS
            .set(Subscriptions {
                _ui_settings: ui_settings,
                _capabilities: capabilities,
                _power_saving_handle: power_saving_handle,
                _energy_saver_handle: energy_saver_handle,
            })
            .ok();

        LAST_PUBLISHED.store(snapshot().bits(), Ordering::Release);
        STARTED.store(true, Ordering::Release);
    });
}

fn init_ui_settings() -> Option<UISettings> {
    let settings = UISettings::new().ok()?;
    if let Ok(enabled) = settings.AdvancedEffectsEnabled() {
        ADVANCED_EFFECTS.store(enabled, Ordering::Release);
    }
    let handler = TypedEventHandler::<UISettings, IInspectable>::new(|sender, _| {
        if let Some(s) = sender.as_ref()
            && let Ok(enabled) = s.AdvancedEffectsEnabled()
        {
            ADVANCED_EFFECTS.store(enabled, Ordering::Release);
            maybe_wake();
        }
        Ok(())
    });
    // Best-effort subscription: even if WinRT event registration declines on an older host,
    // initial read and WM_SETTINGCHANGE refreshes still keep AdvancedEffectsEnabled current.
    settings.AdvancedEffectsEnabledChanged(&handler).ok();
    Some(settings)
}

fn init_composition_capabilities() -> Option<CompositionCapabilities> {
    let caps = CompositionCapabilities::GetForCurrentView().ok()?;
    let supported = caps.AreEffectsSupported().unwrap_or(true);
    let fast = caps.AreEffectsFast().unwrap_or(true);
    COMPOSITION_EFFECTS.store(supported && fast, Ordering::Release);
    let handler = TypedEventHandler::<CompositionCapabilities, IInspectable>::new(|sender, _| {
        if let Some(c) = sender.as_ref() {
            let supported = c.AreEffectsSupported().unwrap_or(true);
            let fast = c.AreEffectsFast().unwrap_or(true);
            COMPOSITION_EFFECTS.store(supported && fast, Ordering::Release);
            maybe_wake();
        }
        Ok(())
    });
    // Best-effort subscription: initial capability read already ran above.
    caps.Changed(&handler).ok();
    Some(caps)
}

fn is_remote_session() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_REMOTESESSION};
    // SAFETY: GetSystemMetrics is a pure Win32 query taking a documented metric constant.
    unsafe { GetSystemMetrics(SM_REMOTESESSION) != 0 }
}

fn query_system_power_saving() -> Option<bool> {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    type GetSystemPowerStatusFn = unsafe extern "system" fn(*mut SystemPowerStatus) -> i32;
    // SAFETY: kernel32.dll is loaded in every Win32 process; GetSystemPowerStatus fills the
    // documented 12-byte SYSTEM_POWER_STATUS struct (`winbase.h`).
    unsafe {
        let kernel32 = GetModuleHandleW(windows_sys::core::w!("kernel32.dll"));
        if kernel32.is_null() {
            return None;
        }
        let proc = GetProcAddress(kernel32, c"GetSystemPowerStatus".as_ptr().cast())?;
        let get_status: GetSystemPowerStatusFn = std::mem::transmute(proc);
        let mut status = SystemPowerStatus {
            ac_line_status: 0,
            battery_flag: 0,
            battery_life_percent: 0,
            system_status_flag: 0,
            battery_life_time: 0,
            battery_full_life_time: 0,
        };
        (get_status(&mut status) != 0).then_some(status.system_status_flag != 0)
    }
}

fn register_power_notifications() -> (usize, usize) {
    type RegisterFn = unsafe extern "system" fn(
        setting_guid: *const GUID,
        flags: u32,
        recipient: *const c_void,
        registration_handle: *mut *mut c_void,
    ) -> u32;
    // SAFETY: loads powrprof.dll exclusively from System32 and calls documented
    // PowerSettingRegisterNotification (`powersetting.h`) with a static callback block.
    unsafe {
        let lib = crate::platform::win32::load_system_dll("powrprof.dll");
        if lib.is_null() {
            return (0, 0);
        }
        let Some(proc) = windows_sys::Win32::System::LibraryLoader::GetProcAddress(
            lib,
            c"PowerSettingRegisterNotification".as_ptr().cast(),
        ) else {
            return (0, 0);
        };
        let register: RegisterFn = std::mem::transmute(proc);
        let register_guid = |parts: (u32, u16, u16, [u8; 8])| -> usize {
            let guid = guid_from_parts(parts);
            let mut handle: *mut c_void = std::ptr::null_mut();
            let err = register(
                &guid,
                DEVICE_NOTIFY_CALLBACK,
                (&raw const POWER_PARAMS).cast(),
                &mut handle,
            );
            if err == 0 { handle as usize } else { 0 }
        };
        let power_saving = register_guid(GUID_POWER_SAVING_STATUS_PARTS);
        let energy_saver = register_guid(GUID_ENERGY_SAVER_STATUS_PARTS);
        (power_saving, energy_saver)
    }
}

unsafe extern "system" fn power_setting_callback(
    _context: *mut c_void,
    event_type: u32,
    setting: *mut c_void,
) -> u32 {
    if event_type != PBT_POWERSETTINGCHANGE || setting.is_null() {
        return 0;
    }
    // SAFETY: on PBT_POWERSETTINGCHANGE, `setting` points to a valid `POWERBROADCAST_SETTING`
    // (`GUID` at offset 0, `DWORD DataLength` at offset 16, `UCHAR Data[DataLength]` at offset 20).
    let (guid, data_len) = unsafe {
        let base = setting.cast::<u8>();
        let guid = std::ptr::read_unaligned(base.cast::<GUID>());
        let data_len = std::ptr::read_unaligned(base.add(size_of::<GUID>()).cast::<u32>()) as usize;
        (guid, data_len)
    };
    if data_len < 4 {
        return 0;
    }
    // SAFETY: `data_len >= 4`, so 4 bytes at offset 20 are initialized by Windows.
    let data = unsafe {
        let ptr = setting.cast::<u8>().add(size_of::<GUID>() + size_of::<u32>());
        std::ptr::read_unaligned(ptr.cast::<[u8; 4]>())
    };
    let parts = (guid.data1, guid.data2, guid.data3, guid.data4);
    match parse_power_setting(parts, &data) {
        Some(PowerSettingUpdate::PowerSaving(on)) => {
            POWER_SAVING_STATUS.store(on, Ordering::Release);
            maybe_wake();
        }
        Some(PowerSettingUpdate::EnergySaver(on)) => {
            ENERGY_SAVER_STATUS.store(on, Ordering::Release);
            maybe_wake();
        }
        None => {}
    }
    0
}
