//! PROOF ONLY, never merged (rune-proof: device-removal): 20 removals on WARP (one per child
//! process, since D3D12 keeps one device per adapter per process) with ~80 ms of compute queued,
//! releasing in `queue-first`, `together`, and `device-first-immediate` order, then making a
//! replacement WARP device and running compute on it to prove zero leaked COM refs.
#![allow(clippy::undocumented_unsafe_blocks)]

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("Windows only");
}

#[cfg(target_os = "windows")]
mod win {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    use windows::Win32::Graphics::Direct3D12::ID3D12Device5;
    use windows::core::Interface;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    #[repr(C)]
    struct ExceptionRecord {
        code: u32,
        flags: u32,
        inner: *mut ExceptionRecord,
        address: *mut core::ffi::c_void,
        parameters: u32,
        information: [usize; 15],
    }
    #[repr(C)]
    struct ExceptionPointers {
        record: *mut ExceptionRecord,
        context: *mut core::ffi::c_void,
    }
    type Filter = unsafe extern "system" fn(*mut ExceptionPointers) -> i32;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetUnhandledExceptionFilter(filter: Option<Filter>) -> Option<Filter>;
        fn GetModuleHandleExW(flags: u32, name: *const core::ffi::c_void, module: *mut *mut core::ffi::c_void) -> i32;
        fn GetModuleFileNameW(module: *mut core::ffi::c_void, name: *mut u16, size: u32) -> u32;
    }

    unsafe extern "system" fn crash_filter(pointers: *mut ExceptionPointers) -> i32 {
        let record = unsafe { &*(*pointers).record };
        let mut module = core::ptr::null_mut();
        let found = unsafe { GetModuleHandleExW(0x4 | 0x2, record.address, &mut module) } != 0;
        let mut name = [0u16; 260];
        let len = if found { unsafe { GetModuleFileNameW(module, name.as_mut_ptr(), 260) } } else { 0 };
        eprintln!(
            "proof crash: exception {:#010x} at {:?} in {} +{:#x}; params {:?}",
            record.code,
            record.address,
            String::from_utf16_lossy(&name[..len as usize]),
            (record.address as usize).wrapping_sub(module as usize),
            &record.information[..(record.parameters as usize).min(15)],
        );
        0
    }

    fn assert_warp(adapter: &wgpu::Adapter) {
        let info = adapter.get_info();
        if info.device_type != wgpu::DeviceType::Cpu || !info.name.contains("Microsoft Basic Render Driver") {
            eprintln!("refusing to run on {:?} ({:?}): WARP only", info.name, info.device_type);
            std::process::exit(3);
        }
    }

    fn build_compute(device: &wgpu::Device) -> (wgpu::ShaderModule, wgpu::ComputePipeline, wgpu::Buffer, wgpu::BindGroup) {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(
                "@group(0) @binding(0) var<storage, read_write> r: array<u32>;
                @compute @workgroup_size(64) fn main(@builtin(global_invocation_id) id: vec3<u32>) {
                    var x = id.x; for (var i = 0u; i < 256u; i++) { x = (x ^ (x >> 13u)) * 1664525u + 1013904223u; } r[id.x] = x; }"
                    .into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 65536 * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
        });
        (shader, pipeline, buffer, bind)
    }

    fn run_child(idx: usize) {
        unsafe { SetUnhandledExceptionFilter(Some(crash_filter)) };
        let arm = match idx % 3 {
            0 => "queue-first",
            1 => "together",
            _ => "device-first-immediate",
        };
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::DX12;
        let instance = wgpu::Instance::new(desc);
        let adapter = rune_gpu::pick::pick(&instance, None, false).expect("adapter").adapter;
        assert_warp(&adapter);
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).expect("device");
        let (shader, pipeline, buffer, bind) = build_compute(&device);
        let submit = |n: u32| {
            for _ in 0..n {
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let mut pass = encoder.begin_compute_pass(&Default::default());
                    pass.set_pipeline(&pipeline);
                    pass.set_bind_group(0, &bind, &[]);
                    pass.dispatch_workgroups(1024, 1, 1);
                }
                queue.submit([encoder.finish()]);
            }
        };
        let t = Instant::now();
        submit(1);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let one = t.elapsed();
        let n = (Duration::from_millis(80).as_micros() / one.as_micros().max(1)).clamp(4, 400) as u32;
        submit(n);

        // Remove the WARP device while `n` submissions are queued.
        assert_warp(&adapter);
        let hal = unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }.expect("D3D12 device");
        let device5: ID3D12Device5 = hal.raw_device().cast().expect("ID3D12Device5");
        unsafe { device5.RemoveDevice() };
        drop(device5);
        drop(hal);

        let t_rel = Instant::now();
        drop((bind, buffer, pipeline, shader));
        let release_us = match arm {
            "queue-first" => {
                drop(queue);
                let us = t_rel.elapsed().as_micros();
                std::thread::sleep(Duration::from_millis(50));
                drop(device);
                drop(adapter);
                us
            }
            "together" => {
                drop((adapter, device, queue));
                t_rel.elapsed().as_micros()
            }
            _ => {
                drop(device);
                drop(adapter);
                drop(queue);
                t_rel.elapsed().as_micros()
            }
        };

        // Wait briefly to catch any late WARP worker AV.
        std::thread::sleep(Duration::from_millis(150));

        // Prove zero leaked D3D12 references: a fresh WARP device must come up and execute work.
        let t_new = Instant::now();
        let adapter2 = rune_gpu::pick::pick(&instance, None, false).expect("replacement adapter").adapter;
        assert_warp(&adapter2);
        let (device2, queue2) = pollster::block_on(adapter2.request_device(&Default::default())).expect("replacement device");
        let (s2, p2, b2, bg2) = build_compute(&device2);
        let mut enc = device2.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&p2);
            pass.set_bind_group(0, &bg2, &[]);
            pass.dispatch_workgroups(64, 1, 1);
        }
        queue2.submit([enc.finish()]);
        let poll_res = rune_gpu::health::poll(&device2, wgpu::PollType::wait_indefinitely());
        assert!(poll_res.is_ok(), "replacement device failed poll: {poll_res:?}");
        let replace_us = t_new.elapsed().as_micros();
        drop((bg2, b2, p2, s2, queue2, device2, adapter2, instance));

        println!("CHILD_OK idx={idx} arm={arm} one_us={} queued={n} release_us={release_us} replace_us={replace_us}", one.as_micros());
    }

    /// Wrap-around proof (B2): execute >16,384 HAL command lists (wrapping the 16,384-slot
    /// readback ring buffer), verify zero entries are stuck in `in_flight`, then queue ~60 ms of
    /// compute work on the wrapped slots, remove WARP with work queued, and drop. Must drain
    /// promptly (< 200 ms, 0 timeouts, 0 stuck entries, NOT the 250 ms stall) and recreate WARP.
    fn run_wrap() {
        unsafe { SetUnhandledExceptionFilter(Some(crash_filter)) };
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::DX12;
        let instance = wgpu::Instance::new(desc);
        let adapter = rune_gpu::pick::pick(&instance, None, false).expect("adapter").adapter;
        assert_warp(&adapter);
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).expect("device");
        let (shader, pipeline, buffer, bind) = build_compute(&device);
        let submit_compute = |n: u32| {
            for _ in 0..n {
                let mut enc = device.create_command_encoder(&Default::default());
                {
                    let mut pass = enc.begin_compute_pass(&Default::default());
                    pass.set_pipeline(&pipeline);
                    pass.set_bind_group(0, &bind, &[]);
                    pass.dispatch_workgroups(1024, 1, 1);
                }
                queue.submit([enc.finish()]);
            }
        };
        let t_cal = Instant::now();
        submit_compute(1);
        assert!(rune_gpu::health::poll(&device, wgpu::PollType::wait_indefinitely()).is_ok());
        let one = t_cal.elapsed();

        let rec_start = wgpu::hal::dx12::proof_record_count();
        // Queue 16 compute submissions (~80-100 ms) first, then 130 encoders x 64 empty compute
        // passes (each compute pass records 2 HAL lists: Pre Pass + Pass -> 16,640+ HAL lists > 16,384).
        submit_compute(16);
        for i in 0..130 {
            let mut enc = device.create_command_encoder(&Default::default());
            for _ in 0..64 {
                let _pass = enc.begin_compute_pass(&Default::default());
            }
            queue.submit([enc.finish()]);
            if i % 16 == 15 {
                let _ = device.poll(wgpu::PollType::Poll);
            }
        }
        let rec_total = wgpu::hal::dx12::proof_record_count() - rec_start;
        assert!(rec_total > 16_384, "expected >16384 HAL lists recorded, got {rec_total}");
        // Wait for all >16,384 wrapped lists to complete on WARP.
        assert!(rune_gpu::health::poll(&device, wgpu::PollType::wait_indefinitely()).is_ok());

        // Submit 1 tiny pass so `track_submitted` prunes all completed wrapped entries, then check
        // that zero entries from the >16,384 lists remained stuck in `in_flight`.
        {
            let mut enc = device.create_command_encoder(&Default::default());
            let _pass = enc.begin_compute_pass(&Default::default());
            queue.submit([enc.finish()]);
        }
        let pruned_after_wrap = wgpu::hal::dx12::proof_last_pruned_len();
        assert!(rune_gpu::health::poll(&device, wgpu::PollType::wait_indefinitely()).is_ok());
        assert_eq!(
            pruned_after_wrap, 0,
            "stale entries stuck in in_flight after >16384 lists: {pruned_after_wrap}"
        );

        // Now queue ~60 ms of compute work on already-wrapped slots and remove WARP while queued.
        let n = (Duration::from_millis(60).as_micros() / one.as_micros().max(1)).clamp(4, 200) as u32;
        submit_compute(n);

        assert_warp(&adapter);
        let hal = unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }.expect("D3D12 device");
        let device5: ID3D12Device5 = hal.raw_device().cast().expect("ID3D12Device5");
        unsafe { device5.RemoveDevice() };
        drop(device5);
        drop(hal);

        let t_rel = Instant::now();
        drop((bind, buffer, pipeline, shader, adapter, device, queue));
        let wrap_release_us = t_rel.elapsed().as_micros();
        std::thread::sleep(Duration::from_millis(150));

        let timeouts = wgpu::hal::dx12::proof_wait_timeouts();
        let stuck = wgpu::hal::dx12::proof_wait_stuck_count();
        let wrap_hits = wgpu::hal::dx12::proof_wrap_done_hits();
        assert_eq!(timeouts, 0, "wait_after_removal hit timeout ({timeouts}) after wrap-around!");
        assert_eq!(stuck, 0, "wait_after_removal left {stuck} stuck entries!");
        assert!(
            wrap_release_us < 200_000,
            "wrap-around release stalled: {wrap_release_us} us (stall cap is 250_000 us)"
        );

        // Verify replacement WARP device comes up and runs compute cleanly.
        let t_new = Instant::now();
        let adapter2 = rune_gpu::pick::pick(&instance, None, false).expect("replacement adapter").adapter;
        assert_warp(&adapter2);
        let (device2, queue2) = pollster::block_on(adapter2.request_device(&Default::default())).expect("replacement device");
        let (s2, p2, b2, bg2) = build_compute(&device2);
        let mut enc = device2.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&p2);
            pass.set_bind_group(0, &bg2, &[]);
            pass.dispatch_workgroups(64, 1, 1);
        }
        queue2.submit([enc.finish()]);
        let poll_res = rune_gpu::health::poll(&device2, wgpu::PollType::wait_indefinitely());
        assert!(poll_res.is_ok(), "replacement device failed poll: {poll_res:?}");
        let replace_us = t_new.elapsed().as_micros();
        drop((bg2, b2, p2, s2, queue2, device2, adapter2, instance));
        println!(
            "WRAP_OK hal_lists={rec_total} wrap_done_hits={wrap_hits} pruned_after_wrap={pruned_after_wrap} one_us={} queued={n} release_us={wrap_release_us} timeouts={timeouts} stuck={stuck} replace_us={replace_us}",
            one.as_micros()
        );
    }

    pub fn main() {
        let args: Vec<String> = std::env::args().collect();
        if args.get(1).map(String::as_str) == Some("--child") {
            let idx: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
            run_child(idx);
            return;
        }
        if args.get(1).map(String::as_str) == Some("--wrap") {
            run_wrap();
            return;
        }
        let runs: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(20);
        let exe = std::env::current_exe().expect("current_exe");
        let mut release_times = Vec::new();
        let mut replace_times = Vec::new();
        let mut failures = 0usize;
        for idx in 0..runs {
            let out = Command::new(&exe)
                .args(["--child", &idx.to_string()])
                .env("OPENCODE_RUNE_OTEL", "0")
                .creation_flags(CREATE_NO_WINDOW)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .expect("spawn child");
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !out.status.success() {
                failures += 1;
                eprintln!("RUN {idx:02} FAILED status={:?}\nstdout:\n{stdout}\nstderr:\n{stderr}", out.status.code());
            } else {
                let line = stdout.lines().find(|l| l.starts_with("CHILD_OK")).unwrap_or("");
                eprintln!("RUN {idx:02} PASS {line}");
                for tok in line.split_whitespace() {
                    if let Some(v) = tok.strip_prefix("release_us=").and_then(|s| s.parse::<u128>().ok()) {
                        release_times.push(v);
                    }
                    if let Some(v) = tok.strip_prefix("replace_us=").and_then(|s| s.parse::<u128>().ok()) {
                        replace_times.push(v);
                    }
                }
            }
        }
        release_times.sort_unstable();
        replace_times.sort_unstable();
        let stats = |v: &[u128]| -> String {
            if v.is_empty() { return "none".into(); }
            format!("min={} median={} max={} us", v[0], v[v.len() / 2], v[v.len() - 1])
        };
        eprintln!(
            "SUMMARY: {}/{runs} passed, {failures} failed; release: {}; replacement: {}",
            runs - failures,
            stats(&release_times),
            stats(&replace_times),
        );
        // Also run the >16,384-list wrap-around proof in its own child process.
        let wrap_out = Command::new(&exe)
            .arg("--wrap")
            .env("OPENCODE_RUNE_OTEL", "0")
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("spawn wrap child");
        let w_stdout = String::from_utf8_lossy(&wrap_out.stdout);
        let w_stderr = String::from_utf8_lossy(&wrap_out.stderr);
        if !wrap_out.status.success() {
            failures += 1;
            eprintln!("WRAP FAILED status={:?}\nstdout:\n{w_stdout}\nstderr:\n{w_stderr}", wrap_out.status.code());
        } else {
            let line = w_stdout.lines().find(|l| l.starts_with("WRAP_OK")).unwrap_or("");
            eprintln!("WRAP PASS {line}");
        }
        if failures > 0 {
            std::process::exit(1);
        }
    }
}

#[cfg(target_os = "windows")]
fn main() {
    win::main();
}
