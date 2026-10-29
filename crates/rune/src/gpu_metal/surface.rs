//! The CAMetalLayer a surface presents into, and its drawables.

use std::sync::{Arc, OnceLock};

use objc2_core_foundation::CGSize;
use objc2_quartz_core::{CAAutoresizingMask, CALayer, CAMetalDrawable, CAMetalLayer};
use objc2_video::msg_send;
use objc2_video::rc::Retained;
use objc2_video::runtime::AnyObject;
use wgpu::custom::*;

use super::encode::{MetalTexture, pixel_format};
use super::{MetalDevice, Obj, Shared, send_sync};

#[derive(Debug)]
pub(crate) struct MetalSurface {
    layer: Retained<CAMetalLayer>,
    shared: OnceLock<Arc<Shared>>,
}

#[derive(Debug)]
pub(crate) struct MetalOutput {
    pub(crate) drawable: Option<Obj<dyn CAMetalDrawable>>,
    shared: Option<Arc<Shared>>,
}

send_sync!(MetalSurface, MetalOutput);

/// A CAMetalLayer layer as given, or one filling an AppKit view's own layer (on the main thread).
pub(super) unsafe fn create(target: wgpu::SurfaceTargetUnsafe) -> DispatchSurface {
    let handle = match target {
        wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer) => {
            // SAFETY: the caller hands a live CAMetalLayer.
            let layer = unsafe { Retained::retain(layer.cast::<CAMetalLayer>()) }.expect("a CAMetalLayer");
            return DispatchSurface::custom(MetalSurface { layer, shared: OnceLock::new() });
        }
        wgpu::SurfaceTargetUnsafe::RawHandle { raw_window_handle: wgpu::rwh::RawWindowHandle::AppKit(handle), .. } => handle,
        _ => panic!("RUNE_GPU_BACKEND=metal needs an AppKit view or a CAMetalLayer"),
    };
    let layer = CAMetalLayer::new();
    // SAFETY: a live NSView, on the main thread.
    unsafe {
        let view: &AnyObject = handle.ns_view.cast().as_ref();
        let _: () = msg_send![view, setWantsLayer: true];
        let root: Option<Retained<CALayer>> = msg_send![view, layer];
        if let Some(root) = root {
            layer.setFrame(root.bounds());
            layer.setContentsScale(root.contentsScale());
            layer.setAutoresizingMask(CAAutoresizingMask::LayerWidthSizable | CAAutoresizingMask::LayerHeightSizable);
            root.addSublayer(&layer);
        }
    }
    DispatchSurface::custom(MetalSurface { layer, shared: OnceLock::new() })
}

impl SurfaceInterface for MetalSurface {
    fn get_capabilities(&self, _: &DispatchAdapter) -> wgpu::SurfaceCapabilities {
        use wgpu::TextureFormat as F;
        let formats = vec![F::Bgra8Unorm, F::Bgra8UnormSrgb];
        wgpu::SurfaceCapabilities {
            format_capabilities: formats.iter().map(|&format| wgpu::SurfaceFormatCapabilities { format, color_spaces: wgpu::SurfaceColorSpaces::SRGB }).collect(),
            formats,
            present_modes: vec![wgpu::PresentMode::Fifo, wgpu::PresentMode::Immediate],
            alpha_modes: vec![wgpu::CompositeAlphaMode::Opaque, wgpu::CompositeAlphaMode::PostMultiplied],
            usages: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        }
    }

    fn configure(&self, device: &DispatchDevice, config: &wgpu::SurfaceConfiguration) {
        let device = device.as_custom::<MetalDevice>().expect("a metal spike device");
        let _ = self.shared.set(device.0.clone());
        let layer = &self.layer;
        layer.setDevice(Some(&device.0.device));
        layer.setPixelFormat(pixel_format(config.format));
        layer.setFramebufferOnly(config.usage == wgpu::TextureUsages::RENDER_ATTACHMENT);
        layer.setMaximumDrawableCount((config.desired_maximum_frame_latency + 1).clamp(2, 3) as usize);
        layer.setDrawableSize(CGSize::new(f64::from(config.width), f64::from(config.height)));
        layer.setDisplaySyncEnabled(!matches!(config.present_mode, wgpu::PresentMode::Immediate | wgpu::PresentMode::AutoNoVsync));
        layer.setOpaque(config.alpha_mode == wgpu::CompositeAlphaMode::Opaque);
        let srgb = objc2_core_graphics::CGColorSpace::with_name(Some(unsafe { objc2_core_graphics::kCGColorSpaceSRGB }));
        layer.setColorspace(srgb.as_deref());
    }

    fn get_current_texture(&self) -> (Option<DispatchTexture>, wgpu::SurfaceStatus, DispatchSurfaceOutputDetail) {
        let shared = self.shared.get().cloned();
        if let Some(shared) = &shared { shared.pace(); }
        let Some(drawable) = self.layer.nextDrawable() else {
            return (None, wgpu::SurfaceStatus::Timeout, DispatchSurfaceOutputDetail::custom(MetalOutput { drawable: None, shared }));
        };
        let texture = MetalTexture { texture: Some(drawable.texture()), drawable: true };
        let output = MetalOutput { drawable: Some(drawable), shared };
        (Some(DispatchTexture::custom(texture)), wgpu::SurfaceStatus::Good, DispatchSurfaceOutputDetail::custom(output))
    }
}

impl SurfaceOutputDetailInterface for MetalOutput {
    fn texture_discard(&self) {
        if let Some(shared) = &self.shared { shared.flush_held(); }
    }
    fn texture_release(&self) {}
}
