//! `Tx`'s reads about the key a key handler is seeing: repeat, physical key, unmodified
//! character, modifiers and the focused element.

use crate::Tx;

impl Tx {
    /// Whether the key a key handler is seeing is the OS auto-repeat of a held key: true for
    /// each repeat, false for the first press and outside key handlers. One-shot actions
    /// (start recording on Enter, say) can return early on a repeat; arrows and Tab usually
    /// want every repeat.
    pub fn key_repeat(&self) -> bool { self.key_repeat }

    /// In a key handler, the physical key behind the logical key it's seeing: where the key
    /// sits, whatever the keyboard layout and Shift make it type. `KeyEvent` carries the
    /// character, which moves between layouts once Shift is held (`⌘⇧7` arrives as
    /// `Key::Command('&')` on a US layout and `Key::Command('/')` on a German one), so a shifted
    /// shortcut matches this instead: `tx.physical_key() == Some(PhysicalKeyCode::Digit7)`.
    /// `None` for a key with no physical origin (`Runtime::key`, a script's `"key"` without a
    /// `"code"`) and outside key handlers. Headless hosts and embedders can supply both with
    /// [`Runtime::key_with`](crate::Runtime::key_with) or
    /// [`Runtime::physical_key_input`](crate::Runtime::physical_key_input).
    pub fn physical_key(&self) -> Option<crate::PhysicalKeyCode> { self.physical_key }

    /// In a key handler, the character the key types under the current keyboard layout with
    /// no modifiers held: `'a'` for ⌥A (which arrives as `Key::Text("å")` on a US Mac layout)
    /// and ⇧A, and `'a'` for AZERTY's A key, which sits where QWERTY's Q does. A binding by
    /// letter, as macOS's `DefaultKeyBinding.dict` binds ⌥-letters, matches this rather than
    /// [`Tx::physical_key`], which names where the key sits (`KeyQ` there). On the web,
    /// derived from `KeyboardEvent.key` when unmodified (or lowercased for shifted letters, so
    /// non-ASCII chords like Ctrl+Ö report `Some('ö')`) and `None` for shifted punctuation,
    /// Alt/Option combinations and dead keys. `None` on iOS and Android, for a key that types no
    /// single character (arrows, Return), for a key with no physical origin (`Runtime::key`, a
    /// script's `"key"` without a `"code"`) and outside key handlers.
    pub fn key_base(&self) -> Option<char> { self.key_base }

    /// The modifier keys held as this handler runs, so a click can tell Shift-click (extend a
    /// list's selection) and Cmd-click (toggle a row) from a plain one. A key handler reads
    /// the key's own [`KeyEvent::mods`](crate::KeyEvent) instead.
    pub fn mods(&self) -> crate::Mods { self.mods }

    /// When the input this handler runs for arrived, to measure input-to-photon latency
    /// against [`FrameEncoded`](crate::FrameEncoded): on macOS the timestamp of the `NSEvent`
    /// AppKit dispatched it in, elsewhere when the window took it from winit. Input that piles
    /// up between frames keeps each event's own time. `None` in handlers no input caused
    /// (timers, polls, updates, layout callbacks) and for input fed to a `Runtime` directly
    /// (`Runtime::key` and kin, headless scripts).
    pub fn input_time(&self) -> Option<crate::time::Instant> { self.input_time }

    /// In `poll`: whether the user's input (a key, a press, the pointer, the wheel, text) is
    /// waiting for this frame's handlers, which run after `poll`. Work that can wait a frame (a
    /// background start, a cache warm-up) waits, so the input's frame comes without it. False
    /// everywhere else.
    pub fn input_waiting(&self) -> bool { self.input_waiting }

    /// The key of the element that had focus when the event arrived, `None` when nothing had:
    /// in a key handler, when the key arrived; in a press handler, after the press moved focus (a
    /// menu opened there remembers it to hand focus back). Timers, polls and updates read the focus
    /// as they run. An unkeyed element reads as `#n`, its
    /// index among its siblings, and a focused [`Link`](crate::Link) as its id. A window's `on_key` hears
    /// a key either before anything else (nothing focused) or after the focused control passed
    /// it on, so `tx.focused().is_none()` picks out the first case: plain arrows can drive the
    /// window (a settings pane) only while no control has focus, as in System Settings. The
    /// whole order is under "Key routing" in the rune README.
    pub fn focused(&self) -> Option<&str> {
        self.focused.as_deref()
    }

    /// Whether focus is on an element matching `key` or anywhere inside it (CSS `:focus-within`),
    /// as this event arrived (or after the transition inside [`El::on_focus_change`](crate::El::on_focus_change)
    /// and [`El::on_focus_within_change`](crate::El::on_focus_within_change)).
    ///
    /// `key` takes the same key or `/`-separated key path that [`Cx::layout`](crate::Cx::layout)
    /// resolves (skipping unkeyed `#n` ancestors): `"popover"` matches any focused control inside
    /// `"popover"` including unkeyed descendants, while `"popover/close"` distinguishes a `"close"`
    /// button inside `"popover"` from one inside `"page"`. Returns `false` when nothing has focus
    /// or `key` is empty.
    ///
    /// ```
    /// use rune::Tx;
    ///
    /// fn close_if_outside(tx: &Tx) -> bool {
    ///     !tx.focus_within("popover")
    /// }
    /// assert!(close_if_outside(&Tx::default()));
    /// ```
    pub fn focus_within(&self, key: &str) -> bool {
        if key.is_empty() { return false; }
        let Some(path) = self.focused_path.as_deref() else { return false };
        if let Some(link) = self.focused_link.as_deref()
            && crate::flight::selects_under(key, path, link)
        {
            return true;
        }
        let mut cur = path;
        loop {
            let own = cur.rsplit_once('/').map_or(cur, |(_, s)| s);
            if let Some(decoded) = crate::runtime::element_path::decode_key_segment(own)
                && crate::flight::selects(key, cur, Some(decoded.as_ref()))
            {
                return true;
            }
            let Some((above, _)) = cur.rsplit_once('/') else { return false };
            cur = above;
        }
    }
}
