// Browser text services and semantic DOM accessibility mirror.
const MAC = /Mac|iP/.test(navigator.platform);
const coarse = () => matchMedia('(pointer: coarse)').matches;
export function isMac() { return MAC; }

// Detects platforms whose browser dispatches touch `contextmenu` while the finger is still down
// (Android, ChromeOS, Linux). Excludes Windows (Chrome defers touch contextmenu until finger lift
// via context_menu_on_mouse_up), iOS/iPadOS (WebKit never fires touch contextmenu, including
// iPadOS desktop mode where platform is MacIntel with maxTouchPoints > 1), and macOS.
export function supportsLiveTouchContextMenu(nav = globalThis.navigator) {
    if (!nav) return false;
    const uad = (nav.userAgentData?.platform ?? '').toLowerCase();
    const plat = (nav.platform ?? '').toLowerCase();
    const ua = (nav.userAgent ?? '').toLowerCase();
    const maxTouch = typeof nav.maxTouchPoints === 'number' ? nav.maxTouchPoints : 0;

    if (uad.includes('win') || uad.includes('mac') || uad.includes('ios')) return false;
    if (plat.includes('win') || plat.includes('mac') || /iphone|ipad|ipod/.test(plat)) return false;
    if (/windows|iphone|ipad|ipod|macintosh|mac os x/.test(ua)) return false;
    if (plat === 'macintel' && maxTouch > 1) return false;

    if (uad.includes('android') || uad.includes('chrome os') || uad.includes('chromeos') || uad.includes('linux')) return true;
    if (plat.includes('linux') || plat.includes('android')) return true;
    return /android|cros|linux/.test(ua);
}
const high = n => n >= 0xd800 && n <= 0xdbff;
const low = n => n >= 0xdc00 && n <= 0xdfff;
function boundary(text, at, forward) {
    if (at > 0 && at < text.length && high(text.charCodeAt(at - 1)) && low(text.charCodeAt(at))) at += forward ? 1 : -1;
    return at;
}

function replacement(old, next, composition, caret) {
    let start = 0;
    while (start < old.length && start < next.length && old[start] === next[start]) start++;
    start = boundary(old, start, false);
    let end = old.length, nextEnd = next.length;
    while (end > start && nextEnd > start && old[end - 1] === next[nextEnd - 1]) { end--; nextEnd--; }
    const adjusted = boundary(old, end, true) - end;
    end += adjusted; nextEnd += adjusted;
    if (composition && caret >= composition.length && next.slice(caret - composition.length, caret) === composition) {
        const from = caret - composition.length;
        const to = caret + old.length - next.length;
        if (from <= to && to <= old.length) {
            if (old === next) { start = from; end = to; nextEnd = caret; }
            else { start = Math.min(start, from); end = Math.max(end, to); nextEnd = end + next.length - old.length; }
        }
    }
    return { start, end, text: next.slice(start, nextEnd) };
}

// HTML <textarea> normalizes CRLF and lone CR to LF in its API value. Track the normalized mirror
// and the raw UTF-16 offsets of each CRLF so diffs leave untouched line breaks intact and offsets
// translate cleanly between DOM coordinates and Rune's text.
function normalizeTextarea(raw) {
    if (!raw.includes('\r')) return { dom: raw, crlfs: null };
    const crlfs = [];
    const dom = raw.replace(/\r\n?/g, (match, offset) => {
        if (match.length === 2) crlfs.push(offset);
        return '\n';
    });
    return { dom, crlfs: crlfs.length ? crlfs : null };
}

function rawToDom(offset, crlfs) {
    if (!crlfs) return offset;
    let lo = 0, hi = crlfs.length;
    while (lo < hi) {
        const mid = (lo + hi) >> 1;
        if (crlfs[mid] < offset) lo = mid + 1;
        else hi = mid;
    }
    return offset - lo;
}

function domToRaw(offset, crlfs) {
    if (!crlfs) return offset;
    let lo = 0, hi = crlfs.length;
    while (lo < hi) {
        const mid = (lo + hi) >> 1;
        if (crlfs[mid] - mid < offset) lo = mid + 1;
        else hi = mid;
    }
    return offset + lo;
}

function browserShortcut(event) {
    const chord = (event.metaKey || event.ctrlKey) && !event.getModifierState?.('AltGraph');
    if (event.key === 'F5' || event.key === 'F12') return true;
    if (!chord) return false;
    const k = event.key.toLowerCase();
    // With Alt only the developer tools' chords are the browser's: Ctrl+Alt+W and the like are
    // the app's (its Close tab, where the browser keeps Ctrl+W).
    if (event.altKey) return ['i', 'j', 'u'].includes(k);
    // An installed app's window gives the page every key first (Chromium: "In Apps mode, no keys
    // are reserved"), and closes itself on a Ctrl+W the page lets through: there the tab, window
    // and new-session keys are the app's, as in its desktop build.
    if (['t', 'w', 'n'].includes(k) && appWindow()) return false;
    // Leave copy ('c') un-prevented too so the browser fires its synchronous `copy` event.
    return ['c', 'r', 'l', 't', 'w', 'n', 'f', 'g', '=', '+', '-', '0'].includes(k);
}

function appWindow() {
    return navigator.standalone === true
        || matchMedia('(display-mode: standalone), (display-mode: window-controls-overlay), (display-mode: minimal-ui)').matches;
}

// Keys that scroll a page when nothing in Rune has focus.
const SCROLL_KEYS = [' ', 'PageUp', 'PageDown', 'Home', 'End', 'ArrowUp', 'ArrowDown'];
// Function keys Rune parses (`named_key_from_str`: F1–F24).
const FUNCTION_KEY = /^F([1-9]|1\d|2[0-4])$/;
// Hardware media transport keys Rune parses when opted in via `App::media_keys`.
const MEDIA_KEYS = ['MediaPlayPause', 'MediaPlay', 'MediaPause', 'MediaTrackNext', 'MediaTrackPrevious', 'MediaStop', 'MediaFastForward', 'MediaRewind'];

const graphemeSegmenter = typeof Intl?.Segmenter === 'function'
    ? new Intl.Segmenter(undefined, { granularity: 'grapheme' })
    : null;

function isAutofilled(input) {
    if (typeof input?.matches !== 'function') return false;
    for (const selector of [':autofill', ':-webkit-autofill']) {
        try { if (input.matches(selector)) return true; } catch {}
    }
    return false;
}

function moreThanOneGrapheme(text, segmenter) {
    if (!segmenter || !text) return false;
    let count = 0;
    for (const _ of segmenter.segment(text)) {
        if (++count > 1) return true;
    }
    return false;
}

// Classifies a password `input` event as whole-value replacement ('secure_replace') for browser or
// password-manager fills, or incremental typing ('secure') for single-grapheme keystrokes.
export function secureKind(event, input, segmenter = graphemeSegmenter) {
    if (isAutofilled(input)) return 'secure_replace';
    const type = event?.inputType;
    if (!type || type === 'insertReplacementText') return 'secure_replace';
    if (type === 'insertText') {
        const added = event.data ?? input?.value ?? '';
        if (moreThanOneGrapheme(added, segmenter)) return 'secure_replace';
    }
    return 'secure';
}

// Handles one `input` event on the hidden password input: always empties the DOM value, ignores
// events while no Rune field has focus, and sends fills even when empty so a clear reaches Rust.
export function passwordInput(event, input, key, send) {
    const kind = secureKind(event, input);
    const text = input.value;
    input.value = '';
    if (!key) return;
    if (kind === 'secure_replace' || text) send({ kind, key, text });
}

// What a page's mirror needs: focus rings on its links and buttons, and a selection highlight
// over its transparent text. The page can restyle both.
const PAGE_CSS = `[data-rune-a11y] :focus-visible{outline:2px solid var(--rune-focus,Highlight);outline-offset:2px;border-radius:4px}
[data-rune-a11y] ::selection{background:var(--rune-selection,rgba(80,150,255,.35));color:transparent}
[data-rune-a11y] a,[data-rune-a11y] button{color:transparent;-webkit-tap-highlight-color:transparent}
[data-rune-a11y] [data-rune-scroll-x]::-webkit-scrollbar{display:none}`;

export function mount(canvas, callback, embedded, fitHeight, page) {
    const textarea = document.createElement('textarea');
    textarea.setAttribute('aria-label', 'Text input');
    textarea.setAttribute('autocapitalize', 'sentences');
    // It is 1 px wide: wrapped, its every character would be a line to a screen reader reading by
    // line. Unwrapped, a line is what a line break ends.
    textarea.wrap = 'off';
    const password = document.createElement('input');
    password.type = 'password';
    password.setAttribute('aria-label', 'Password input');
    password.autocomplete = 'current-password';
    password.setAttribute('autocorrect', 'off');
    password.setAttribute('autocapitalize', 'off');
    password.spellcheck = false;
    for (const field of [textarea, password]) {
        field.style.cssText = 'position:fixed;left:0;top:0;width:1px;height:1px;padding:0;border:0;opacity:0;font-size:16px;resize:none;pointer-events:none;';
        field.tabIndex = -1;
        field.inputMode = coarse() ? 'none' : 'text';
        field.inert = true;
        document.body.append(field);
    }
    // The mirror's text is transparent over the canvas. `forced-color-adjust:none` keeps it so in
    // Windows' contrast themes, which would otherwise paint it, on backplates, over what Rune drew.
    const a11y = document.createElement('div');
    a11y.setAttribute('data-rune-a11y', '');
    a11y.setAttribute('role', 'document');
    let band = null;
    if (page) {
        // The canvas is a band of the page, not a window fixed to the screen: clipped to the
        // page's own height, in the document's flow, so the browser's compositor scrolls the
        // pixels Rune drew rather than moving a fixed layer at a frame's delay.
        band = document.createElement('div');
        band.setAttribute('data-rune-band', '');
        band.style.cssText = 'position:absolute;left:0;top:0;width:100%;height:0;overflow:hidden;overflow:clip;pointer-events:none;';
        canvas.replaceWith(band);
        band.append(canvas);
        a11y.style.cssText = 'position:absolute;left:0;top:0;width:100%;height:0;overflow-x:clip;pointer-events:none;z-index:1;color:transparent;forced-color-adjust:none;touch-action:pan-y pinch-zoom;';
        const style = document.createElement('style');
        style.textContent = PAGE_CSS;
        a11y.append(style);
        document.body.append(a11y);
    } else if (embedded && canvas.parentElement) {
        if (getComputedStyle(canvas.parentElement).position === 'static') canvas.parentElement.style.position = 'relative';
        a11y.style.cssText = 'position:absolute;inset:0;overflow:hidden;pointer-events:none;z-index:1;color:transparent;forced-color-adjust:none;';
        canvas.after(a11y);
    } else {
        a11y.style.cssText = 'position:fixed;inset:0;overflow:hidden;pointer-events:none;z-index:1;color:transparent;forced-color-adjust:none;';
        document.body.append(a11y);
    }
    const h = { canvas, textarea, password, a11y, a11yNodes: new Map(), field: textarea, key: '', base: 0, value: '', domValue: '', crlfs: null,
        selection: null, copyText: '', composing: false, composition: '', shift: false, modBits: 0, listeners: [], syncing: false, disposed: false,
        embedded, fitHeight, page, focused: false, presented: false, swapped: false, touchDown: false,
        touchTimestamps: [], touchHoldConfirm: null, activeTouchId: null,
        lastScrollAt: 0, staticTimer: 0, scrollEls: new Map(), band, mediaKeys: false };
    const send = event => {
        const message = JSON.stringify(event);
        if (h.syncing) queueMicrotask(() => { if (!h.disposed) callback(message); });
        else if (!h.disposed) callback(message);
    };
    h.send = send;
    const listen = (target, name, handler, options) => {
        target.addEventListener(name, handler, options);
        h.listeners.push(() => target.removeEventListener(name, handler, options));
    };
    const MOD_KEYS = ['Shift', 'Control', 'Alt', 'Meta', 'AltGraph'];
    const ownsTarget = target => target === canvas || target === textarea || target === password || (target instanceof Node && a11y.contains(target));
    const syncMods = event => {
        const bits = (event.shiftKey ? 1 : 0) | (event.ctrlKey ? 2 : 0) | (event.altKey ? 4 : 0) | (event.metaKey ? 8 : 0);
        if (bits === h.modBits) return;
        h.modBits = bits;
        send({ kind: 'mods', shift: !!(bits & 1), ctrl: !!(bits & 2), alt: !!(bits & 4), meta: !!(bits & 8) });
    };
    // Refresh modifiers from DOM events before winit's canvas listeners run: winit suppresses
    // ModifiersChanged on pointer/wheel when the canvas lacks focus (e.g. while a text field is
    // focused or in document mode) and emits KeyboardInput before ModifiersChanged.
    for (const name of ['pointerdown', 'pointermove', 'pointerup', 'pointerover', 'pointerout', 'wheel', 'keydown']) {
        listen(window, name, event => { if (ownsTarget(event.target)) syncMods(event); }, { capture: true, passive: true });
    }
    // Capture hardware PointerEvent.timeStamp before winit's canvas listeners drop it, so Rune
    // gets real event times for pan velocity, fling hold, and long-press delay compensation
    // without patching vendored winit.
    const MAX_TOUCH_TIMESTAMPS = 64;
    const recordTouchTimestamp = (isMove, event) => {
        if (event.target !== canvas || event.pointerType !== 'touch') return;
        // winit routes chorded pointermove (button !== -1) as a button press, not a touch move.
        if (isMove && typeof event.button === 'number' && event.button !== -1) return;
        const coalesced = isMove && typeof event.getCoalescedEvents === 'function' ? event.getCoalescedEvents() : null;
        const items = coalesced && coalesced.length > 0 ? coalesced : [event];
        for (const item of items) {
            h.touchTimestamps.push({ id: event.pointerId, ts: item.timeStamp });
            if (h.touchTimestamps.length > MAX_TOUCH_TIMESTAMPS) h.touchTimestamps.shift();
        }
    };
    listen(window, 'pointerdown', event => {
        if (event.target === canvas && event.pointerType === 'touch' && h.activeTouchId === null) {
            h.activeTouchId = event.pointerId;
        }
        recordTouchTimestamp(false, event);
    }, { capture: true, passive: true });
    listen(window, 'pointermove', event => recordTouchTimestamp(true, event), { capture: true, passive: true });
    const clearActiveTouch = event => {
        if (event.pointerType === 'touch' && event.pointerId === h.activeTouchId) h.activeTouchId = null;
        recordTouchTimestamp(false, event);
    };
    listen(window, 'pointerup', clearActiveTouch, { capture: true, passive: true });
    listen(window, 'pointercancel', clearActiveTouch, { capture: true, passive: true });
    listen(window, 'keyup', event => {
        if (ownsTarget(event.target) && !(event.target === canvas && MOD_KEYS.includes(event.key))) syncMods(event);
    }, { capture: true, passive: true });
    // Let winit deliver a modifier key's own release on the canvas before clearing its flag so
    // on_physical_input handlers see the key-up first.
    listen(window, 'keyup', event => {
        if (event.target === canvas && MOD_KEYS.includes(event.key)) syncMods(event);
    }, { passive: true });
    listen(window, 'blur', () => { h.modBits = 0; h.activeTouchId = null; });
    listen(document, 'visibilitychange', () => { if (document.hidden) { h.modBits = 0; h.activeTouchId = null; } });
    function changed(preedit = false) {
        const backward = textarea.selectionDirection === 'backward';
        const edit = replacement(h.domValue, textarea.value, preedit ? h.composition : '', textarea.selectionEnd);
        const start = domToRaw(edit.start, h.crlfs);
        const end = domToRaw(edit.end, h.crlfs);
        const mapNext = pos => {
            const insertedEnd = edit.start + edit.text.length;
            if (pos <= edit.start) return domToRaw(pos, h.crlfs);
            if (pos <= insertedEnd) return start + (pos - edit.start);
            return start + edit.text.length + (domToRaw(edit.end + (pos - insertedEnd), h.crlfs) - end);
        };
        const selStart = mapNext(textarea.selectionStart);
        const selEnd = mapNext(textarea.selectionEnd);
        send({ kind: preedit ? 'preedit' : 'edit', key: h.key, base: h.base, expected: h.value,
            start, end, text: edit.text,
            anchor: backward ? selEnd : selStart,
            focus: backward ? selStart : selEnd });
    }
    const key = (name, event = {}) => send({ kind: 'key', key: name, code: event.code ?? '', shift: !!event.shiftKey,
        ctrl: !!event.ctrlKey, alt: !!event.altKey, meta: !!event.metaKey, repeat: !!event.repeat });
    listen(textarea, 'input', event => {
        // No Rune field has the keyboard: nothing to edit, so the field stays empty.
        if (!h.key) { textarea.value = ''; return; }
        changed(h.composing || event.isComposing);
    });
    listen(textarea, 'compositionstart', () => { h.composing = true; h.composition = ''; });
    listen(textarea, 'compositionupdate', event => {
        h.composition = event.data ?? '';
        queueMicrotask(() => { if (!h.disposed && h.composing) changed(true); });
    });
    listen(textarea, 'compositionend', () => {
        changed();
        h.composing = false; h.composition = '';
        send({ kind: 'sync' });
    });
    listen(password, 'input', event => passwordInput(event, password, h.key, send));
    for (const field of [textarea, password]) {
        listen(field, 'paste', event => {
            // Only a paste into a Rune field is Rune's; the browser keeps the rest.
            if (!h.key) return;
            event.preventDefault();
            const data = event.clipboardData;
            h.image = field === textarea ? [...(data?.items ?? [])].find(i => i.kind === 'file' && i.type === 'image/png')?.getAsFile() ?? undefined : undefined;
            try {
                send({ kind: 'paste', text: data?.getData('text/plain') ?? '', html: field === textarea ? data?.getData('text/html') ?? '' : '', image: !!h.image });
            } finally {
                if (h.syncing) queueMicrotask(() => { h.image = undefined; });
                else h.image = undefined;
            }
        });
        listen(field, 'beforeinput', event => {
            if (event.inputType === 'insertLineBreak' || event.inputType === 'insertParagraph') {
                event.preventDefault();
                if (!h.composing && !event.isComposing) key('Enter', { shiftKey: h.shift, code: 'Enter' });
            } else if (field === password && event.inputType.startsWith('delete')) {
                event.preventDefault();
                const forward = event.inputType.endsWith('Forward');
                key(forward ? 'Delete' : 'Backspace',
                    { altKey: event.inputType.includes('Word'), code: forward ? 'Delete' : 'Backspace' });
            } else if (field === textarea && !h.composing && !event.isComposing && event.inputType.startsWith('delete')) {
                if (field.selectionStart === 0 && field.selectionEnd === 0 && event.inputType.endsWith('Backward')) {
                    event.preventDefault();
                    key('Backspace', { altKey: event.inputType.includes('Word'), code: 'Backspace' });
                } else if (field.selectionStart === field.value.length && field.selectionEnd === field.value.length
                    && event.inputType.endsWith('Forward')) {
                    event.preventDefault();
                    key('Delete', { altKey: event.inputType.includes('Word'), code: 'Delete' });
                }
            }
        });
        listen(field, 'keydown', event => {
            h.shift = event.shiftKey;
            if (event.isComposing || h.composing || event.keyCode === 229) return;
            // On Windows European layouts AltGr sets both ctrlKey and altKey; let those characters type.
            const chord = (event.metaKey || event.ctrlKey) && !event.getModifierState?.('AltGraph');
            const editing = ['Enter', 'Tab', 'Escape', 'ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End', 'PageUp', 'PageDown'];
            if (field === password) editing.push('Backspace', 'Delete');
            else {
                // At either boundary with nothing selected, the browser deletes nothing and fires
                // no `input` event, so forward Backspace or Delete as a key as native platforms
                // and password fields do.
                if (field.selectionStart === 0 && field.selectionEnd === 0) editing.push('Backspace');
                if (field.selectionStart === field.value.length && field.selectionEnd === field.value.length) editing.push('Delete');
            }
            // Forward function keys other than what `browserShortcut` leaves to the browser so
            // focused fields receive them as canvas focus does.
            const functionKey = FUNCTION_KEY.test(event.key) && !browserShortcut(event);
            const mediaKey = h.mediaKeys && MEDIA_KEYS.includes(event.key);
            if (!chord && !functionKey && !mediaKey && !editing.includes(event.key)) return;
            if (chord && event.key.toLowerCase() === 'v') return;
            event.preventDefault();
            key(event.key, event);
        });
        listen(field, 'keyup', event => { h.shift = event.shiftKey; });
        listen(field, 'blur', () => { h.shift = false; });
    }
    listen(textarea, 'select', () => {
        if (h.syncing || h.composing || document.activeElement !== textarea) return;
        const backward = textarea.selectionDirection === 'backward';
        const selStart = domToRaw(textarea.selectionStart, h.crlfs);
        const selEnd = domToRaw(textarea.selectionEnd, h.crlfs);
        send({ kind: 'selection', key: h.key, base: h.base, expected: h.value,
            anchor: backward ? selEnd : selStart,
            focus: backward ? selStart : selEnd });
    });
    // Synchronous copy support for read-only `El::selectable` selections (`Edit › Copy` / ⌘C).
    listen(document, 'copy', event => {
        const active = document.activeElement;
        if (!h.key && h.copyText && event.clipboardData && (active === canvas || active === document.body)) {
            event.preventDefault();
            event.clipboardData.setData('text/plain', h.copyText);
        }
    });
    // A file dropped while a Rune field has the keyboard is pasted into it, as a page's field
    // takes a drop: an image as a pasted image, text as text. Elsewhere a drop does nothing (the
    // browser would open the file in place of the app).
    listen(canvas, 'dragover', event => {
        if (!event.dataTransfer?.types.includes('Files')) return;
        event.preventDefault();
        event.dataTransfer.dropEffect = h.key ? 'copy' : 'none';
    });
    listen(canvas, 'drop', async event => {
        const file = event.dataTransfer?.files?.[0];
        if (!file) return;
        event.preventDefault();
        if (!h.key) return;
        if (file.type === 'image/png') {
            h.image = file;
            send({ kind: 'paste', text: '', html: '', image: true });
            return;
        }
        if (file.type.startsWith('text/') || file.size < 1048576 && !file.type) send({ kind: 'paste', text: await file.text(), html: '', image: false });
    });
    listen(canvas, 'pointerdown', event => event.preventDefault(), { capture: true });
    // WebKit on iOS 27 still fires a tap's compatibility mousedown to the canvas after a
    // prevented pointerdown; preventing it keeps the canvas's tabindex from taking focus
    // back from the hidden field that activate() just focused on pointerup.
    listen(canvas, 'mousedown', event => event.preventDefault(), { capture: true });
    // winit's web backend ignores pointercancel for mouse/pen and does not listen to
    // lostpointercapture; forward non-touch cancellations so held presses and drags never stick.
    listen(canvas, 'pointercancel', event => {
        if (event.pointerType !== 'touch') send({ kind: 'pointer_cancel', lost_capture: false });
    }, { passive: true });
    listen(canvas, 'lostpointercapture', event => {
        if (event.pointerType !== 'touch') send({ kind: 'pointer_cancel', lost_capture: true });
    }, { passive: true });
    listen(a11y, 'pointerleave', event => {
        if (event.pointerType !== 'touch' && event.relatedTarget !== canvas) send({ kind: 'leave' });
    }, { passive: true });
    listen(canvas, 'keydown', event => {
        if (page && !h.focused && SCROLL_KEYS.includes(event.key) && !event.metaKey && !event.ctrlKey) return;
        if (!h.mediaKeys && MEDIA_KEYS.includes(event.key)) return;
        if (!browserShortcut(event)) event.preventDefault();
    });
    if (page) {
        const scrolled = () => { h.lastScrollAt = performance.now(); send({ kind: 'scroll', y: window.scrollY }); };
        listen(window, 'scroll', scrolled, { passive: true });
        // A finger down or a page still coasting moves the export's layout under the reader; the
        // swap to the live mirror waits for it (see `dropStatic`).
        listen(window, 'touchstart', () => { h.touchDown = true; deferStatic(h); }, { passive: true });
        listen(window, 'touchend', () => { h.touchDown = false; deferStatic(h); }, { passive: true });
        listen(window, 'touchcancel', () => { h.touchDown = false; deferStatic(h); }, { passive: true });
        // Mirrored text and links take the pointer from the canvas; hover still reaches Rune.
        listen(a11y, 'pointermove', event => {
            if (event.pointerType !== 'touch') send({ kind: 'hover', x: event.clientX, y: event.clientY });
        }, { passive: true });
        queueMicrotask(() => { if (!h.disposed) scrolled(); });
    }
    // winit's web backend drops WheelEvent.DOM_DELTA_PAGE (deltaMode === 2); forward page
    // deltas to Rune before checking wheelUsed / preventDefault.
    const forwardPageWheel = event => {
        if (event.deltaMode !== 2) return;
        send({
            kind: 'wheel_page',
            dx: -event.deltaX,
            dy: -event.deltaY,
            shift: event.shiftKey,
            ctrl: event.ctrlKey,
            alt: event.altKey,
            meta: event.metaKey,
        });
    };
    if (!fitHeight) {
        // Ctrl+wheel (a touchpad's pinch) is the browser's zoom unless Rune pinched something with
        // it: Rune hears the wheel first (winit's listener is older) and says so (`syncWheel`).
        listen(canvas, 'wheel', event => {
            forwardPageWheel(event);
            const used = h.wheelUsed;
            h.wheelUsed = false;
            if (event.ctrlKey && !used) return;
            event.preventDefault();
        }, { passive: false });
        listen(document, 'gesturestart', event => event.preventDefault(), { passive: false });
    } else {
        listen(canvas, 'wheel', forwardPageWheel, { passive: true });
    }
    if (!page && !embedded) {
        // Printing prints what's on screen (the canvas, and the mirror's text over it, which the
        // PDF keeps selectable), fitted to the paper's width rather than cut off at its edge.
        const style = document.createElement('style');
        style.textContent = '@media print{canvas,[data-rune-a11y]{transform:scale(var(--rune-print-scale,1));transform-origin:0 0}}';
        document.head.append(style);
        listen(window, 'beforeprint', () => {
            // An A4 or Letter page's printable width at the browser's default margins, in CSS pixels.
            document.documentElement.style.setProperty('--rune-print-scale', String(Math.min(1, 718 / innerWidth)));
        });
        h.listeners.push(() => style.remove());
    }
    if (!page) {
        // Flush any deferred scroll or pending idle mirror update immediately when assistive
        // tech or a browser consumer asks: focus inside the mirror, print, or Find-in-page.
        listen(a11y, 'focusin', () => send({ kind: 'a11y_flush' }));
        listen(window, 'beforeprint', () => send({ kind: 'a11y_flush' }));
        listen(window, 'keydown', event => {
            const chord = (event.metaKey || event.ctrlKey) && !event.altKey && !event.getModifierState?.('AltGraph');
            if (chord && event.key?.toLowerCase() === 'f' && (!embedded || ownsTarget(event.target))) {
                send({ kind: 'a11y_flush' });
            }
        }, { capture: true, passive: true });
        // A web link takes the pointer while it's over it, as a page's would: the status bar
        // shows where it goes, and middle-click, Ctrl+click and its own menu open it.
        listen(canvas, 'pointermove', event => {
            if (event.pointerType === 'touch' || event.buttons) return;
            const a = linkAt(h, event.clientX, event.clientY);
            if (a && a !== h.hoverLink) armLink(h, a);
        }, { passive: true });
        // The pointer over a link (or anything else of the mirror's that took it) still hovers Rune.
        listen(a11y, 'pointermove', event => {
            if (event.pointerType !== 'touch') {
                const r = canvas.getBoundingClientRect();
                send({ kind: 'hover', x: event.clientX - r.left, y: event.clientY - r.top });
            }
        }, { passive: true });
    }
    // Right-click (and Ctrl+click on macOS): Rune draws an element's own menu; anywhere else the
    // browser opens its own over what a page would have there (the selection, a link, text, a text
    // field), never the canvas's image menu; Shift+right-click always opens the browser's. Armed
    // before the menu's event, which on Windows and Linux follows the release; macOS's follows the
    // press, which Rune has just heard (`syncSecondary`).
    const isSecondaryPress = event => event.button === 2 || (MAC && event.button === 0 && event.ctrlKey);
    const armAt = event => {
        if (!isSecondaryPress(event) || event.pointerType === 'touch' || event.target !== canvas) return;
        if (armMenu(h, event.clientX, event.clientY, event.shiftKey)) send({ kind: 'secondary_browser' });
    };
    if (MAC) listen(canvas, 'pointerdown', armAt);
    else listen(window, 'pointerup', armAt, { capture: true });
    listen(document, 'contextmenu', event => {
        const touchOrigin = event.target === canvas
            && h.activeTouchId !== null
            && event.button !== 2
            && (event.pointerType === 'touch' || event.pointerType === undefined || event.pointerType === '');
        if (touchOrigin) {
            event.preventDefault();
            const id = typeof event.pointerId === 'number' && event.pointerId > 0 ? event.pointerId : h.activeTouchId;
            send({ kind: 'touch_hold', id });
            return;
        }
        if (h.menu === 'rune' || event.target === canvas) event.preventDefault();
        if (h.menu === 'rune') h.menu = '';
    }, { capture: true });
    for (const name of ['pointerdown', 'keydown', 'wheel']) {
        listen(window, name, event => { if (name !== 'pointerdown' || !isSecondaryPress(event)) disarmMenu(h); }, { capture: true, passive: true });
    }
    queueMicrotask(() => {
        if (!h.disposed && !embedded && !page && document.activeElement === document.body) {
            (h.key ? h.field : canvas).focus({ preventScroll: true });
            if (h.key) reselect(h);
        }
    });
    return h;
}

// Reapplies the synced non-secure selection after focus moves to the hidden textarea (browsers
// collapse an unfocused textarea's selection to 0), without disturbing an active composition.
function reselect(h) {
    if (!h.selection || h.field !== h.textarea || h.composing) return;
    const [start, end, direction] = h.selection;
    if (h.textarea.selectionStart !== start || h.textarea.selectionEnd !== end || h.textarea.selectionDirection !== direction) {
        const syncing = h.syncing;
        h.syncing = true;
        try { h.textarea.setSelectionRange(start, end, direction); }
        finally { h.syncing = syncing; }
    }
}

export function sync(h, key, text, base, anchor, focus, secure) {
    if (h.composing && key === h.key) return;
    h.composing = false;
    h.syncing = true;
    const active = document.activeElement;
    const editing = active === h.textarea || active === h.password;
    const { dom, crlfs } = key && !secure ? normalizeTextarea(text) : { dom: '', crlfs: null };
    h.key = key; h.base = base; h.value = text; h.domValue = dom; h.crlfs = crlfs;
    h.password.value = '';
    h.field = secure ? h.password : h.textarea;
    if (!key) {
        h.selection = null;
        h.textarea.value = '';
        if (editing) h.canvas.focus({ preventScroll: true });
        h.textarea.inert = true;
        h.password.inert = true;
        h.textarea.inputMode = h.password.inputMode = coarse() ? 'none' : 'text';
    } else {
        h.field.inert = false;
        if (h.textarea.value !== dom) h.textarea.value = dom;
        if (secure) {
            h.selection = null;
        } else {
            const domAnchor = rawToDom(anchor, crlfs), domFocus = rawToDom(focus, crlfs);
            const start = Math.min(domAnchor, domFocus), end = Math.max(domAnchor, domFocus);
            const direction = domAnchor > domFocus ? 'backward' : 'forward';
            h.selection = [start, end, direction];
        }
        if (editing && active !== h.field) activate(h);
        else if (active === h.canvas) h.field.focus({ preventScroll: true });
        reselect(h);
        (secure ? h.textarea : h.password).inert = true;
    }
    h.syncing = false;
}

// Drops browser composition tracking after Rune finished/cancelled composition (e.g. on blur or hide).
export function cancelIme(h) {
    h.composing = false;
    h.composition = '';
}

export function syncCopy(h, text) {
    h.copyText = text;
}

// What the right-click Rune just heard is on: 'app' (an element's menu, which Rune draws),
// 'field', 'text' or '' (`Runtime::secondary_claim`).
export function syncSecondary(h, claim) {
    h.secondary = claim;
}

// Whether the app opted into hardware media transport keys (`App::media_keys`).
export function syncMediaKeys(h, enabled) {
    h.mediaKeys = !!enabled;
}

// The sans and mono slots' families (CSS lists): the mirror's text sets in them (`family`).
export function syncFamilies(h, sans, mono) {
    const css = (list, generic) => list.split(',').map(n => n.trim()).filter(Boolean).map(n => `"${n.replace(/["\\]/g, '')}"`).concat(generic).join(',');
    h.a11y.style.setProperty('--rune-sans', css(sans, 'sans-serif'));
    h.a11y.style.setProperty('--rune-mono', css(mono, 'monospace'));
}

// A pop-up menu (`Tx::popup_menu`, a field's menu with its spelling suggestions, selected
// text's Copy) as the page's own elements: `role="menu"` with keyboard, pointer and screen
// reader use, top-left at window point (x, y) of the canvas, at least `minWidth` wide, light or
// dark as the window draws. A pick or a dismissal is sent once: `{ kind: 'menu', id }`, `id`
// null when dismissed.
export function showMenu(h, json, x, y, minWidth, dark) {
    closeMenu(h, false);
    const spec = JSON.parse(json);
    const box = h.canvas.getBoundingClientRect();
    const colors = dark
        ? { bg: '#2b2b2b', fg: '#f2f2f2', muted: '#8f8f8f', line: '#454545', hover: '#3d3d3d' }
        : { bg: '#ffffff', fg: '#1a1a1a', muted: '#8c8c8c', line: '#e0e0e0', hover: '#ebebeb' };
    const menus = [];
    let answered = false;
    const answer = (id) => {
        if (answered) return;
        answered = true;
        closeMenu(h, true);
        h.send({ kind: 'menu', id });
    };
    const open = (spec, left, top, min, parentItem) => {
        const menu = document.createElement('div');
        menu.setAttribute('data-rune-menu', '');
        menu.setAttribute('role', 'menu');
        menu.tabIndex = -1;
        menu.style.cssText = `position:fixed;left:0;top:0;z-index:2147483647;box-sizing:border-box;min-width:${Math.max(min, 160)}px;max-height:calc(100vh - 8px);overflow-y:auto;padding:4px;margin:0;background:${colors.bg};color:${colors.fg};border:1px solid ${colors.line};border-radius:8px;box-shadow:0 4px 16px rgba(0,0,0,.18);font:13px/18px system-ui,sans-serif;user-select:none;-webkit-user-select:none;outline:none`;
        const rows = [];
        for (const entry of spec.items) {
            if (entry.separator) {
                const sep = document.createElement('div');
                sep.setAttribute('role', 'separator');
                sep.style.cssText = `height:1px;margin:4px 0;background:${colors.line}`;
                menu.append(sep);
                continue;
            }
            const row = document.createElement('div');
            const sub = entry.menu;
            const enabled = entry.enabled !== false;
            row.setAttribute('role', entry.checked ? 'menuitemcheckbox' : 'menuitem');
            if (entry.checked !== undefined && !sub) row.setAttribute('aria-checked', String(!!entry.checked));
            if (!enabled) row.setAttribute('aria-disabled', 'true');
            if (sub) row.setAttribute('aria-haspopup', 'menu');
            row.tabIndex = -1;
            row.style.cssText = `display:flex;align-items:center;gap:16px;padding:4px 10px 4px 26px;border-radius:4px;position:relative;cursor:default;white-space:nowrap;color:${enabled ? colors.fg : colors.muted};outline:none`;
            const title = document.createElement('span');
            title.style.flex = '1';
            title.textContent = sub ? entry.submenu : entry.title;
            row.append(title);
            if (entry.checked) {
                const tick = document.createElement('span');
                tick.textContent = '✓';
                tick.setAttribute('aria-hidden', 'true');
                tick.style.cssText = 'position:absolute;left:9px';
                row.append(tick);
            }
            if (entry.keyEquivalent || sub) {
                const hint = document.createElement('span');
                hint.setAttribute('aria-hidden', 'true');
                hint.textContent = sub ? '›' : entry.keyEquivalent.replace(/^mod\+/i, navigator.platform.startsWith('Mac') ? '⌘' : 'Ctrl+');
                hint.style.color = colors.muted;
                row.append(hint);
            }
            const item = { row, enabled, sub, id: entry.id };
            rows.push(item);
            row.addEventListener('pointerenter', () => focusRow(item, true));
            row.addEventListener('click', (event) => {
                event.preventDefault();
                if (!enabled) return;
                if (sub) { openSub(item); return; }
                answer(entry.id);
            });
            menu.append(row);
        }
        document.body.append(menu);
        // Inside the viewport: flipped left of the parent item, or up, where it would overflow.
        const r = menu.getBoundingClientRect();
        let l = left, t = top;
        if (l + r.width > innerWidth - 4) l = parentItem ? parentItem.row.getBoundingClientRect().left - r.width + 4 : innerWidth - 4 - r.width;
        // A menu at a point opens upward from it where it would run off the bottom, as a context
        // menu does; a submenu moves up.
        if (t + r.height > innerHeight - 4) t = Math.max(4, parentItem || top - r.height < 4 ? innerHeight - 4 - r.height : top - r.height);
        menu.style.left = `${Math.max(4, l)}px`;
        menu.style.top = `${Math.max(4, t)}px`;
        const level = { menu, rows, active: null, child: null, parentItem };
        menus.push(level);
        const focusRow = (item, fromPointer) => {
            for (const m of menus) if (m.rows.includes(item)) {
                if (m.active) m.active.row.style.background = '';
                m.active = item;
                if (item.enabled) item.row.style.background = colors.hover;
                item.row.focus({ preventScroll: true });
                // Leaving a submenu's item for another closes the submenu.
                if (m.child && m.child.parentItem !== item) closeFrom(menus.indexOf(m) + 1);
                if (fromPointer && item.sub && item.enabled) openSub(item);
            }
        };
        level.focusRow = focusRow;
        return level;
    };
    const closeFrom = (k) => {
        while (menus.length > k) {
            const m = menus.pop();
            m.menu.remove();
            if (menus.length) menus[menus.length - 1].child = null;
        }
    };
    const openSub = (item) => {
        const parent = menus.find(m => m.rows.includes(item));
        if (!parent || parent.child?.parentItem === item) return parent?.child;
        closeFrom(menus.indexOf(parent) + 1);
        const r = item.row.getBoundingClientRect();
        const child = open(item.sub, r.right - 4, r.top - 5, 0, item);
        parent.child = child;
        return child;
    };
    const top = open(spec, box.left + x, box.top + y, minWidth, null);
    const selected = spec.selected && top.rows.find(r => r.id === spec.selected);
    // A positioned menu puts its selected item over the point, as a pop-up button opens.
    if (selected) {
        const dy = selected.row.getBoundingClientRect().top - top.menu.getBoundingClientRect().top;
        top.menu.style.top = `${Math.max(4, box.top + y - dy)}px`;
    }
    top.menu.focus({ preventScroll: true });
    if (selected) top.focusRow(selected, false);
    const step = (level, by) => {
        const live = level.rows.filter(r => r.enabled);
        if (!live.length) return;
        const at = live.indexOf(level.active);
        level.focusRow(live[(at + by + live.length) % live.length], false);
    };
    const onKey = (event) => {
        const level = menus[menus.length - 1];
        const k = event.key;
        if (k === 'Escape' || k === 'Tab') {
            if (menus.length > 1 && k === 'Escape') { closeFrom(menus.length - 1); menus[menus.length - 1].active?.row.focus(); }
            else answer(null);
        } else if (k === 'ArrowDown') step(level, 1);
        else if (k === 'ArrowUp') step(level, -1);
        else if (k === 'Home') { level.active = null; step(level, 1); }
        else if (k === 'End') { level.active = null; step(level, -1); }
        else if (k === 'ArrowRight' && level.active?.sub) { const child = openSub(level.active); if (child) step(child, 1); }
        else if (k === 'ArrowLeft' && menus.length > 1) { closeFrom(menus.length - 1); menus[menus.length - 1].active?.row.focus(); }
        else if ((k === 'Enter' || k === ' ') && level.active?.enabled) {
            if (level.active.sub) { const child = openSub(level.active); if (child) step(child, 1); }
            else answer(level.active.id);
        } else if (k.length === 1 && /\S/.test(k)) {
            const live = level.rows.filter(r => r.enabled);
            const from = live.indexOf(level.active);
            const hit = [...live.slice(from + 1), ...live.slice(0, from + 1)].find(r => r.row.textContent.trim().toLowerCase().startsWith(k.toLowerCase()));
            if (hit) level.focusRow(hit, false);
        } else return;
        event.preventDefault();
        event.stopPropagation();
    };
    const onDown = (event) => {
        if (!menus.some(m => m.menu.contains(event.target))) answer(null);
    };
    const onBlur = () => answer(null);
    document.addEventListener('keydown', onKey, true);
    document.addEventListener('pointerdown', onDown, true);
    window.addEventListener('blur', onBlur);
    window.addEventListener('resize', onBlur);
    h.menu = { menus, done: () => {
        document.removeEventListener('keydown', onKey, true);
        document.removeEventListener('pointerdown', onDown, true);
        window.removeEventListener('blur', onBlur);
        window.removeEventListener('resize', onBlur);
        for (const m of menus) m.menu.remove();
        menus.length = 0;
    } };
}

// Takes the menu down; `refocus` gives the focused field or canvas its focus back, as a closing
// menu does.
function closeMenu(h, refocus) {
    if (typeof h.menu?.done !== 'function') return;
    const menu = h.menu;
    h.menu = null;
    menu.done();
    if (refocus) activate(h);
}

// The app went away or the menu was answered by Rune itself: no answer is sent.
export function hideMenu(h) {
    closeMenu(h, false);
}

// Whether Rune pinched something with the Ctrl+wheel it just heard.
export function syncWheel(h, used) {
    h.wheelUsed = used;
}

// The focused field's IME area in window points: the page's own text field sits there, so an
// input method's candidate window opens beside the caret, not in the page's corner.
export function syncCaret(h, x, y, w, height) {
    h.caret = [x, y, Math.max(1, w), Math.max(1, height)];
    if (h.fieldArmed) return;
    for (const field of [h.textarea, h.password]) placeField(field, h.caret);
}

function placeField(field, [x, y, w, height]) {
    Object.assign(field.style, { left: `${x}px`, top: `${y}px`, width: `${w}px`, height: `${height}px`, pointerEvents: 'none' });
}

// The mirror's web link under (x, y), if any.
function linkAt(h, x, y) {
    for (const a of h.a11y.querySelectorAll('a[target="_blank"]')) {
        for (const r of a.getClientRects()) if (x >= r.left && x < r.right && y >= r.top && y < r.bottom) return a;
    }
    return null;
}

function armLink(h, a) {
    disarmLink(h);
    h.hoverLink = a;
    a.style.pointerEvents = 'auto';
    a.style.cursor = 'pointer';
    a.addEventListener('pointerleave', () => disarmLink(h), { once: true });
}

function disarmLink(h) {
    if (!h.hoverLink) return;
    h.hoverLink.style.pointerEvents = 'none';
    h.hoverLink = null;
}

// Readies the browser's own menu for a right-click at (x, y), unless Rune's takes it. True when
// the browser's will open.
function armMenu(h, x, y, browser) {
    disarmMenu(h);
    const claim = h.secondary === 'app' && browser ? 'text' : h.secondary;
    h.secondary = '';
    if (claim === 'app') { h.menu = 'rune'; return false; }
    h.menu = 'browser';
    if (claim === 'field' && h.key) {
        // The page's field holds the Rune field's text and selection: its menu's Cut, Copy, Paste
        // and Select all edit them, as a page's text field's would.
        h.fieldArmed = true;
        placeField(h.field, [x - 2, y - 2, 4, 4]);
        h.field.style.pointerEvents = 'auto';
        return true;
    }
    if (h.copyText) {
        // The selection, as a page's: in a node under the pointer, selected, so the menu offers
        // Copy and a search for it.
        if (!h.pick) {
            h.pick = document.createElement('div');
            h.pick.setAttribute('data-rune-selection', '');
            const style = document.createElement('style');
            style.textContent = '[data-rune-selection]::selection{background:transparent;color:transparent}';
            document.head.append(style);
            document.body.append(h.pick);
        }
        h.pick.textContent = h.copyText;
        h.pick.style.cssText = `position:fixed;left:${x - 4}px;top:${y - 4}px;width:8px;height:8px;overflow:hidden;white-space:pre;color:transparent;forced-color-adjust:none;z-index:2;user-select:text;-webkit-user-select:text;`;
        const range = document.createRange();
        range.selectNodeContents(h.pick);
        const selection = getSelection();
        selection.removeAllRanges();
        selection.addRange(range);
        return true;
    }
    // Flush any deferred mirror update synchronously before hit-testing links/text for the menu.
    if (!h.page) h.send({ kind: 'a11y_flush' });
    // A link, the text under the pointer, or the page itself (Back, Reload, Print, Inspect).
    const el = linkAt(h, x, y) ?? textAt(h, x, y) ?? h.a11y;
    h.armed = [el, el.style.pointerEvents];
    el.style.pointerEvents = 'auto';
    return true;
}

function textAt(h, x, y) {
    let hit = null;
    for (const el of h.a11y.querySelectorAll('p, li, h1, h2, h3, h4, h5, h6')) {
        const r = el.getBoundingClientRect();
        if (x >= r.left && x < r.right && y >= r.top && y < r.bottom) hit = el;
    }
    return hit;
}

function disarmMenu(h) {
    if (h.armed) {
        const [el, before] = h.armed;
        el.style.pointerEvents = before;
        h.armed = null;
    }
    if (h.pick?.textContent) {
        const selection = getSelection();
        if (selection.anchorNode && h.pick.contains(selection.anchorNode)) selection.removeAllRanges();
        h.pick.textContent = '';
        h.pick.style.left = '-10000px';
    }
    if (h.fieldArmed) {
        h.fieldArmed = false;
        if (h.caret) placeField(h.field, h.caret);
        else Object.assign(h.field.style, { left: '0px', top: '0px', width: '1px', height: '1px', pointerEvents: 'none' });
    }
    if (h.menu === 'browser') h.menu = '';
}

function webLink(url) {
    return url.startsWith('https://') || url.startsWith('http://') || url.startsWith('mailto:');
}

// Matches Rust's `open_url::check_href`: allows http:, https:, mailto:, or a relative
// path/query/fragment that isn't scheme-relative (`//`).
function safeHref(url) {
    if (typeof url !== 'string') return null;
    const s = url.trim();
    if (!s || s.length > 8192 || /[\s\u0000-\u001f\u007f-\u009f\\"<>^`{|}]/.test(s)) return null;
    if (/%(?![0-9a-fA-F]{2})|%00/.test(s)) return null;
    const delim = s.search(/[:/?#]/);
    if (!s.startsWith('//') && (delim === -1 || s[delim] !== ':')) return s;
    const colon = s.indexOf(':');
    if (colon <= 0) return null;
    const scheme = s.slice(0, colon).toLowerCase();
    const rest = s.slice(colon + 1);
    if (scheme === 'http' || scheme === 'https') {
        if (!rest.startsWith('//')) return null;
        const after = rest.slice(2);
        const end = after.search(/[/?#]/);
        const auth = end === -1 ? after : after.slice(0, end);
        const tail = end === -1 ? '' : after.slice(end);
        if (!auth || /[\[\]]/.test(tail)) return null;
        const at = auth.lastIndexOf('@');
        const hp = at === -1 ? auth : auth.slice(at + 1);
        if (at !== -1 && (!auth.slice(0, at) || /[\[\]@]/.test(auth.slice(0, at)))) return null;
        if (!hp || hp.startsWith('.') || hp.includes('..')) return null;
        return `${scheme}:${rest}`;
    }
    if (scheme === 'mailto') {
        if (!rest || rest.startsWith('//') || /[\[\]]/.test(rest)) return null;
        return `${scheme}:${rest}`;
    }
    return null;
}

const TAGS = { heading: null, button: 'button', link: 'a', image: 'div', progressbar: 'div', text: 'p', scroll_x: 'div' };
const RANGE_ROLES = new Set(['slider', 'spinbutton', 'progressbar', 'meter', 'scrollbar']);

function family(name) {
    if (name === 'sans') return 'var(--rune-sans,sans-serif)';
    if (name === 'mono') return 'var(--rune-mono,monospace)';
    const clean = String(name).replace(/[\u0000-\u001f\u007f-\u009f"'\\;{}<>]/g, '').trim();
    return clean ? `"${clean}",var(--rune-sans,sans-serif)` : 'var(--rune-sans,sans-serif)';
}

// The mirror's first item element, after its stylesheet.
function firstItem(el) {
    let c = el.firstElementChild;
    while (c && c.tagName === 'STYLE') c = c.nextElementSibling;
    return c;
}

function runs(h, el, item) {
    for (const run of item.u) {
        const href = run.h ? safeHref(run.h) : null;
        if (!href) { el.append(document.createTextNode(run.t)); continue; }
        const a = document.createElement('a');
        a.textContent = run.t;
        if (run.d) a.setAttribute('aria-description', run.d);
        a.style.cssText = `color:transparent;text-decoration:none;pointer-events:${h.page ? 'auto' : 'none'};`;
        if (webLink(href)) {
            a.href = href;
            a.target = '_blank';
            a.rel = 'noopener noreferrer';
        } else {
            a.href = '#';
            a.addEventListener('click', ev => {
                ev.preventDefault();
                h.send({ kind: 'a11y_link', path: run.p, link: run.i });
            });
        }
        el.append(a);
    }
}

function round1(v) {
    const r = Math.round(v * 10) / 10;
    return r === 0 ? 0 : r;
}

function itemTag(item) {
    return item.k === 'group' ? (item.g || 'div') : item.k === 'heading' ? `h${item.l || 2}` : TAGS[item.k] ?? 'p';
}

function itemStyle(h, item, origin) {
    const [x, y, w, bh] = item.r;
    const left = round1(x - origin[0]);
    const top = round1(y - origin[1]);
    let css = `position:absolute;left:${left}px;top:${top}px;width:${w}px;height:${bh}px;margin:0;padding:0;border:0;background:transparent;color:transparent;`;
    if (item.k === 'group') return css + (item.sc ? 'overflow:hidden;list-style:none;pointer-events:none;' : 'overflow:visible;list-style:none;pointer-events:none;');
    // A page's horizontal scroller: the browser scrolls it natively, so a flick inside a code
    // block scrolls the block at the compositor's rate while a vertical drag still scrolls the
    // page.
    if (item.k === 'scroll_x') return css + 'overflow-x:auto;overflow-y:hidden;pointer-events:auto;touch-action:pan-x pan-y pinch-zoom;scrollbar-width:none;';
    if (item.k === 'link') return css + 'display:block;overflow:hidden;text-decoration:none;cursor:pointer;pointer-events:auto;font:14px/1.25 sans-serif;';
    if (item.k === 'image' || item.k === 'button' || item.k === 'progressbar') return css + 'overflow:hidden;pointer-events:none;font:14px/1.25 sans-serif;';
    const fs = item.s || 14;
    if (!item.f) return css + `font:${fs}px/1.25 sans-serif;overflow:hidden;white-space:pre-wrap;pointer-events:none;`;
    const [fam, weight, lh, tracking, italic, align] = item.f;
    css += `font:${italic ? 'italic ' : ''}${weight} ${fs}px/${lh}px ${family(fam)};letter-spacing:${tracking}px;text-align:${align};white-space:pre-wrap;overflow-wrap:break-word;overflow:visible;`;
    return css + (item.e ? 'pointer-events:auto;user-select:text;-webkit-user-select:text;cursor:text;' : 'pointer-events:none;user-select:none;-webkit-user-select:none;');
}

// Wraps an app-mode scroll container's children in a translated content layer so scroll settle
// updates one transform instead of rewriting `style.cssText` on every staying child.
function syncScrollContent(el, sc) {
    if (sc) {
        let wrap = el._scrollContent;
        if (!wrap) {
            wrap = document.createElement('div');
            wrap.setAttribute('role', 'none');
            wrap.setAttribute('data-rune-scroll-content', '');
            wrap.style.cssText = 'position:absolute;left:0;top:0;width:0;height:0;overflow:visible;pointer-events:none;';
            while (el.firstElementChild) wrap.insertBefore(el.firstElementChild, null);
            el.insertBefore(wrap, null);
            el._scrollContent = wrap;
        }
        const sx = sc[0] ? -sc[0] : 0;
        const sy = sc[1] ? -sc[1] : 0;
        const tr = `translate3d(${sx}px,${sy}px,0)`;
        if (wrap._tr !== tr) {
            wrap._tr = tr;
            wrap.style.transform = tr;
        }
    } else if (el._scrollContent) {
        const wrap = el._scrollContent;
        el._scrollContent = null;
        while (wrap.firstElementChild) el.insertBefore(wrap.firstElementChild, wrap);
        wrap.remove();
    }
}

function contentParent(el) {
    return el._scrollContent ?? el;
}

function ensureA11yNode(h, item, tag, key) {
    let el = h.a11yNodes.get(key);
    if (!el) {
        el = document.createElement(tag);
        if (tag === 'button') {
            el.type = 'button';
            el.tabIndex = h.page ? 0 : -1;
            el.addEventListener('click', ev => {
                ev.preventDefault();
                if (el.getAttribute('aria-disabled') === 'true') return;
                h.send({ kind: 'a11y_press', path: item.p });
            });
        }
        h.a11yNodes.set(key, el);
    }
    return el;
}

function applyA11yItem(h, el, item, tag, origin) {
    // Writing `cssText` invalidates the node's style and its layout: only do it when the
    // mirror's item actually moved or changed.
    const css = itemStyle(h, item, origin);
    if (el._css !== css) { el._css = css; el.style.cssText = css; }
    if (item.i) el.id = item.i; else el.removeAttribute('id');
    // `El::description` and `El::described_by`'s words: what a reader says after the name.
    if (item.d) el.setAttribute('aria-description', item.d); else el.removeAttribute('aria-description');
    // Role-carrying containers, live text (`status`, `alert`, `log`), images and progressbars.
    if (item.k === 'image') {
        if (el.getAttribute('role') !== 'img') el.setAttribute('role', 'img');
    } else if (tag !== 'button') {
        const role = item.o ?? (item.k === 'progressbar' ? 'progressbar' : null);
        if (role === null) {
            if (el.getAttribute('role') !== null) el.removeAttribute('role');
        } else if (el.getAttribute('role') !== role) {
            el.setAttribute('role', role);
        }
    }
    if (item.lv === undefined) {
        if (el.getAttribute('aria-live') !== null) el.removeAttribute('aria-live');
    } else if (el.getAttribute('aria-live') !== item.lv) {
        el.setAttribute('aria-live', item.lv);
    }
    const rangeAttrs = [
        ['aria-valuemin', item.rg ? String(item.rg[0]) : undefined],
        ['aria-valuemax', item.rg ? String(item.rg[1]) : undefined],
        ['aria-valuenow', item.rg ? String(item.rg[2]) : undefined],
        ['aria-valuetext', item.vt],
        ['aria-multiselectable', item.ms ? 'true' : undefined],
        ['aria-required', item.rq ? 'true' : undefined],
        ['aria-haspopup', item.hp],
        ['aria-orientation', item.or],
        ['aria-autocomplete', item.ac],
        ['aria-controls', item.ct],
        ['aria-owns', item.ow],
    ];
    for (const [name, value] of rangeAttrs) {
        if (value === undefined) {
            if (el.getAttribute(name) !== null) el.removeAttribute(name);
        } else if (el.getAttribute(name) !== value) {
            el.setAttribute(name, value);
        }
    }
    if (item.dis) {
        if (el.getAttribute('aria-disabled') !== 'true') el.setAttribute('aria-disabled', 'true');
    } else if (el.getAttribute('aria-disabled') !== null) {
        el.removeAttribute('aria-disabled');
    }
    if (item.b) {
        if (el.getAttribute('aria-busy') !== 'true') el.setAttribute('aria-busy', 'true');
    } else if (el.getAttribute('aria-busy') !== null) {
        el.removeAttribute('aria-busy');
    }
    if (item.dir) {
        if (el.getAttribute('dir') !== item.dir) el.setAttribute('dir', item.dir);
    } else if (item.k !== 'scroll_x' && el.getAttribute('dir') !== null) {
        el.removeAttribute('dir');
    }
    if (item.x === true || item.x === false) {
        const v = String(item.x);
        if (el.getAttribute('aria-expanded') !== v) el.setAttribute('aria-expanded', v);
    } else if (el.getAttribute('aria-expanded') !== null) {
        el.removeAttribute('aria-expanded');
    }
    if (item.k !== 'heading') {
        if (item.l) {
            const v = String(item.l);
            if (el.getAttribute('aria-level') !== v) el.setAttribute('aria-level', v);
        } else if (el.getAttribute('aria-level') !== null) {
            el.removeAttribute('aria-level');
        }
    }
    if (tag === 'button') {
        // A control's role and states (a palette's option, a tab, a switch, a toggle button, a range control).
        const states = [['role', item.o], ['aria-selected', item.se === undefined ? undefined : String(item.se)],
            ['aria-checked', item.c], ['aria-pressed', item.pr],
            ['aria-posinset', item.n ? String(item.n[0]) : undefined], ['aria-setsize', item.n ? String(item.n[1]) : undefined]];
        for (const [name, value] of states) {
            if (value === undefined) el.removeAttribute(name);
            else if (el.getAttribute(name) !== value) el.setAttribute(name, value);
        }
    }
    const combobox = item.k === 'button' && item.o === 'combobox';
    const rangeControl = item.k === 'button' && (item.o === 'slider' || item.o === 'spinbutton');
    if (item.k === 'group' || item.k === 'image' || item.k === 'progressbar' || combobox || rangeControl) {
        if (item.t) {
            if (el.getAttribute('aria-label') !== item.t) el.setAttribute('aria-label', item.t);
        } else if (el.getAttribute('aria-label') !== null) {
            el.removeAttribute('aria-label');
        }
    } else if (item.k === 'button') {
        if (el.getAttribute('aria-label') !== null) el.removeAttribute('aria-label');
    } else if (item.k === 'link') {
        const href = safeHref(item.h);
        if (href) {
            el.href = href;
            if (!h.page && webLink(href)) { el.target = '_blank'; el.rel = 'noopener noreferrer'; }
            else { el.removeAttribute('target'); el.removeAttribute('rel'); }
        } else {
            el.removeAttribute('href');
            el.removeAttribute('target');
            el.removeAttribute('rel');
        }
    }
    if (item.k === 'scroll_x') {
        if (el.getAttribute('data-rune-scroll-x') === null) el.setAttribute('data-rune-scroll-x', '');
        if (!el._scrollXBound) {
            el._scrollXBound = true;
            // The browser scrolls this container itself; Rune draws the content where the
            // container says it is.
            el.addEventListener('scroll', () => {
                const path = el._path;
                if (path && !h.disposed) h.send({ kind: 'scroll_x', path, x: readScrollX(el) });
            }, { passive: true });
        }
        if (el._path !== item.p) { el._path = item.p; h.scrollEls.set(item.p, el); }
    } else {
        if (el.getAttribute('data-rune-scroll-x') !== null) el.removeAttribute('data-rune-scroll-x');
        if (el._path && h.scrollEls.get(el._path) === el) { h.scrollEls.delete(el._path); el._path = undefined; }
    }
    syncScrollContent(el, item.k === 'group' ? item.sc : null);
    const container = item.k === 'group' || item.k === 'scroll_x';
    const noContent = container || item.k === 'image' || item.k === 'progressbar';
    const content = combobox ? (item.v ?? '') : rangeControl ? '' : (item.t ?? '');
    const sig = noContent ? '' : item.u?.length ? JSON.stringify(item.u) : content;
    if (!noContent && el._sig !== sig) {
        el._sig = sig;
        if (item.u?.length) {
            el.replaceChildren();
            runs(h, el, item);
        } else {
            if (el.firstElementChild) el.replaceChildren();
            el.textContent = content;
        }
    }
    return container;
}

function syncA11yPatch(h, patch) {
    if (patch.dir !== undefined) {
        if (patch.dir) {
            if (h.a11y.getAttribute('dir') !== patch.dir) h.a11y.setAttribute('dir', patch.dir);
        } else if (h.a11y.getAttribute('dir') !== null) {
            h.a11y.removeAttribute('dir');
        }
    }
    if (patch.lang !== undefined) {
        if (patch.lang) {
            if (h.a11y.getAttribute('lang') !== patch.lang) h.a11y.setAttribute('lang', patch.lang);
        } else if (h.a11y.getAttribute('lang') !== null) {
            h.a11y.removeAttribute('lang');
        }
    }
    if (patch.d) {
        for (const key of patch.d) {
            const el = h.a11yNodes.get(key);
            if (!el) continue;
            if (el._path && h.scrollEls.get(el._path) === el) h.scrollEls.delete(el._path);
            el.remove();
            h.a11yNodes.delete(key);
        }
    }
    if (patch.u) {
        for (const item of patch.u) {
            const tag = itemTag(item);
            const key = `${tag}:${item.p}`;
            const el = ensureA11yNode(h, item, tag, key);
            applyA11yItem(h, el, item, tag, [0, 0]);
        }
    }
    if (patch.o) {
        for (const [parentKey, childKeys] of Object.entries(patch.o)) {
            const host = parentKey === '' ? h.a11y : h.a11yNodes.get(parentKey);
            if (!host) continue;
            const container = parentKey === '' ? h.a11y : contentParent(host);
            let cursor = parentKey === '' ? firstItem(container) : container.firstElementChild;
            for (const childKey of childKeys) {
                const childEl = h.a11yNodes.get(childKey);
                if (!childEl) continue;
                if (cursor !== childEl) container.insertBefore(childEl, cursor);
                else cursor = cursor.nextElementSibling;
            }
        }
    }
}

export function syncA11y(h, json) {
    if (h.disposed) return;
    const payload = JSON.parse(json);
    if (payload && !Array.isArray(payload) && payload.p === 1) {
        syncA11yPatch(h, payload);
        return;
    }
    const items = payload;
    const seen = new Set();
    const els = [];
    const cursors = new Map([[h.a11y, firstItem(h.a11y)]]);
    for (const item of items) {
        const tag = itemTag(item);
        const key = `${tag}:${item.p}`;
        seen.add(key);
        const el = ensureA11yNode(h, item, tag, key);
        const parent = item.a === undefined ? null : items[item.a];
        const parentHost = item.a === undefined ? h.a11y : els[item.a];
        const parentEl = item.a === undefined ? h.a11y : contentParent(parentHost);
        const container = applyA11yItem(h, el, item, tag, parent ? parent.r : [0, 0]);
        els.push(el);
        if (container) {
            const target = contentParent(el);
            if (!cursors.has(target)) cursors.set(target, target.firstElementChild);
        }
        const cursor = cursors.get(parentEl) ?? null;
        if (cursor !== el) parentEl.insertBefore(el, cursor);
        else cursors.set(parentEl, cursor.nextElementSibling);
    }
    for (const [key, el] of h.a11yNodes) {
        if (!seen.has(key)) {
            if (el._path && h.scrollEls.get(el._path) === el) h.scrollEls.delete(el._path);
            el.remove();
            h.a11yNodes.delete(key);
        }
    }
}

// An app's focus (`Runtime::web_a11y_focus`), for a reader: the browser's own focus stays on the
// canvas (where keys go) or its text field, so the canvas is an `application` whose active
// descendant is a proxy in its fallback content saying the focused element's role, name and
// states. Two proxies, taking turns, so every move is a change of active descendant a reader
// follows, even between two elements alike. A text field's name goes on the field the browser
// edits.
export function focusA11y(h, json) {
    if (h.disposed || h.page) return;
    const f = json ? JSON.parse(json) : null;
    if (!h.focusProxies) {
        h.focusProxies = [0, 1].map(n => {
            const el = document.createElement('div');
            el.id = `rune-focus-${n}`;
            el.tabIndex = -1;
            h.canvas.append(el);
            return el;
        });
        h.focusTurn = 0;
        h.canvas.setAttribute('role', 'application');
        if (document.title) h.canvas.setAttribute('aria-label', document.title);
    }
    if (!f) {
        h.canvas.removeAttribute('aria-activedescendant');
        h.textarea.removeAttribute('aria-autocomplete');
        return;
    }
    if (f.o === 'textbox') h.textarea.setAttribute('aria-label', f.t || 'Text input');
    if (f.f) h.textarea.setAttribute('aria-label', f.f);
    h.focusTurn ^= 1;
    const el = h.focusProxies[h.focusTurn];
    for (const a of [...el.attributes]) if (a.name === 'role' || a.name.startsWith('aria-')) el.removeAttribute(a.name);
    el.textContent = '';
    el.setAttribute('role', f.o);
    el.setAttribute('aria-label', f.t ?? '');
    if (f.d) el.setAttribute('aria-description', f.d);
    if (f.c) el.setAttribute('aria-checked', f.c);
    if (f.pr) el.setAttribute('aria-pressed', f.pr);
    if (f.s !== undefined) el.setAttribute('aria-selected', String(f.s));
    if (f.x !== undefined) el.setAttribute('aria-expanded', String(f.x));
    if (f.n) { el.setAttribute('aria-posinset', String(f.n[0])); el.setAttribute('aria-setsize', String(f.n[1])); }
    if (f.l) el.setAttribute('aria-level', String(f.l));
    if (f.dis) el.setAttribute('aria-disabled', 'true');
    if (f.b) el.setAttribute('aria-busy', 'true');
    if (f.ms) el.setAttribute('aria-multiselectable', 'true');
    if (f.rq) el.setAttribute('aria-required', 'true');
    if (f.hp) el.setAttribute('aria-haspopup', f.hp);
    if (f.or) el.setAttribute('aria-orientation', f.or);
    if (f.ac) el.setAttribute('aria-autocomplete', f.ac);
    if (f.ct) el.setAttribute('aria-controls', f.ct);
    if (f.ow) el.setAttribute('aria-owns', f.ow);
    if (f.rg) {
        el.setAttribute('aria-valuemin', String(f.rg[0]));
        el.setAttribute('aria-valuemax', String(f.rg[1]));
        el.setAttribute('aria-valuenow', String(f.rg[2]));
    }
    if (f.vt) {
        el.setAttribute('aria-valuetext', f.vt);
    } else if (f.v) {
        if (RANGE_ROLES.has(f.o)) el.setAttribute('aria-valuetext', f.v);
        else el.textContent = f.v;
    }
    h.canvas.setAttribute('aria-activedescendant', el.id);
    // Typing in a field whose highlighted row this is (the palette): the field says it too.
    if (f.o !== 'textbox') h.textarea.setAttribute('aria-activedescendant', el.id);
    else h.textarea.removeAttribute('aria-activedescendant');
    const taAc = f.fac ?? ((f.o === 'textbox' || f.o === 'combobox') ? f.ac : undefined);
    if (taAc) h.textarea.setAttribute('aria-autocomplete', taAc);
    else h.textarea.removeAttribute('aria-autocomplete');
}

// `Tx::announce`: two live regions in the mirror, polite and assertive, made the first time
// something is said (a region must be in the page before its text changes for a reader to say
// it). The words go in on the next frame, after the region is emptied, so the same words said
// twice are said twice.
export function announceA11y(h, text, assertive) {
    if (h.disposed) return;
    h.live ??= {};
    const kind = assertive ? 'assertive' : 'polite';
    let el = h.live[kind];
    if (!el) {
        el = document.createElement('div');
        el.setAttribute('aria-live', kind);
        el.setAttribute('aria-atomic', 'true');
        if (!assertive) el.setAttribute('role', 'status');
        el.style.cssText = 'position:absolute;width:1px;height:1px;margin:-1px;overflow:hidden;clip-path:inset(50%);white-space:nowrap;';
        h.a11y.parentNode.insertBefore(el, h.a11y);
        h.live[kind] = el;
    }
    el.textContent = '';
    requestAnimationFrame(() => { if (!h.disposed) el.textContent = text; });
}

// Pops the oldest captured PointerEvent.timeStamp (ms since timeOrigin) for touch pointer `id`,
// clamped to performance.now(), or NaN when absent, non-finite, or non-positive.
export function takeTouchTimestamp(h, id) {
    const idx = h.touchTimestamps.findIndex(entry => entry.id === id);
    if (idx < 0) return Number.NaN;
    const [{ ts }] = h.touchTimestamps.splice(idx, 1);
    if (typeof ts !== 'number' || !Number.isFinite(ts) || ts <= 0) return Number.NaN;
    const now = performance.now();
    return Number.isFinite(now) && now > 0 ? Math.min(ts, now) : ts;
}

export function performanceNow() {
    return performance.now();
}

// Cancels any pending touch-hold confirmation frames or trailing macrotask.
export function cancelTouchHoldConfirm(h) {
    const pending = h.touchHoldConfirm;
    if (!pending) return;
    h.touchHoldConfirm = null;
    pending.cancelled = true;
    if (pending.raf && typeof globalThis.cancelAnimationFrame === 'function') globalThis.cancelAnimationFrame(pending.raf);
    if (pending.controller) pending.controller.abort();
    if (pending.timer) clearTimeout(pending.timer);
}

// Waits two animation frames and a trailing low-priority macrotask (re-yielding up to 2 turns if
// input is pending) before confirming a due touch hold. After a main-thread stall, the first rAF
// commits any overdue frame from the stall (flushing compositor/browser acks), the second rAF
// runs on a fresh post-stall BeginMainFrame after rAF-aligned touchmove and discrete pointerup
// delivery, and the trailing task lets any remaining input drain (~33 ms on 60 Hz, ~17 ms on
// 120 Hz for an unstalled hold).
export function scheduleTouchHoldConfirm(h, attempt) {
    cancelTouchHoldConfirm(h);
    if (h.disposed) return;
    const token = { cancelled: false, raf: 0, controller: null, timer: 0 };
    h.touchHoldConfirm = token;
    let yields = 0;
    const isCancelled = () => token.cancelled || h.disposed || h.touchHoldConfirm !== token;
    const run = () => {
        token.controller = null;
        token.timer = 0;
        if (isCancelled()) return;
        if (yields < 2 && globalThis.navigator?.scheduling?.isInputPending?.({ includeContinuous: true })) {
            yields += 1;
            scheduleTask();
            return;
        }
        h.touchHoldConfirm = null;
        h.send({ kind: 'touch_hold_confirm', attempt });
    };
    const scheduleTask = () => {
        if (typeof globalThis.scheduler?.postTask === 'function') {
            const controller = typeof AbortController === 'function' ? new AbortController() : null;
            token.controller = controller;
            const opts = controller ? { priority: 'background', signal: controller.signal } : { priority: 'background' };
            Promise.resolve(globalThis.scheduler.postTask(run, opts)).catch(() => {});
        } else {
            token.timer = setTimeout(run, 0);
        }
    };
    if (typeof globalThis.requestAnimationFrame === 'function') {
        token.raf = globalThis.requestAnimationFrame(() => {
            token.raf = 0;
            if (isCancelled()) return;
            token.raf = globalThis.requestAnimationFrame(() => {
                token.raf = 0;
                if (isCancelled()) return;
                scheduleTask();
            });
        });
    } else {
        scheduleTask();
    }
}

// Cancels any pending post-scroll settle timer or post-frame idle mirror flush.
export function cancelA11y(h) {
    if (h.a11yTimer) {
        clearTimeout(h.a11yTimer);
        h.a11yTimer = 0;
    }
    if (h.a11yIdle) {
        if (h.a11yIdle.kind === 'idle' && globalThis.cancelIdleCallback) cancelIdleCallback(h.a11yIdle.id);
        else clearTimeout(h.a11yIdle.id);
        h.a11yIdle = null;
    }
}

// Schedules a post-frame idle mirror flush off `rAF` (`requestIdleCallback` with bounded timeout,
// falling back to `setTimeout(..., 0)`).
export function scheduleA11yIdle(h, attempt = 0, timeoutMs = 100) {
    cancelA11y(h);
    const due = () => {
        h.a11yIdle = null;
        if (!h.disposed) h.send({ kind: 'a11y_idle', attempt });
    };
    if (globalThis.requestIdleCallback) {
        h.a11yIdle = { kind: 'idle', id: requestIdleCallback(due, { timeout: timeoutMs }) };
    } else {
        h.a11yIdle = { kind: 'timeout', id: setTimeout(due, 0) };
    }
}

// Arms or resets the post-scroll settle timer so mirror updates wait until scrolling rests.
export function scheduleA11y(h, attempt = 0, delayMs = 150) {
    cancelA11y(h);
    h.a11yTimer = setTimeout(() => {
        h.a11yTimer = 0;
        if (!h.disposed) h.send({ kind: 'a11y_settle', attempt });
    }, delayMs);
}

// A page's height and whether a Rune control has the keyboard (scroll keys are the page's
// when none has).
export function syncPage(h, height, focused) {
    h.focused = focused;
    h.a11y.style.height = `${height}px`;
    // The clipper is as tall as the page, so the band's overscan past the document's end is
    // clipped instead of making the page scrollable past its foot.
    if (h.band && h.band.style.height !== `${height}px`) h.band.style.height = `${height}px`;
}

// Detects how this browser represents `Element.scrollLeft` on a `direction: rtl` container:
// - 'negative': 0 at the right (start) edge, -(scrollWidth - clientWidth) at the left (end) edge
//   (CSSOM View standard: Chromium >= 85, Firefox, Safari >= 15.4).
// - 'reverse':  0 at the right (start) edge, +(scrollWidth - clientWidth) at the left (end) edge
//   (legacy Edge / IE).
// - 'default':  +(scrollWidth - clientWidth) at the right (start) edge, 0 at the left (end) edge
//   (legacy Blink / WebKit).
let rtlScrollType = null;
function detectRtlScrollType() {
    if (rtlScrollType) return rtlScrollType;
    const host = document.body ?? document.documentElement;
    if (!host) return 'negative';
    const outer = document.createElement('div');
    outer.dir = 'rtl';
    outer.style.cssText = 'position:fixed;top:-9999px;left:-9999px;width:40px;height:40px;overflow-x:scroll;overflow-y:hidden;scrollbar-width:none;visibility:hidden;pointer-events:none;';
    const inner = document.createElement('div');
    inner.style.cssText = 'width:80px;height:1px;';
    outer.append(inner);
    host.append(outer);
    let type = 'negative';
    if (outer.scrollWidth > outer.clientWidth) {
        if (outer.scrollLeft > 0) {
            type = 'default';
        } else {
            outer.scrollLeft = 1;
            type = outer.scrollLeft === 0 ? 'negative' : 'reverse';
        }
        rtlScrollType = type;
    }
    outer.remove();
    return type;
}

function isRtl(el) {
    if (el.dir === 'rtl') return true;
    if (el.dir === 'ltr') return false;
    return getComputedStyle(el).direction === 'rtl';
}

// Normalizes `el.scrollLeft` into Rune's logical horizontal scroll offset (0 at the inline-start
// edge, growing toward the inline-end edge up to `scrollWidth - clientWidth`).
function readScrollX(el) {
    const raw = el.scrollLeft;
    if (!Number.isFinite(raw)) return 0;
    const max = Math.max(0, el.scrollWidth - el.clientWidth);
    if (!isRtl(el)) return Math.min(Math.max(raw, 0), max);
    const type = detectRtlScrollType();
    const logical = type === 'negative' ? -raw : type === 'default' ? max - raw : raw;
    return Math.min(Math.max(logical, 0), max);
}

// Converts Rune's logical horizontal scroll offset (`0..=max`) into the DOM `scrollLeft` value
// expected by `el`'s direction and browser RTL scroll model.
function domScrollLeft(el, x) {
    if (!Number.isFinite(x)) return 0;
    const max = Math.max(0, el.scrollWidth - el.clientWidth);
    const clamped = Math.min(Math.max(x, 0), max);
    if (!isRtl(el)) return clamped;
    const type = detectRtlScrollType();
    if (type === 'negative') return clamped === 0 ? 0 : -clamped;
    if (type === 'default') return max - clamped;
    return clamped;
}

// A horizontal viewport Rune scrolled itself (a wheel, a key): move the mirror's container to
// match, so the two stay the same text. The container's own scroll event echoes back the same
// offset, which Rune ignores.
export function syncScrollX(h, path, x, rtl) {
    const el = h.scrollEls.get(path);
    if (!el) return;
    if (rtl !== undefined) {
        const dir = rtl ? 'rtl' : 'ltr';
        if (el.dir !== dir) el.dir = dir;
    }
    const target = domScrollLeft(el, x);
    if (Math.abs(el.scrollLeft - target) > 0.5) el.scrollLeft = target;
}

// Rune's first frame is on screen: the canvas announces `runepresent`, and a page swaps its
// static export (`[data-rune-static]`) for the live mirror, keeping the section that was at
// the top of the window where it was. The export is hidden, not removed: removing the
// element a browser counted as the largest contentful paint takes the metric away. It goes
// two frames later (the canvas covers it by then), as a phone may not have painted the page
// before Rune's first frame, and content never painted can't count at all.
export function presented(h) {
    if (h.disposed || h.presented) return;
    h.presented = true;
    h.canvas.dispatchEvent(new CustomEvent('runepresent', { bubbles: true }));
    if (h.page) requestAnimationFrame(() => requestAnimationFrame(() => { if (!h.disposed) dropStatic(h); }));
}

function dropStatic(h) {
    if (h.disposed || h.swapped) return;
    const parts = [...document.querySelectorAll('[data-rune-static]')];
    const exported = parts.find((el) => el.tagName !== 'STYLE');
    if (!exported) { h.swapped = true; return; }
    // A finger is down, or the page is still coasting: hiding the export would take it out of the
    // page's flow under a scroll that is under way, and iOS stops momentum dead when the scroll
    // position moves underneath it. The touchend or the next scroll arms the retry.
    if (h.touchDown) return;
    if (performance.now() - h.lastScrollAt < 120) { deferStatic(h); return; }
    h.swapped = true;
    let anchor = null, offset = 0;
    if (window.scrollY > 0) {
        for (const el of exported.querySelectorAll('[id]')) {
            const r = el.getBoundingClientRect();
            if (r.bottom > 0) { anchor = el.id; offset = r.top; break; }
        }
    }
    for (const el of parts) {
        if (el.tagName === 'STYLE') { el.remove(); continue; }
        // Out of the page's flow, find, focus and accessibility tree, its ids the mirror's.
        el.style.cssText = 'position:absolute;top:0;left:0;width:0;height:0;overflow:hidden;visibility:hidden;';
        el.inert = true;
        el.setAttribute('aria-hidden', 'true');
        el.removeAttribute('id');
        for (const named of el.querySelectorAll('[id]')) named.removeAttribute('id');
        el.removeAttribute('data-rune-static');
        el.setAttribute('data-rune-static-hidden', '');
    }
    const live = anchor && h.a11y.querySelector(`#${CSS.escape(anchor)}`);
    if (live) {
        const delta = live.getBoundingClientRect().top - offset;
        // The export's CSS and the live layout differ by a pixel or two here and there; taking the
        // page's scroll for that would stop a flick for nothing.
        if (Math.abs(delta) > 2) window.scrollTo({ top: window.scrollY + delta, behavior: 'instant' });
    }
}

// Swaps once the page has settled. Only a scroll or a touch arms it, so a still page doesn't poll.
function deferStatic(h) {
    if (h.staticTimer || h.swapped || !h.presented || h.disposed) return;
    h.staticTimer = setTimeout(() => { h.staticTimer = 0; if (!h.disposed) dropStatic(h); }, 150);
}

export function activate(h) {
    // Keep DOM focus inside an open Rune popup menu until it closes.
    if (typeof h.menu?.done === 'function') return;
    // A page's canvas is `tabindex="-1"`: focusing it on a tap would only drop the reader's text
    // selection or a link's focus. An input that took the keyboard still gets it.
    if (h.page && !h.key) return;
    const field = h.key ? h.field : h.canvas;
    if (h.key) field.inputMode = 'text';
    if (document.activeElement !== field) field.focus({ preventScroll: true });
    if (h.key) reselect(h);
}

export async function pastedImage(h, max) {
    const file = h.image;
    h.image = undefined;
    return file && file.size <= max ? new Uint8Array(await file.arrayBuffer()) : undefined;
}

export function destroy(h) {
    closeMenu(h, false);
    h.disposed = true;
    h.image = undefined;
    clearTimeout(h.staticTimer);
    cancelTouchHoldConfirm(h);
    h.touchTimestamps.length = 0;
    h.activeTouchId = null;
    cancelA11y(h);
    for (const remove of h.listeners) remove();
    h.password.value = ''; h.textarea.value = ''; h.value = ''; h.domValue = ''; h.crlfs = null; h.copyText = ''; h.selection = null;
    h.a11yNodes.clear();
    h.scrollEls.clear();
    h.a11y.remove();
    h.band?.remove();
    h.password.remove(); h.textarea.remove();
}
