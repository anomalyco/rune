// Browser text services and semantic DOM accessibility mirror.
const coarse = () => matchMedia('(pointer: coarse)').matches;
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

function browserShortcut(event) {
    const chord = event.metaKey || event.ctrlKey;
    if (event.key === 'F5' || event.key === 'F12') return true;
    if (!chord) return false;
    const k = event.key.toLowerCase();
    // Leave copy ('c') un-prevented too so the browser fires its synchronous `copy` event.
    return ['c', 'r', 'l', 't', 'w', 'n', 'f', 'g', '=', '+', '-', '0'].includes(k)
        || (event.altKey && ['i', 'j', 'u'].includes(k));
}

// Keys that scroll a page when nothing in Rune has focus.
const SCROLL_KEYS = [' ', 'PageUp', 'PageDown', 'Home', 'End', 'ArrowUp', 'ArrowDown'];

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
    const password = document.createElement('input');
    password.type = 'password';
    password.setAttribute('aria-label', 'Password input');
    password.autocomplete = 'off';
    password.setAttribute('autocorrect', 'off');
    password.setAttribute('autocapitalize', 'off');
    password.spellcheck = false;
    for (const field of [textarea, password]) {
        field.style.cssText = 'position:fixed;left:0;top:0;width:1px;height:1px;padding:0;border:0;opacity:0;font-size:16px;resize:none;pointer-events:none;';
        field.tabIndex = -1;
        field.inputMode = coarse() ? 'none' : 'text';
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
    const h = { canvas, textarea, password, a11y, a11yNodes: new Map(), field: textarea, key: '', base: 0, value: '',
        copyText: '', composing: false, composition: '', shift: false, listeners: [], syncing: false, disposed: false,
        embedded, fitHeight, page, focused: false, presented: false, swapped: false, touchDown: false,
        lastScrollAt: 0, staticTimer: 0, scrollEls: new Map(), band };
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
    function changed(preedit = false) {
        const backward = textarea.selectionDirection === 'backward';
        send({ kind: preedit ? 'preedit' : 'edit', key: h.key, base: h.base, expected: h.value,
            ...replacement(h.value, textarea.value, preedit ? h.composition : '', textarea.selectionEnd),
            anchor: backward ? textarea.selectionEnd : textarea.selectionStart,
            focus: backward ? textarea.selectionStart : textarea.selectionEnd });
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
    listen(password, 'input', () => {
        const text = password.value;
        password.value = '';
        if (text) send({ kind: 'secure', key: h.key, text });
    });
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
            }
        });
        listen(field, 'keydown', event => {
            h.shift = event.shiftKey;
            if (event.isComposing || h.composing || event.keyCode === 229) return;
            const chord = event.metaKey || event.ctrlKey;
            const editing = ['Enter', 'Tab', 'Escape', 'ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End', 'PageUp', 'PageDown'];
            if (field === password) editing.push('Backspace', 'Delete');
            if (!chord && !editing.includes(event.key)) return;
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
        send({ kind: 'selection', key: h.key, base: h.base, expected: h.value,
            anchor: backward ? textarea.selectionEnd : textarea.selectionStart,
            focus: backward ? textarea.selectionStart : textarea.selectionEnd });
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
    listen(canvas, 'keydown', event => {
        if (page && !h.focused && SCROLL_KEYS.includes(event.key) && !event.metaKey && !event.ctrlKey) return;
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
            if (event.pointerType === 'mouse') send({ kind: 'hover', x: event.clientX, y: event.clientY });
        }, { passive: true });
        queueMicrotask(() => { if (!h.disposed) scrolled(); });
    }
    if (!fitHeight) {
        // Ctrl+wheel (a touchpad's pinch) is the browser's zoom unless Rune pinched something with
        // it: Rune hears the wheel first (winit's listener is older) and says so (`syncWheel`).
        listen(canvas, 'wheel', event => {
            const used = h.wheelUsed;
            h.wheelUsed = false;
            if (event.ctrlKey && !used) return;
            event.preventDefault();
        }, { passive: false });
        listen(document, 'gesturestart', event => event.preventDefault(), { passive: false });
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
        // A web link takes the pointer while it's over it, as a page's would: the status bar
        // shows where it goes, and middle-click, Ctrl+click and its own menu open it.
        listen(canvas, 'pointermove', event => {
            if (event.pointerType !== 'mouse' || event.buttons) return;
            const a = linkAt(h, event.clientX, event.clientY);
            if (a && a !== h.hoverLink) armLink(h, a);
        }, { passive: true });
        // The pointer over a link (or anything else of the mirror's that took it) still hovers Rune.
        listen(a11y, 'pointermove', event => {
            if (event.pointerType === 'mouse') send({ kind: 'hover', x: event.clientX, y: event.clientY });
        }, { passive: true });
    }
    // Right-click: Rune draws an element's own menu; anywhere else the browser opens its own over
    // what a page would have there (the selection, a link, text, a text field), never the
    // canvas's image menu; Shift+right-click always opens the browser's. Armed before the menu's
    // event, which on Windows and Linux follows the release; macOS's follows the press, which
    // Rune has just heard (`syncSecondary`).
    const armAt = event => {
        if (event.button !== 2 || event.pointerType !== 'mouse' || event.target !== canvas) return;
        if (armMenu(h, event.clientX, event.clientY, event.shiftKey)) send({ kind: 'secondary_browser' });
    };
    if (/Mac|iP/.test(navigator.platform)) listen(canvas, 'pointerdown', armAt);
    else listen(window, 'pointerup', armAt, { capture: true });
    listen(document, 'contextmenu', event => {
        if (h.menu === 'rune' || event.target === canvas) event.preventDefault();
        if (h.menu === 'rune') h.menu = '';
    }, { capture: true });
    for (const name of ['pointerdown', 'keydown', 'wheel']) {
        listen(window, name, event => { if (name !== 'pointerdown' || event.button !== 2) disarmMenu(h); }, { capture: true, passive: true });
    }
    queueMicrotask(() => {
        if (!h.disposed && !embedded && !page && document.activeElement === document.body) canvas.focus({ preventScroll: true });
    });
    return h;
}

export function sync(h, key, text, base, anchor, focus, secure) {
    if (h.composing && key === h.key) return;
    h.composing = false;
    h.syncing = true;
    const editing = document.activeElement === h.textarea || document.activeElement === h.password;
    h.key = key; h.base = base; h.value = text;
    h.password.value = '';
    h.field = secure ? h.password : h.textarea;
    if (!key) {
        h.textarea.value = '';
        if (editing) h.canvas.focus({ preventScroll: true });
        h.textarea.inputMode = h.password.inputMode = coarse() ? 'none' : 'text';
    } else {
        if (h.textarea.value !== text) h.textarea.value = text;
        if (!secure) {
            const start = Math.min(anchor, focus), end = Math.max(anchor, focus);
            const direction = anchor > focus ? 'backward' : 'forward';
            if (h.textarea.selectionStart !== start || h.textarea.selectionEnd !== end || h.textarea.selectionDirection !== direction) {
                h.textarea.setSelectionRange(start, end, direction);
            }
        }
        if (editing && document.activeElement !== h.field) activate(h);
    }
    h.syncing = false;
}

export function syncCopy(h, text) {
    h.copyText = text;
}

// What the right-click Rune just heard is on: 'app' (an element's menu, which Rune draws),
// 'field', 'text' or '' (`Runtime::secondary_claim`).
export function syncSecondary(h, claim) {
    h.secondary = claim;
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

// Takes the menu down; `refocus` gives the canvas its focus back, as a closing menu does.
function closeMenu(h, refocus) {
    if (!h.menu) return;
    const menu = h.menu;
    h.menu = null;
    menu.done();
    if (refocus) h.canvas.focus({ preventScroll: true });
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

const TAGS = { heading: null, button: 'button', link: 'a', image: 'div', text: 'p', scroll_x: 'div' };

function family(name) {
    if (name === 'sans') return 'var(--rune-sans,sans-serif)';
    if (name === 'mono') return 'var(--rune-mono,monospace)';
    return `"${name.replace(/["\\]/g, '')}",var(--rune-sans,sans-serif)`;
}

// The mirror's first item element, after its stylesheet.
function firstItem(el) {
    let c = el.firstElementChild;
    while (c && c.tagName === 'STYLE') c = c.nextElementSibling;
    return c;
}

function runs(h, el, item) {
    for (const run of item.u) {
        if (!run.h) { el.append(document.createTextNode(run.t)); continue; }
        const a = document.createElement('a');
        a.textContent = run.t;
        a.style.cssText = `color:transparent;text-decoration:none;pointer-events:${h.page ? 'auto' : 'none'};`;
        if (webLink(run.h)) {
            a.href = run.h;
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

function itemStyle(h, item, origin) {
    const [x, y, w, bh] = item.r;
    let css = `position:absolute;left:${x - origin[0]}px;top:${y - origin[1]}px;width:${w}px;height:${bh}px;margin:0;padding:0;border:0;background:transparent;color:transparent;`;
    if (item.k === 'group') return css + 'overflow:visible;list-style:none;pointer-events:none;';
    // A page's horizontal scroller: the browser scrolls it natively, so a flick inside a code
    // block scrolls the block at the compositor's rate while a vertical drag still scrolls the
    // page.
    if (item.k === 'scroll_x') return css + 'overflow-x:auto;overflow-y:hidden;pointer-events:auto;touch-action:pan-x pan-y pinch-zoom;scrollbar-width:none;';
    if (item.k === 'link') return css + 'display:block;overflow:hidden;text-decoration:none;cursor:pointer;pointer-events:auto;font:14px/1.25 sans-serif;';
    if (item.k === 'image' || item.k === 'button') return css + 'overflow:hidden;pointer-events:none;font:14px/1.25 sans-serif;';
    const fs = item.s || 14;
    if (!item.f) return css + `font:${fs}px/1.25 sans-serif;overflow:hidden;white-space:pre-wrap;pointer-events:none;`;
    const [fam, weight, lh, tracking, italic, align] = item.f;
    css += `font:${italic ? 'italic ' : ''}${weight} ${fs}px/${lh}px ${family(fam)};letter-spacing:${tracking}px;text-align:${align};white-space:pre-wrap;overflow-wrap:break-word;overflow:visible;`;
    return css + (item.e ? 'pointer-events:auto;user-select:text;-webkit-user-select:text;cursor:text;' : 'pointer-events:none;user-select:none;-webkit-user-select:none;');
}

export function syncA11y(h, json) {
    if (h.disposed) return;
    const items = JSON.parse(json);
    const seen = new Set();
    const els = [];
    const cursors = new Map([[h.a11y, firstItem(h.a11y)]]);
    for (const item of items) {
        const tag = item.k === 'group' ? item.g : item.k === 'heading' ? `h${item.l || 2}` : TAGS[item.k] ?? 'p';
        const key = `${tag}:${item.p}`;
        seen.add(key);
        let el = h.a11yNodes.get(key);
        if (!el) {
            el = document.createElement(tag);
            if (tag === 'button') {
                el.type = 'button';
                el.tabIndex = h.page ? 0 : -1;
                el.addEventListener('click', ev => {
                    ev.preventDefault();
                    h.send({ kind: 'a11y_press', path: item.p });
                });
            }
            if (item.k === 'image') el.setAttribute('role', 'img');
            if (item.k === 'scroll_x') {
                el.setAttribute('data-rune-scroll-x', '');
                // The browser scrolls this container itself; Rune draws the content where the
                // container says it is.
                el.addEventListener('scroll', () => {
                    const path = el._path;
                    if (path && !h.disposed) h.send({ kind: 'scroll_x', path, x: el.scrollLeft });
                }, { passive: true });
            }
            h.a11yNodes.set(key, el);
        }
        const parent = item.a === undefined ? null : items[item.a];
        const parentEl = item.a === undefined ? h.a11y : els[item.a];
        // Writing `cssText` invalidates the node's style and its layout: only do it when the
        // mirror's item actually moved or changed, so one animating element never rewrites a whole
        // page's worth of nodes mid-scroll.
        const css = itemStyle(h, item, parent ? parent.r : [0, 0]);
        if (el._css !== css) { el._css = css; el.style.cssText = css; }
        if (item.i) el.id = item.i; else el.removeAttribute('id');
        // A table's parts: the role says what the div is (`aria_role`).
        if ((el.getAttribute('role') ?? undefined) !== item.o && item.k === 'group') {
            if (item.o) el.setAttribute('role', item.o); else el.removeAttribute('role');
        }
        if (tag === 'button') {
            if (item.x === true || item.x === false) el.setAttribute('aria-expanded', String(item.x));
            else el.removeAttribute('aria-expanded');
        }
        if (item.k === 'group' || item.k === 'image') {
            if (item.t) el.setAttribute('aria-label', item.t); else el.removeAttribute('aria-label');
        } else if (item.k === 'link') {
            el.href = item.h;
            if (!h.page && webLink(item.h)) { el.target = '_blank'; el.rel = 'noopener noreferrer'; }
        }
        const container = item.k === 'group' || item.k === 'scroll_x';
        if (item.k === 'scroll_x' && el._path !== item.p) { el._path = item.p; h.scrollEls.set(item.p, el); }
        const sig = container || item.k === 'image' ? '' : item.u?.length ? JSON.stringify(item.u) : item.t ?? '';
        if (!container && el._sig !== sig) {
            el._sig = sig;
            el.replaceChildren();
            if (item.u?.length) runs(h, el, item);
            else if (item.k !== 'image') el.textContent = item.t ?? '';
        }
        els.push(el);
        if (container && !cursors.has(el)) cursors.set(el, el.firstElementChild);
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

// Rune skipped rebuilding the mirror while it scrolls: it asks again once the scroll has rested.
export function scheduleA11y(h) {
    clearTimeout(h.a11yTimer);
    h.a11yTimer = setTimeout(() => { h.a11yTimer = 0; if (!h.disposed) h.send({ kind: 'a11y_due' }); }, 160);
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

// A horizontal viewport Rune scrolled itself (a wheel, a key): move the mirror's container to
// match, so the two stay the same text. The container's own scroll event echoes back the same
// offset, which Rune ignores.
export function syncScrollX(h, path, x) {
    const el = h.scrollEls.get(path);
    if (el && Math.abs(el.scrollLeft - x) > 0.5) el.scrollLeft = x;
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
    // A page's canvas is `tabindex="-1"`: focusing it on a tap would only drop the reader's text
    // selection or a link's focus. An input that took the keyboard still gets it.
    if (h.page && !h.key) return;
    const field = h.key ? h.field : h.canvas;
    if (h.key) field.inputMode = 'text';
    if (document.activeElement !== field) field.focus({ preventScroll: true });
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
    clearTimeout(h.a11yTimer);
    for (const remove of h.listeners) remove();
    h.password.value = ''; h.textarea.value = ''; h.copyText = '';
    h.a11yNodes.clear();
    h.scrollEls.clear();
    h.a11y.remove();
    h.band?.remove();
    h.password.remove(); h.textarea.remove();
}
