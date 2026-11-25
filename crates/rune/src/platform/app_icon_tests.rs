use super::*;

#[test]
fn a_pair_shows_the_side_of_the_appearance_and_the_bundle_shows_no_image() {
    let pair = AppIcon::Pair { light: b"light".to_vec(), dark: b"dark".to_vec() };
    assert_eq!((pair.shown(false), pair.shown(true)), (Some(&b"light"[..]), Some(&b"dark"[..])));
    assert!(pair.follows_appearance());
    let image = AppIcon::Image(b"one".to_vec());
    assert_eq!((image.shown(false), image.shown(true)), (Some(&b"one"[..]), Some(&b"one"[..])));
    assert!(!image.follows_appearance());
    assert_eq!((AppIcon::Bundle.shown(false), AppIcon::Bundle.shown(true)), (None, None));
    assert!(!AppIcon::Bundle.follows_appearance());
    assert_eq!(pair.images().collect::<Vec<_>>(), [&b"light"[..], &b"dark"[..]]);
    assert_eq!(AppIcon::Bundle.images().count(), 0);
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn png() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(16, 16, image::Rgba([40, 120, 220, 255])).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

#[cfg(target_os = "macos")]
#[test]
fn an_icon_must_be_an_image_and_waits_for_launch() {
    use std::sync::atomic::Ordering;
    assert!(set_app_icon(b"not an image".to_vec()).is_err());
    assert!(set_app_icon_choice(AppIcon::Pair { light: png(), dark: b"not an image".to_vec() }).is_err());
    assert!(set_app_icon(png()).is_ok());
    assert!(set_app_icon_choice(AppIcon::Pair { light: png(), dark: png() }).is_ok());
    // Without an event loop nothing touches NSApplication, so a headless run stays windowless.
    assert!(!LAUNCHED.load(Ordering::Acquire));
    assert!(matches!(CURRENT.lock().unwrap().as_ref(), Some((_, AppIcon::Pair { .. }))));
    assert!(set_app_icon_choice(AppIcon::Bundle).is_ok());
    assert!(matches!(CURRENT.lock().unwrap().as_ref(), Some((_, AppIcon::Bundle))));
}

#[cfg(target_os = "windows")]
#[test]
fn a_pair_fails_on_windows_when_either_side_is_not_an_image() {
    assert!(set_app_icon_choice(AppIcon::Pair { light: png(), dark: b"not an image".to_vec() }).is_err());
    assert!(set_app_icon_choice(AppIcon::Pair { light: b"not an image".to_vec(), dark: png() }).is_err());
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod linux {
    use crate::platform::app_icon::*;

    fn png(px: u32) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(px, px, image::Rgba([40, 120, 220, 255])).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    // One test, since both steps share the process's icon.
    #[test]
    fn an_icon_must_be_an_image_and_reaches_each_new_window() {
        assert!(set_app_icon(b"not an image".to_vec()).is_err());
        assert!(set_app_icon_choice(AppIcon::Pair { light: png(16), dark: b"not an image".to_vec() }).is_err());
        assert!(set_app_icon(png(16)).is_ok());
        assert_eq!(LINUX.lock().unwrap().as_ref().map(Vec::len), Some((LINUX_SIZE * LINUX_SIZE * 4) as usize));
        assert!(with_icon(winit::window::WindowAttributes::default()).window_icon.is_some());
        assert!(set_app_icon_choice(AppIcon::Bundle).is_ok());
        assert!(with_icon(winit::window::WindowAttributes::default()).window_icon.is_none());
    }
}
