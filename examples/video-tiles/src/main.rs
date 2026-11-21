//! Copy this producer/view boundary into a VideoToolbox application.

#![forbid(unsafe_code)]

use rune::native_surface::{ByteFrame, synthetic};
use rune::{AppSpec, Cx, El, NativeSurface, SurfaceFit, Waker, col, el, row};
use std::time::Duration;
mod bench;

fn push(surface: &NativeSurface, bytes: &ByteFrame, id: u64) {
    #[cfg(target_os = "macos")]
    surface.push(rune::native_surface::macos::PixelBuffer::from_bytes(bytes).unwrap(), id).unwrap();
    #[cfg(not(target_os = "macos"))]
    surface
        .push_bytes(
            ByteFrame {
                width: bytes.width,
                height: bytes.height,
                format: bytes.format,
                color: bytes.color,
                planes: bytes.planes.clone(),
            },
            id,
        )
        .unwrap();
}
struct Tiles {
    surfaces: Vec<NativeSurface>,
}
fn state() -> Tiles {
    let surfaces: Vec<_> = (0..4).map(|_| NativeSurface::new()).collect();
    let bytes = synthetic(640, 360, 0);
    for s in &surfaces {
        push(s, &bytes, 0);
    }
    Tiles { surfaces }
}
fn setup(s: &mut Tiles, wake: Waker) {
    for surface in &s.surfaces {
        surface.set_waker(wake.clone());
    }
    // Headless scenes use the deterministic initial frame, not a wall-clock producer.
    if std::env::args()
        .any(|v| matches!(v.as_str(), "--snap" | "--frames" | "--first-pixels" | "--tree" | "--trace" | "--strip"))
    {
        return;
    }
    let surfaces = s.surfaces.clone();
    std::thread::spawn(move || {
        for id in 1..=u64::MAX {
            let bytes = synthetic(640, 360, id);
            for s in &surfaces {
                push(s, &bytes, id);
            }
            std::thread::sleep(Duration::from_millis(16));
        }
    });
}
fn view(s: &Tiles, _: &mut Cx<Tiles>) -> El {
    col().key("tiles").gap(8.0).padding(8.0).children((0..2).map(|y| {
        row().gap(8.0).children((0..2).map(|x| {
            el().key(format!("tile-{x}-{y}"))
                .w(320.0)
                .h(180.0)
                .radius(8.0)
                .native_surface(&s.surfaces[y * 2 + x], SurfaceFit::Fit)
        }))
    }))
}
fn main() {
    if std::env::args().any(|a| a == "--bench-native") {
        bench::run();
        return;
    }
    rune::run(AppSpec { setup: Some(setup), ..AppSpec::new("Video tiles", (664.0, 384.0), state, view) });
}
