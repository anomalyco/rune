use super::{is_wide_color_config, select_surface_format};

fn mac_metal_caps(with_extended_linear: bool) -> wgpu::SurfaceCapabilities {
    use wgpu::TextureFormat as F;
    let formats = vec![F::Bgra8Unorm, F::Bgra8UnormSrgb, F::Rgba16Float, F::Rgb10a2Unorm];
    let format_capabilities = formats
        .iter()
        .map(|&format| {
            let mut color_spaces = wgpu::SurfaceColorSpaces::SRGB;
            if with_extended_linear && format == F::Rgba16Float {
                color_spaces |= wgpu::SurfaceColorSpaces::EXTENDED_SRGB_LINEAR;
            }
            wgpu::SurfaceFormatCapabilities { format, color_spaces }
        })
        .collect();
    wgpu::SurfaceCapabilities {
        formats,
        format_capabilities,
        present_modes: vec![wgpu::PresentMode::Fifo],
        alpha_modes: vec![wgpu::CompositeAlphaMode::Opaque],
        usages: wgpu::TextureUsages::RENDER_ATTACHMENT,
    }
}

#[test]
fn default_wide_color_off_keeps_standard_srgb_surface_format() {
    let caps = mac_metal_caps(true);
    assert_eq!(
        select_surface_format(&caps, true, false, true),
        Some((wgpu::TextureFormat::Bgra8Unorm, vec![], wgpu::SurfaceColorSpace::Auto)),
    );
    assert_eq!(
        select_surface_format(&caps, false, false, true),
        Some((wgpu::TextureFormat::Bgra8UnormSrgb, vec![], wgpu::SurfaceColorSpace::Auto)),
    );
}

#[test]
fn opt_in_wide_color_selects_rgba16float_extended_linear_srgb_when_supported() {
    let caps = mac_metal_caps(true);
    // Regardless of initial srgb_blend flag, opt-in wide color uses linear Rgba16Float.
    for srgb_blend in [true, false] {
        assert_eq!(
            select_surface_format(&caps, srgb_blend, true, true),
            Some((
                wgpu::TextureFormat::Rgba16Float,
                vec![],
                wgpu::SurfaceColorSpace::ExtendedSrgbLinear,
            )),
            "srgb_blend={srgb_blend}",
        );
    }
}

#[test]
fn wide_color_falls_back_to_srgb_when_platform_disallowed_or_unsupported() {
    let with_caps = mac_metal_caps(true);
    assert_eq!(
        select_surface_format(&with_caps, true, true, false),
        Some((wgpu::TextureFormat::Bgra8Unorm, vec![], wgpu::SurfaceColorSpace::Auto)),
        "non-macOS platforms keep sRGB so unscaled scRGB is not accidentally enabled",
    );

    let without_caps = mac_metal_caps(false);
    assert_eq!(
        select_surface_format(&without_caps, true, true, true),
        Some((wgpu::TextureFormat::Bgra8Unorm, vec![], wgpu::SurfaceColorSpace::Auto)),
        "surface without EXTENDED_SRGB_LINEAR capability falls back cleanly to sRGB",
    );
}

#[test]
fn is_wide_color_config_only_matches_rgba16float_extended_linear_srgb() {
    let base = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: wgpu::TextureFormat::Bgra8Unorm,
        width: 64,
        height: 64,
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: wgpu::CompositeAlphaMode::Opaque,
        view_formats: vec![],
        color_space: wgpu::SurfaceColorSpace::Auto,
    };
    assert!(!is_wide_color_config(&base));
    assert!(!is_wide_color_config(&wgpu::SurfaceConfiguration {
        format: wgpu::TextureFormat::Rgba16Float,
        ..base.clone()
    }));
    assert!(is_wide_color_config(&wgpu::SurfaceConfiguration {
        format: wgpu::TextureFormat::Rgba16Float,
        color_space: wgpu::SurfaceColorSpace::ExtendedSrgbLinear,
        ..base
    }));
}
