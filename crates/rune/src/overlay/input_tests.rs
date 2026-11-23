use std::sync::Arc;

use super::should_recreate_layer;
use crate::overlay::{OverlayHandle, Requests, ScreenEdge, Shared};
use crate::{Align, DisplayTarget, NativeDisplayId, OverlayPlacement};

fn requests_after(f: impl FnOnce(&OverlayHandle<()>)) -> Requests {
    let shared = Arc::new(Shared::<()>::default());
    let handle = OverlayHandle { shared: shared.clone() };
    f(&handle);
    std::mem::take(&mut *shared.core.requests.lock().unwrap())
}

#[test]
fn wayland_layer_recreates_only_when_target_display_changes() {
    let primary = DisplayTarget::Primary;
    let out_a = DisplayTarget::Native(NativeDisplayId::Wayland(7));
    let out_b = DisplayTarget::Native(NativeDisplayId::Wayland(8));

    // Placement updates (Rect, Edge, FullScreen, and None) stay on the live layer surface.
    for placement in [
        Some(OverlayPlacement::Rect { x: 24.0, y: 48.0, width: 240.0, height: 64.0 }),
        Some(OverlayPlacement::Edge { edge: ScreenEdge::Bottom, margin: 16.0, align: Align::End, inset: 12.0 }),
        Some(OverlayPlacement::FullScreen),
        None,
    ] {
        let req = requests_after(|h| h.set_placement(placement));
        assert!(!should_recreate_layer(&primary, &req, true), "placement {placement:?} must update in place");
        assert!(!should_recreate_layer(&out_a, &req, true), "placement {placement:?} on a targeted output must update in place");
    }

    // Re-specifying the same target display (with or without a new placement) also updates in place.
    let same = requests_after(|h| {
        h.set_display(out_a.clone());
        h.set_placement(Some(OverlayPlacement::Rect { x: 10.0, y: 10.0, width: 100.0, height: 40.0 }));
    });
    assert!(!should_recreate_layer(&out_a, &same, true), "same display must not recreate the layer");

    // Switching to another display must recreate the layer because wl_output is fixed at creation.
    let to_a = requests_after(|h| h.set_display(out_a.clone()));
    assert!(should_recreate_layer(&primary, &to_a, true));
    assert!(!should_recreate_layer(&primary, &to_a, false), "non-layer backings never recreate a layer");

    let to_b_with_placement = requests_after(|h| {
        h.set_display(out_b.clone());
        h.set_placement(Some(OverlayPlacement::FullScreen));
    });
    assert!(should_recreate_layer(&out_a, &to_b_with_placement, true));

    let back_to_primary = requests_after(|h| h.set_display(primary.clone()));
    assert!(should_recreate_layer(&out_a, &back_to_primary, true));
}
