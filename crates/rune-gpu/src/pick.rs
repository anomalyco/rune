//! Choosing the GPU adapter. On a machine with two GPUs, the one a Wayland compositor renders
//! and scans out with is the one whose buffers it can show: Hyprland on an NVIDIA card shows an
//! integrated GPU's buffers as black rectangles. So Rune asks the compositor for its main device
//! (`zwp_linux_dmabuf_v1` feedback) and takes the adapter on the same PCI device. A laptop that
//! drives its panel from the integrated GPU and offloads to the discrete one gets the integrated
//! GPU, as before.
//!
//! On Windows, every device takes the adapter that drives the primary display (the one with the
//! taskbar), else the next display's: windows, overlays, offscreen renders and tests alike.
//! Offscreen work for an existing window (snapshots, drag image, video) uses that window's own
//! device, whichever display it is on. Only when no display's GPU has an adapter (a headless
//! server, a remote session, a VM) does Rune take the lowest-power one. Offscreen work used to
//! take the lowest-power adapter: on a desktop that is often an integrated GPU no monitor uses,
//! far slower than the discrete card beside it, where a test's single submission ran past
//! Windows' 2-second limit and reset it. On a hybrid laptop whose panel is on the integrated GPU,
//! that GPU drives the primary display, so nothing changes there and the discrete GPU stays
//! asleep. On a hybrid laptop whose monitors hang off the discrete GPU, that's the discrete GPU,
//! for the main window and the HUD alike: an AMD integrated GPU presenting the HUD across to
//! them grew the process by gigabytes over an hour of dictation (hex#1268). A swapchain on a GPU
//! that drives none (a desktop's unused integrated GPU beside the discrete card the monitors
//! hang off) is presented across adapters: each one costs the display GPU's driver a device of
//! its own (about 37 threads with the NVIDIA driver, one of them waking 64 times a second) and
//! about 100 MB per full-screen surface on each GPU, and takes 50 to 160 ms to make instead of
//! 6 ms.
//!
//! `WGPU_ADAPTER_NAME` (a case-insensitive part of the adapter's name, software adapters
//! included: `Microsoft Basic Render Driver` is WARP) and `WGPU_POWER_PREF` (`low`, `high` or
//! `none`) override the choice. A named adapter is picked or the pick fails with an error: Rune
//! never falls through to another GPU, and on Windows DX12 finds it from DXGI without making a
//! device on any other adapter. A replacement device after a loss follows the same rule: the
//! named adapter (or error), else the GPU a window would get, never falling back to lowest power
//! when a display GPU exists. Without overrides, a compositor device or a display's GPU, Rune
//! takes the lowest-power adapter. `RUNE_GPU_LOG=1` prints every pick in a process with its
//! reason; without it, a machine with more than one GPU still prints the first once per process.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    EnvName,
    EnvPower,
    Compositor,
    Display,
    /// Offscreen or test work, on the GPU driving the primary display.
    WindowGpu,
    LowPower,
}

impl Reason {
    fn describe(self) -> &'static str {
        match self {
            Reason::EnvName => "WGPU_ADAPTER_NAME",
            Reason::EnvPower => "WGPU_POWER_PREF",
            Reason::Compositor => "the compositor's GPU",
            Reason::Display => "the GPU driving the primary display",
            Reason::WindowGpu => "the GPU a window would get (it drives the primary display)",
            Reason::LowPower => "lowest power",
        }
    }
}

pub struct Pick {
    pub adapter: wgpu::Adapter,
    pub reason: Reason,
    /// The compositor's main device as a PCI address, when it told us one.
    compositor: Option<String>,
}

impl Pick {
    /// The user chose the adapter; nothing else should swap it.
    pub fn from_env(&self) -> bool {
        matches!(self.reason, Reason::EnvName | Reason::EnvPower)
    }

    /// Whether `info` is an adapter on the compositor's device.
    pub fn on_compositor_device(&self, info: &wgpu::AdapterInfo) -> bool {
        self.compositor.as_deref().is_some_and(|pci| same_pci(pci, &info.device_pci_bus_id))
    }

    /// Whether the compositor can show `info`'s buffers: any adapter's when it named no device,
    /// otherwise only its own device's.
    pub fn shows(&self, info: &wgpu::AdapterInfo) -> bool {
        shows(self.compositor.as_deref(), &info.device_pci_bus_id)
    }
}

/// `Pick::shows` for an adapter on PCI device `pci`.
fn shows(compositor: Option<&str>, pci: &str) -> bool {
    compositor.is_none_or(|compositor| same_pci(compositor, pci))
}

/// The adapter for `surface` (or for offscreen work without one, unless it's for a surface not
/// made yet: `presented`), by the order in the module docs.
pub fn pick(instance: &wgpu::Instance, surface: Option<&wgpu::Surface<'_>>, presented: bool) -> Result<Pick, String> {
    // DX12: the same order through DXGI's adapter list, and only the chosen adapter goes to wgpu.
    #[cfg(target_os = "windows")]
    if let Some(picked) = dxgi::pick(instance, surface, presented) {
        let (pick, adapters) = picked.inspect_err(|error| log_failure(error))?;
        return Ok(log(pick, Some(adapters)));
    }
    let usable = |adapter: &wgpu::Adapter| surface.is_none_or(|s| adapter.is_surface_supported(s));
    let compositor = compositor_pci();
    let mut enumerated = None;
    if let Some(name) = rune_core::env::var("WGPU_ADAPTER_NAME") {
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        let infos: Vec<_> = adapters.iter().map(|a| (a.get_info(), usable(a))).collect();
        let found = by_name(&name, &infos);
        enumerated = Some(infos.len());
        let Some(adapter) = found.and_then(|index| adapters.into_iter().nth(index)) else {
            let error = format!("no GPU adapter matches WGPU_ADAPTER_NAME={name:?}");
            log_failure(&error);
            return Err(error);
        };
        return Ok(log(Pick { adapter, reason: Reason::EnvName, compositor }, enumerated));
    }
    if let Some(power) = wgpu::PowerPreference::from_env() {
        let adapter = request(instance, surface, power)?;
        return Ok(log(Pick { adapter, reason: Reason::EnvPower, compositor }, enumerated));
    }
    if let Some(pci) = compositor.as_deref() {
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        let infos: Vec<_> = adapters.iter().map(|a| (a.get_info(), usable(a))).collect();
        enumerated = Some(infos.iter().filter(|(info, _)| info.device_type != wgpu::DeviceType::Cpu).count());
        if let Some(adapter) = by_pci(pci, &infos).and_then(|index| adapters.into_iter().nth(index)) {
            return Ok(log(Pick { adapter, reason: Reason::Compositor, compositor }, enumerated));
        }
    }
    // Every device takes the GPU a window would get; `presented` only names the reason.
    let displays = display_devices();
    if !displays.is_empty() {
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        let infos: Vec<_> = adapters.iter().map(|a| (a.get_info(), usable(a))).collect();
        enumerated = Some(infos.iter().filter(|(info, _)| info.device_type != wgpu::DeviceType::Cpu).count());
        if let Some(adapter) = by_display(&displays, &infos).and_then(|index| adapters.into_iter().nth(index)) {
            let reason = if surface.is_some() || presented { Reason::Display } else { Reason::WindowGpu };
            return Ok(log(Pick { adapter, reason, compositor }, enumerated));
        }
    }
    let adapter = request(instance, surface, wgpu::PowerPreference::LowPower)?;
    Ok(log(Pick { adapter, reason: Reason::LowPower, compositor }, enumerated))
}

fn request(instance: &wgpu::Instance, surface: Option<&wgpu::Surface<'_>>, power_preference: wgpu::PowerPreference) -> Result<wgpu::Adapter, String> {
    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference,
        force_fallback_adapter: false,
        compatible_surface: surface,
        apply_limit_buckets: false,
    }))
    .map_err(|error| format!("no GPU adapter: {error}"))
}

/// The first usable adapter whose name contains `name`, ignoring case.
fn by_name(name: &str, adapters: &[(wgpu::AdapterInfo, bool)]) -> Option<usize> {
    let name = name.to_lowercase();
    adapters.iter().position(|(info, usable)| *usable && info.name.to_lowercase().contains(&name))
}

/// The usable adapter on PCI device `pci`, Vulkan first.
fn by_pci(pci: &str, adapters: &[(wgpu::AdapterInfo, bool)]) -> Option<usize> {
    let on = |(info, usable): &(wgpu::AdapterInfo, bool)| *usable && same_pci(pci, &info.device_pci_bus_id);
    adapters.iter().position(|a| on(a) && a.0.backend == wgpu::Backend::Vulkan).or_else(|| adapters.iter().position(on))
}

/// Windows: the pick made from DXGI's adapter list, which makes no device and loads no driver;
/// wgpu then gets the one adapter chosen (`wgpu_hal::dx12::Instance::expose_adapter`, from the
/// vendored wgpu-hal). wgpu's own `enumerate_adapters` and `request_adapter` make a device on every
/// adapter to describe it, the discrete GPU and WARP included: 1,050,000–1,110,000 µs on a
/// two-GPU laptop, waking a discrete GPU the window never uses.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod dxgi {
    use super::{Pick, Reason};
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, DXGI_ADAPTER_DESC1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_GPU_PREFERENCE, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
        DXGI_GPU_PREFERENCE_MINIMUM_POWER, IDXGIAdapter1, IDXGIFactory1, IDXGIFactory6,
    };
    use windows::core::Interface;
    // Unused when the vendored wgpu-hal's inherent `expose_adapter` takes the call.
    #[allow(unused_imports)]
    use without_patch::ExposeAdapter as _;

    /// An app whose workspace doesn't patch wgpu-hal with Rune's copy (`[patch.crates-io]`, see
    /// `vendor/wgpu-hal`) gets wgpu-hal as published, which has no `expose_adapter`: the call then
    /// lands here (an inherent method wins over a trait's), and the pick falls back to wgpu's list.
    mod without_patch {
        use std::cell::Cell;

        thread_local! {
            pub(super) static UNPATCHED: Cell<bool> = const { Cell::new(false) };
        }

        #[allow(dead_code)]
        pub(super) trait ExposeAdapter {
            unsafe fn expose_adapter(&self, _: &windows::Win32::Graphics::Dxgi::IDXGIAdapter1) -> Option<wgpu::hal::ExposedAdapter<wgpu::hal::api::Dx12>> {
                UNPATCHED.with(|unpatched| unpatched.set(true));
                None
            }
        }
        impl ExposeAdapter for wgpu::hal::dx12::Instance {}
    }

    /// The pick and how many hardware adapters DXGI lists; None when the instance isn't DX12's,
    /// `wgpu-hal` is unpatched, or an automatic choice can't run D3D12 or present to `surface`
    /// (the caller then asks wgpu). For `WGPU_ADAPTER_NAME` the answer is final: the adapter it
    /// names, or the error saying why not, without making a device on any other adapter.
    pub(super) fn pick(instance: &wgpu::Instance, surface: Option<&wgpu::Surface<'_>>, presented: bool) -> Option<Result<(Pick, usize), String>> {
        // SAFETY: wgpu's DX12 instance, borrowed for this call.
        let hal = unsafe { instance.as_hal::<wgpu::hal::api::Dx12>() }?;
        // SAFETY: a DXGI factory and its adapters, asked for their descriptions.
        let factory = unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }.ok()?;
        let all: Vec<(IDXGIAdapter1, DXGI_ADAPTER_DESC1)> = (0..)
            .map_while(|i| {
                // SAFETY: `factory` is a live `IDXGIFactory1`; out-of-range `i` returns `DXGI_ERROR_NOT_FOUND`.
                unsafe { factory.EnumAdapters1(i) }.ok()
            })
            .filter_map(|a| {
                // SAFETY: `a` is a live `IDXGIAdapter1` returned by `EnumAdapters1`.
                unsafe { a.GetDesc1() }.ok().map(|d| (a, d))
            })
            .collect();
        let hardware: Vec<&(IDXGIAdapter1, DXGI_ADAPTER_DESC1)> =
            all.iter().filter(|(_, d)| d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0).collect();
        let count = hardware.len();
        let by_preference = |preference: DXGI_GPU_PREFERENCE| -> Option<IDXGIAdapter1> {
            let factory6 = factory.cast::<IDXGIFactory6>().ok()?;
            (0..)
                .map_while(|i| {
                    // SAFETY: `factory6` is a live `IDXGIFactory6`; out-of-range `i` returns `DXGI_ERROR_NOT_FOUND`.
                    unsafe { factory6.EnumAdapterByGpuPreference::<IDXGIAdapter1>(i, preference) }.ok()
                })
                .find(|a| {
                    // SAFETY: `a` is a live `IDXGIAdapter1` returned by `EnumAdapterByGpuPreference`.
                    unsafe { a.GetDesc1() }.is_ok_and(|d| d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0)
                })
        };
        let description = |d: &DXGI_ADAPTER_DESC1| {
            let len = d.Description.iter().position(|&c| c == 0).unwrap_or(d.Description.len());
            String::from_utf16_lossy(&d.Description[..len])
        };
        if let Some(wanted) = rune_core::env::var("WGPU_ADAPTER_NAME") {
            let wanted_lower = wanted.to_lowercase();
            // Software adapters too: `Microsoft Basic Render Driver` names WARP. Try every match
            // in DXGI order: headless Windows lists a non-software WARP entry that `expose_adapter`
            // rejects before the real software WARP adapter, and multi-adapter matches should skip
            // unusable ones just like `by_name`.
            let mut tried = Vec::new();
            for (adapter, desc) in all.iter().filter(|(_, d)| description(d).to_lowercase().contains(&wanted_lower)) {
                let label = description(desc);
                without_patch::UNPATCHED.with(|unpatched| unpatched.set(false));
                // SAFETY: `adapter` comes from a DXGI factory in this process, as `expose_adapter` asks.
                let Some(exposed) = (unsafe { hal.expose_adapter(adapter) }) else {
                    if without_patch::UNPATCHED.with(|unpatched| unpatched.replace(false)) {
                        return None;
                    }
                    tried.push(format!("{label:?} (can't make a D3D12 device now)"));
                    continue;
                };
                // SAFETY: `exposed` was exposed from this same DX12 `instance`'s HAL.
                let adapter = unsafe { instance.create_adapter_from_hal(exposed) };
                if surface.is_some_and(|s| !adapter.is_surface_supported(s)) {
                    tried.push(format!("{label:?} (can't present to this window)"));
                    continue;
                }
                return Some(Ok((Pick { adapter, reason: Reason::EnvName, compositor: None }, count)));
            }
            let error = if tried.is_empty() {
                format!("no GPU adapter matches WGPU_ADAPTER_NAME={wanted:?}")
            } else {
                format!("WGPU_ADAPTER_NAME={wanted:?} matched {}, none usable", tried.join(", "))
            };
            return Some(Err(error));
        }
        let (adapter, reason) = if let Some(power) = wgpu::PowerPreference::from_env() {
            let preference = if power == wgpu::PowerPreference::HighPerformance { DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE } else { DXGI_GPU_PREFERENCE_MINIMUM_POWER };
            (by_preference(preference)?, Reason::EnvPower)
        } else if let Some(found) = super::display_devices()
            .iter()
            .find_map(|ids| hardware.iter().find(|(_, d)| (d.VendorId, d.DeviceId) == *ids))
        {
            // Every device takes the GPU a window would get; `presented` only names the reason.
            (found.0.clone(), if surface.is_some() || presented { Reason::Display } else { Reason::WindowGpu })
        } else {
            (by_preference(DXGI_GPU_PREFERENCE_MINIMUM_POWER).or_else(|| hardware.first().map(|h| h.0.clone()))?, Reason::LowPower)
        };
        // SAFETY: `adapter` comes from a DXGI factory in this process, as `expose_adapter` asks.
        let exposed = unsafe { hal.expose_adapter(&adapter) }?;
        // SAFETY: `exposed` was exposed from this same DX12 `instance`'s HAL.
        let adapter = unsafe { instance.create_adapter_from_hal(exposed) };
        if surface.is_some_and(|s| !adapter.is_surface_supported(s)) {
            return None;
        }
        Some(Ok((Pick { adapter, reason, compositor: None }, count)))
    }
}

/// The usable hardware adapter for the first of `displays` (PCI vendor and device IDs, the
/// primary display's first) that has one.
fn by_display(displays: &[(u32, u32)], adapters: &[(wgpu::AdapterInfo, bool)]) -> Option<usize> {
    displays.iter().find_map(|ids| {
        adapters.iter().position(|(info, usable)| *usable && info.device_type != wgpu::DeviceType::Cpu && (info.vendor, info.device) == *ids)
    })
}

/// `PowerPreference::LowPower`'s order; a software adapter comes last.
pub fn power_rank(device: wgpu::DeviceType) -> u8 {
    match device {
        wgpu::DeviceType::IntegratedGpu => 0,
        wgpu::DeviceType::DiscreteGpu => 1,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Other => 3,
        wgpu::DeviceType::Cpu => 4,
    }
}

/// The PCI vendor of the GPU driving the primary display, which `pick` chooses for a window:
/// known before any GPU is made, for the CPU renderer that draws until then. Read once, at
/// launch (`find_display_vendor_soon`): enumerating the display devices takes ~1,500 µs.
pub fn display_vendor() -> Option<u32> {
    *DISPLAY_VENDOR.get_or_init(|| display_devices().first().map(|ids| ids.0))
}

static DISPLAY_VENDOR: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();

/// Starts reading [`display_vendor`] on a thread of its own once the event loop exists, so the
/// first window needn't wait for it; a window that asks sooner waits for that thread.
#[cfg(target_os = "windows")]
pub fn find_display_vendor_soon() {
    std::thread::Builder::new().name("rune-display-vendor".into()).spawn(display_vendor).ok();
}

/// The PCI vendor and device IDs of the GPUs driving a display on the desktop, the primary
/// display's first.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn display_devices() -> Vec<(u32, u32)> {
    use windows_sys::Win32::Graphics::Gdi::{DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICE_PRIMARY_DEVICE, DISPLAY_DEVICEW, EnumDisplayDevicesW};
    let mut found = Vec::new();
    for index in 0.. {
        // SAFETY: `DISPLAY_DEVICEW` is a plain Win32 C struct where all-zero bytes are valid.
        let mut device = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..unsafe { std::mem::zeroed() } };
        // SAFETY: `device` is a DISPLAY_DEVICEW with `cb` set, as the call requires.
        if unsafe { EnumDisplayDevicesW(std::ptr::null(), index, &mut device, 0) } == 0 { break; }
        if device.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == 0 { continue; }
        let len = device.DeviceID.iter().position(|&c| c == 0).unwrap_or(device.DeviceID.len());
        let Some(ids) = pci_ids(&String::from_utf16_lossy(&device.DeviceID[..len])) else { continue };
        if device.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE != 0 {
            found.retain(|other| *other != ids);
            found.insert(0, ids);
        } else if !found.contains(&ids) {
            found.push(ids);
        }
    }
    found
}

#[cfg(not(target_os = "windows"))]
fn display_devices() -> Vec<(u32, u32)> {
    Vec::new()
}

/// The vendor and device IDs in a PnP hardware ID (`PCI\VEN_10DE&DEV_2C05&SUBSYS_…`); None for
/// a display that isn't on a PCI GPU (a remote or indirect display).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn pci_ids(id: &str) -> Option<(u32, u32)> {
    let rest = id.get(..4).filter(|p| p.eq_ignore_ascii_case("PCI\\")).and_then(|_| id.get(4..))?;
    let field = |key: &str| rest.split('&').find_map(|part| {
        let value = part.get(..key.len()).filter(|k| k.eq_ignore_ascii_case(key)).and_then(|_| part.get(key.len()..))?;
        u32::from_str_radix(value, 16).ok()
    });
    Some((field("VEN_")?, field("DEV_")?))
}

fn same_pci(a: &str, b: &str) -> bool {
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}

fn log(pick: Pick, adapters: Option<usize>) -> Pick {
    static FIRST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
    if gpu_log() || (adapters.is_some_and(|n| n > 1) && FIRST.swap(false, std::sync::atomic::Ordering::Relaxed)) {
        let info = pick.adapter.get_info();
        let pci = if info.device_pci_bus_id.is_empty() { String::new() } else { format!(", {}", info.device_pci_bus_id) };
        let compositor = pick.compositor.as_deref().map(|c| format!("; compositor on {c}")).unwrap_or_default();
        eprintln!("rune: GPU {:?} ({}; {:?}, {:?}{pci}): {}{compositor}", info.name, crate::backend::label(&pick.adapter), info.backend, info.device_type, pick.reason.describe());
    }
    pick
}

/// A pick that found no adapter, said always: the device it was for won't be made.
fn log_failure(error: &str) {
    eprintln!("rune: no GPU: {error}");
}

fn gpu_log() -> bool {
    rune_core::env::var_os("RUNE_GPU_LOG").is_some_and(|v| v != "0")
}

/// The compositor's main device as a PCI address, asked once per process.
#[cfg(target_os = "linux")]
fn compositor_pci() -> Option<String> {
    static PCI: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    PCI.get_or_init(|| {
        let (major, minor) = dev_numbers(&dmabuf::main_device()?)?;
        pci_address(&std::fs::read_link(format!("/sys/dev/char/{major}:{minor}/device")).ok()?)
    })
    .clone()
}

#[cfg(not(target_os = "linux"))]
fn compositor_pci() -> Option<String> {
    None
}

/// A `dev_t` sent as native-endian bytes, split into major and minor numbers (glibc's and the
/// kernel's encoding).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn dev_numbers(bytes: &[u8]) -> Option<(u32, u32)> {
    let dev = match bytes.len() {
        8 => u64::from_ne_bytes(bytes.try_into().ok()?),
        4 => u64::from(u32::from_ne_bytes(bytes.try_into().ok()?)),
        _ => return None,
    };
    let major = ((dev >> 32) & 0xffff_f000) | ((dev >> 8) & 0x0fff);
    let minor = ((dev >> 12) & 0xffff_ff00) | (dev & 0xff);
    Some((major as u32, minor as u32))
}

/// The PCI address a DRM node's `device` link points at (`../../../0000:01:00.0`), lowercased;
/// None for a device that isn't on PCI.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn pci_address(link: &std::path::Path) -> Option<String> {
    let name = link.file_name()?.to_str()?.to_ascii_lowercase();
    let (domain, rest) = name.split_once(':')?;
    let (bus, rest) = rest.split_once(':')?;
    let (device, function) = rest.split_once('.')?;
    let hex = |s: &str, len: usize| s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit());
    (hex(domain, 4) && hex(bus, 2) && hex(device, 2) && hex(function, 1)).then_some(name)
}

#[cfg(target_os = "linux")]
mod dmabuf {
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::wl_registry::WlRegistry;
    use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
    use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_feedback_v1::{self, ZwpLinuxDmabufFeedbackV1};
    use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1;

    #[derive(Default)]
    struct State {
        main: Option<Vec<u8>>,
        done: bool,
    }

    impl Dispatch<WlRegistry, GlobalListContents> for State {
        fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
    }

    impl Dispatch<ZwpLinuxDmabufV1, ()> for State {
        fn event(_: &mut Self, _: &ZwpLinuxDmabufV1, _: <ZwpLinuxDmabufV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    }

    impl Dispatch<ZwpLinuxDmabufFeedbackV1, ()> for State {
        fn event(state: &mut Self, _: &ZwpLinuxDmabufFeedbackV1, event: zwp_linux_dmabuf_feedback_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
            match event {
                zwp_linux_dmabuf_feedback_v1::Event::MainDevice { device } => state.main = Some(device),
                zwp_linux_dmabuf_feedback_v1::Event::Done => state.done = true,
                _ => {}
            }
        }
    }

    /// The compositor's main device `dev_t`, from its default dmabuf feedback on a short-lived
    /// connection of our own. None off Wayland or before `zwp_linux_dmabuf_v1` version 4.
    /// Version 6 stops sending the main device, so Rune binds at most 5.
    pub(super) fn main_device() -> Option<Vec<u8>> {
        std::env::var_os("WAYLAND_DISPLAY")?;
        let conn = Connection::connect_to_env().ok()?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn).ok()?;
        let dmabuf = globals.bind::<ZwpLinuxDmabufV1, _, _>(&queue.handle(), 4..=5, ()).ok()?;
        let feedback = dmabuf.get_default_feedback(&queue.handle(), ());
        let mut state = State::default();
        for _ in 0..4 {
            if state.done { break; }
            queue.roundtrip(&mut state).ok()?;
        }
        feedback.destroy();
        dmabuf.destroy();
        queue.flush().ok();
        state.main
    }
}

#[cfg(test)]
#[path = "pick_tests.rs"]
mod tests;
