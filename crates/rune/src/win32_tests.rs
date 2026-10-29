use super::*;
use winit::event_loop::EventLoop;
use winit::platform::windows::EventLoopBuilderExtWindows;
use winit::window::Window;

#[test]
#[allow(deprecated)]
fn overlay_window_hides_after_each_native_show() {
    let event_loop = EventLoop::builder().with_any_thread(true).build().unwrap();
    let window = event_loop.create_window(Window::default_attributes().with_visible(false)).unwrap();
    prepare_overlay(&window, false);
    let hwnd = hwnd(&window).unwrap();
    for _ in 0..2 {
        show_overlay(&window);
        assert_ne!(unsafe { IsWindowVisible(hwnd) }, 0, "native show exposes the window");
        // The native show bypasses winit's visibility flag, as it does for an overlay.
        window.set_visible(false);
        hide_overlay(&window);
        assert_eq!(unsafe { IsWindowVisible(hwnd) }, 0, "hide removes the native window");
    }
}
