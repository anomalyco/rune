//! PROOF ONLY, never merged: what `rune_gpu::health::removed` costs a call, and its parts.
use std::time::Instant;

fn main() {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::DX12;
    let instance = wgpu::Instance::new(desc);
    let pick = rune_gpu::pick::pick(&instance, None, false).expect("adapter");
    let (device, _queue) = pollster::block_on(pick.adapter.request_device(&Default::default())).expect("device");
    println!("adapter {:?}", pick.adapter.get_info().name);
    let measure = |what: &str, f: &dyn Fn() -> bool| {
        let (batch, batches) = (1000u32, 1000usize);
        for _ in 0..batch * 10 {
            std::hint::black_box(f());
        }
        let mut per: Vec<f64> = (0..batches)
            .map(|_| {
                let t = Instant::now();
                for _ in 0..batch {
                    std::hint::black_box(f());
                }
                t.elapsed().as_nanos() as f64 / f64::from(batch)
            })
            .collect();
        per.sort_by(f64::total_cmp);
        println!("{what}: median {:.1} ns, p10 {:.1}, p90 {:.1}, p99 {:.1} ns per call ({batches} x {batch} calls)", per[batches / 2], per[batches / 10], per[batches * 9 / 10], per[batches * 99 / 100]);
    };
    measure("removed()", &|| rune_gpu::health::removed(&device));
    let removal = rune_gpu::health::Removal::watch(&device);
    measure("Removal::removed()", &|| removal.removed());
    measure("as_hal alone", &|| unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }.is_some());
    let hal = unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }.expect("dx12");
    let raw = hal.raw_device().clone();
    drop(hal);
    measure("GetDeviceRemovedReason alone", &|| unsafe { raw.GetDeviceRemovedReason() }.is_err());
    measure("poll(Poll), idle device", &|| rune_gpu::health::poll(&device, wgpu::PollType::Poll).is_ok());
}
