//! research/30's HUD frame on raw Vulkan with the fewest calls the API allows: one command
//! buffer, a render pass whose layouts do the swapchain transitions (no `vkCmdPipelineBarrier`),
//! one `vkQueueSubmit`, one present. Views, framebuffers and the render pass are made once.
//!
//! `WRITES=n` writes a 256-byte uniform `n` times a frame. `UPLOAD=mapped` (default) copies into
//! a persistently mapped host-visible ring, as on a unified-memory or resizable-BAR GPU, with no
//! API call. `UPLOAD=copy` stages the bytes and records a `vkCmdCopyBuffer` per write into a
//! device-local buffer, then one buffer barrier, in the frame's command buffer.

#[path = "../count.rs"]
mod count;
#[path = "../harness.rs"]
mod harness;

use std::sync::Arc;

use ash::vk;
use harness::{Draw, env};
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::Window;

const IN_FLIGHT: usize = 2;
const SLOT: u64 = 256;

struct Floor {
    _window: Arc<Window>,
    device: ash::Device,
    swapchain_fn: ash::khr::swapchain::Device,
    swapchain: vk::SwapchainKHR,
    queue: vk::Queue,
    render_pass: vk::RenderPass,
    framebuffers: Vec<vk::Framebuffer>,
    extent: vk::Extent2D,
    pools: [vk::CommandPool; IN_FLIGHT],
    cmds: [vk::CommandBuffer; IN_FLIGHT],
    fences: [vk::Fence; IN_FLIGHT],
    acquired: [vk::Semaphore; IN_FLIGHT],
    rendered: Vec<vk::Semaphore>,
    mapped: *mut u8,
    staging: vk::Buffer,
    uniforms: vk::Buffer,
    writes: usize,
    copy: bool,
    frame: usize,
}

fn memory_type(instance: &ash::Instance, pd: vk::PhysicalDevice, bits: u32, flags: vk::MemoryPropertyFlags) -> u32 {
    let props = unsafe { instance.get_physical_device_memory_properties(pd) };
    (0..props.memory_type_count)
        .find(|&i| bits & (1 << i) != 0 && props.memory_types[i as usize].property_flags.contains(flags))
        .expect("memory type")
}

unsafe fn buffer(instance: &ash::Instance, pd: vk::PhysicalDevice, device: &ash::Device, size: u64, usage: vk::BufferUsageFlags,
    flags: vk::MemoryPropertyFlags) -> (vk::Buffer, vk::DeviceMemory) {
    unsafe {
        let buffer = device.create_buffer(&vk::BufferCreateInfo::default().size(size).usage(usage), None).unwrap();
        let req = device.get_buffer_memory_requirements(buffer);
        let info = vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(memory_type(instance, pd, req.memory_type_bits, flags));
        let memory = device.allocate_memory(&info, None).unwrap();
        device.bind_buffer_memory(buffer, memory, 0).unwrap();
        (buffer, memory)
    }
}

impl Floor {
    unsafe fn new(window: Arc<Window>) -> Floor {
        unsafe {
            let entry = ash::Entry::load().expect("libvulkan");
            let display = window.display_handle().unwrap().as_raw();
            let exts = ash_window::enumerate_required_extensions(display).unwrap();
            let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_2);
            let instance = entry.create_instance(&vk::InstanceCreateInfo::default().application_info(&app).enabled_extension_names(exts), None).unwrap();
            let surface = ash_window::create_surface(&entry, &instance, display, window.window_handle().unwrap().as_raw(), None).unwrap();
            let surface_fn = ash::khr::surface::Instance::new(&entry, &instance);
            let (pd, family) = instance.enumerate_physical_devices().unwrap().into_iter().find_map(|pd| {
                instance.get_physical_device_queue_family_properties(pd).iter().enumerate().find_map(|(i, q)| {
                    let present = surface_fn.get_physical_device_surface_support(pd, i as u32, surface).unwrap_or(false);
                    (q.queue_flags.contains(vk::QueueFlags::GRAPHICS) && present).then_some((pd, i as u32))
                })
            }).expect("a device that presents");
            let name = instance.get_physical_device_properties(pd).device_name_as_c_str().unwrap().to_owned();
            eprintln!("device: {name:?}");
            let priorities = [1.0];
            let queues = [vk::DeviceQueueCreateInfo::default().queue_family_index(family).queue_priorities(&priorities)];
            let dev_exts = [ash::khr::swapchain::NAME.as_ptr()];
            let device = instance.create_device(pd, &vk::DeviceCreateInfo::default().queue_create_infos(&queues).enabled_extension_names(&dev_exts), None).unwrap();
            let queue = device.get_device_queue(family, 0);

            let caps = surface_fn.get_physical_device_surface_capabilities(pd, surface).unwrap();
            let format = surface_fn.get_physical_device_surface_formats(pd, surface).unwrap().into_iter()
                .find(|f| f.format == vk::Format::B8G8R8A8_SRGB).expect("BGRA8 sRGB");
            let extent = vk::Extent2D { width: 360, height: 80 };
            let swapchain_fn = ash::khr::swapchain::Device::new(&instance, &device);
            let swapchain = swapchain_fn.create_swapchain(&vk::SwapchainCreateInfoKHR::default()
                .surface(surface).min_image_count(caps.min_image_count.max(2) + 1).image_format(format.format)
                .image_color_space(format.color_space).image_extent(extent).image_array_layers(1)
                .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT).pre_transform(caps.current_transform)
                .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE).present_mode(vk::PresentModeKHR::FIFO).clipped(true), None).unwrap();

            let attachment = [vk::AttachmentDescription::default().format(format.format).samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::CLEAR).store_op(vk::AttachmentStoreOp::STORE)
                .initial_layout(vk::ImageLayout::UNDEFINED).final_layout(vk::ImageLayout::PRESENT_SRC_KHR)];
            let color = [vk::AttachmentReference { attachment: 0, layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL }];
            let subpass = [vk::SubpassDescription::default().pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS).color_attachments(&color)];
            // Orders the layout transition after the acquire semaphore's wait stage.
            let dependency = [vk::SubpassDependency::default().src_subpass(vk::SUBPASS_EXTERNAL).dst_subpass(0)
                .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT).dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)];
            let render_pass = device.create_render_pass(&vk::RenderPassCreateInfo::default()
                .attachments(&attachment).subpasses(&subpass).dependencies(&dependency), None).unwrap();

            let images = swapchain_fn.get_swapchain_images(swapchain).unwrap();
            let framebuffers = images.iter().map(|&image| {
                let view = device.create_image_view(&vk::ImageViewCreateInfo::default().image(image).view_type(vk::ImageViewType::TYPE_2D)
                    .format(format.format).subresource_range(vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1)), None).unwrap();
                let views = [view];
                device.create_framebuffer(&vk::FramebufferCreateInfo::default().render_pass(render_pass).attachments(&views)
                    .width(extent.width).height(extent.height).layers(1), None).unwrap()
            }).collect();
            let rendered = images.iter().map(|_| device.create_semaphore(&Default::default(), None).unwrap()).collect();

            let pools = [(); IN_FLIGHT].map(|_| device.create_command_pool(&vk::CommandPoolCreateInfo::default()
                .flags(vk::CommandPoolCreateFlags::TRANSIENT).queue_family_index(family), None).unwrap());
            let cmds = pools.map(|pool| device.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default()
                .command_pool(pool).command_buffer_count(1)).unwrap()[0]);
            let fences = [(); IN_FLIGHT].map(|_| device.create_fence(&vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED), None).unwrap());
            let acquired = [(); IN_FLIGHT].map(|_| device.create_semaphore(&Default::default(), None).unwrap());

            let writes = env("WRITES", 0usize);
            let ring = SLOT * writes.max(1) as u64 * IN_FLIGHT as u64;
            let host = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
            let (staging, staging_memory) = buffer(&instance, pd, &device, ring,
                vk::BufferUsageFlags::UNIFORM_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC, host);
            let mapped = device.map_memory(staging_memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty()).unwrap().cast();
            let (uniforms, _) = buffer(&instance, pd, &device, SLOT,
                vk::BufferUsageFlags::UNIFORM_BUFFER | vk::BufferUsageFlags::TRANSFER_DST, vk::MemoryPropertyFlags::DEVICE_LOCAL);
            let copy = std::env::var("UPLOAD").as_deref() == Ok("copy");
            Floor { _window: window, device, swapchain_fn, swapchain, queue, render_pass, framebuffers, extent, pools, cmds, fences,
                acquired, rendered, mapped, staging, uniforms, writes, copy, frame: 0 }
        }
    }
}

impl Draw for Floor {
    fn frame(&mut self) {
        let f = self.frame % IN_FLIGHT;
        self.frame += 1;
        let d = &self.device;
        unsafe {
            d.wait_for_fences(&[self.fences[f]], true, u64::MAX).unwrap();
            d.reset_fences(&[self.fences[f]]).unwrap();
            d.reset_command_pool(self.pools[f], vk::CommandPoolResetFlags::empty()).unwrap();
            let (image, _) = self.swapchain_fn.acquire_next_image(self.swapchain, u64::MAX, self.acquired[f], vk::Fence::null()).unwrap();
            let phase = (self.frame % 120) as f32 / 120.0;
            let data = [phase.to_bits(); 64];
            let cmd = self.cmds[f];
            d.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT)).unwrap();
            for w in 0..self.writes {
                let offset = (f * self.writes + w) as u64 * SLOT;
                std::ptr::copy_nonoverlapping(data.as_ptr().cast::<u8>(), self.mapped.add(offset as usize), SLOT as usize);
                if self.copy {
                    d.cmd_copy_buffer(cmd, self.staging, self.uniforms, &[vk::BufferCopy { src_offset: offset, dst_offset: 0, size: SLOT }]);
                }
            }
            if self.copy && self.writes > 0 {
                let barrier = [vk::BufferMemoryBarrier::default().buffer(self.uniforms).size(vk::WHOLE_SIZE)
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE).dst_access_mask(vk::AccessFlags::UNIFORM_READ)];
                d.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::VERTEX_SHADER | vk::PipelineStageFlags::FRAGMENT_SHADER,
                    vk::DependencyFlags::empty(), &[], &barrier, &[]);
            }
            let clear = [vk::ClearValue { color: vk::ClearColorValue { float32: [0.0, 0.0, phase * 0.4, 0.4] } }];
            d.cmd_begin_render_pass(cmd, &vk::RenderPassBeginInfo::default().render_pass(self.render_pass)
                .framebuffer(self.framebuffers[image as usize]).render_area(self.extent.into()).clear_values(&clear), vk::SubpassContents::INLINE);
            d.cmd_end_render_pass(cmd);
            d.end_command_buffer(cmd).unwrap();
            let waits = [self.acquired[f]];
            let stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
            let cmds = [cmd];
            let signals = [self.rendered[image as usize]];
            let submit = [vk::SubmitInfo::default().wait_semaphores(&waits).wait_dst_stage_mask(&stages).command_buffers(&cmds).signal_semaphores(&signals)];
            d.queue_submit(self.queue, &submit, self.fences[f]).unwrap();
            let swapchains = [self.swapchain];
            let images = [image];
            self.swapchain_fn.queue_present(self.queue, &vk::PresentInfoKHR::default()
                .wait_semaphores(&signals).swapchains(&swapchains).image_indices(&images)).unwrap();
        }
    }
}

fn main() {
    harness::run("vk_floor", |window| Box::new(unsafe { Floor::new(window) }));
}
