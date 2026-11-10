//! One balanced native secure-input claim per focused, visible password window.
#[derive(Default)]
pub(crate) struct SecureInput(bool);

impl SecureInput {
    pub(crate) fn set(&mut self, active: bool) {
        if active == self.0 { return; }
        // Tests never take the claim: it would block every other app's key events.
        #[cfg(all(target_os = "macos", not(test)))]
        unsafe {
            if active {
                self.0 = EnableSecureEventInput() == 0;
            } else {
                DisableSecureEventInput();
                self.0 = false;
            }
        }
        #[cfg(any(not(target_os = "macos"), test))]
        { self.0 = active; }
    }

    /// The window gained or lost focus. Losing it releases the claim at once: an idle window
    /// builds no frame to do it. Gaining it waits for a frame that sees a focused secure field.
    pub(crate) fn window_focus(&mut self, focused: bool) {
        if !focused { self.set(false); }
    }
}

impl Drop for SecureInput {
    fn drop(&mut self) { self.set(false); }
}

#[cfg(all(target_os = "macos", not(test)))]
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn EnableSecureEventInput() -> i32;
    fn DisableSecureEventInput() -> i32;
}

#[cfg(test)]
mod tests {
    use super::SecureInput;
    use crate::runtime::Runtime;
    use crate::{Cx, El, Env, TextEdit, TextStyle};

    fn view(s: &TextEdit, _: &mut Cx<TextEdit>) -> El {
        crate::text_input(s, TextStyle::new(16.0, 400.0)).single_line().secure()
            .key("secret").size(200.0, 40.0).on_edit(|s: &mut TextEdit| s)
    }

    /// A window that goes to the background with its password field focused holds no claim,
    /// though the field keeps its focus and no frame is built.
    #[test]
    fn a_background_window_with_a_focused_secure_field_holds_no_claim() {
        let mut edit = TextEdit::new("hunter2");
        edit.focused = true;
        let mut rt = Runtime::new(edit, view, (200.0, 40.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let mut claim = SecureInput::default();
        claim.set(rt.has_secure_focus());
        assert!(claim.0, "a focused secure field in the key window claims secure input");

        // As the host does for `WindowEvent::Focused(false)`, with no frame after it.
        rt.window_focus(false);
        claim.window_focus(false);
        assert!(rt.has_secure_focus(), "the field keeps its focus");
        assert!(!claim.0, "the background window released the claim");

        claim.window_focus(true);
        assert!(!claim.0, "focus alone doesn't claim before a frame sees the field");
    }

    /// A window with no secure field never claims, focused or not.
    #[test]
    fn a_window_without_a_secure_field_never_claims() {
        fn plain(s: &TextEdit, _: &mut Cx<TextEdit>) -> El {
            crate::text_input(s, TextStyle::new(16.0, 400.0)).single_line()
                .key("plain").size(200.0, 40.0).on_edit(|s: &mut TextEdit| s)
        }
        let mut edit = TextEdit::new("");
        edit.focused = true;
        let mut rt = Runtime::new(edit, plain, (200.0, 40.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let mut claim = SecureInput::default();
        claim.set(rt.has_secure_focus());
        assert!(!claim.0);
    }
}
