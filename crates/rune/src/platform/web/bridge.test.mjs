import assert from 'node:assert/strict';
import test from 'node:test';

class FakeNode {
    constructor(tagName) {
        this.tagName = tagName.toUpperCase();
        this.style = { cssText: '', setProperty() {} };
        this._attrs = new Map();
        this.listeners = new Map();
        this.children = [];
        this.parentElement = null;
        this.tabIndex = 0;
        this._inert = false;
        this.inputMode = '';
        this.type = '';
        this.wrap = '';
        this.autocomplete = '';
        this.spellcheck = true;
        this._value = '';
        this.selectionStart = 0;
        this.selectionEnd = 0;
        this.selectionDirection = 'forward';
        this.onFocus = null;
    }

    get inert() { return this._inert; }
    set inert(next) {
        this._inert = Boolean(next);
        if (this._inert && globalThis.document?.activeElement === this) {
            globalThis.document.activeElement = globalThis.document.body;
            this.dispatchEvent({ type: 'blur' });
        }
    }

    get value() { return this._value; }
    set value(next) {
        // HTML <textarea> normalizes CRLF and lone CR to LF in its API value.
        this._value = this.tagName === 'TEXTAREA' ? String(next).replace(/\r\n?/g, '\n') : String(next);
        this.selectionStart = this._value.length;
        this.selectionEnd = this._value.length;
        this.selectionDirection = 'forward';
    }

    get attributes() { return [...this._attrs].map(([name, value]) => ({ name, value })); }
    setAttribute(name, value) { this._attrs.set(name, String(value)); }
    removeAttribute(name) {
        this._attrs.delete(name);
        if (name === 'href') delete this.href;
        if (name === 'target') delete this.target;
        if (name === 'rel') delete this.rel;
    }
    getAttribute(name) { return this._attrs.get(name) ?? null; }
    hasAttribute(name) { return this._attrs.has(name); }

    get firstElementChild() { return this.children[0] ?? null; }
    get previousElementSibling() {
        if (!this.parentElement) return null;
        const i = this.parentElement.children.indexOf(this);
        return i > 0 ? this.parentElement.children[i - 1] : null;
    }
    get nextElementSibling() {
        if (!this.parentElement) return null;
        const i = this.parentElement.children.indexOf(this);
        return i >= 0 ? (this.parentElement.children[i + 1] ?? null) : null;
    }

    insertBefore(node, ref) {
        if (node.parentElement) node.remove();
        node.parentElement = this;
        const i = ref ? this.children.indexOf(ref) : -1;
        if (i >= 0) this.children.splice(i, 0, node);
        else this.children.push(node);
        return node;
    }

    replaceChildren(...nodes) {
        for (const child of this.children) child.parentElement = null;
        this.children = [];
        this.append(...nodes);
    }

    addEventListener(name, handler) {
        let list = this.listeners.get(name);
        if (!list) { list = []; this.listeners.set(name, list); }
        list.push(handler);
    }

    removeEventListener(name, handler) {
        const list = this.listeners.get(name);
        if (!list) return;
        const i = list.indexOf(handler);
        if (i >= 0) list.splice(i, 1);
    }

    dispatchEvent(event) {
        event.target ??= this;
        for (const handler of [...(this.listeners.get(event.type) ?? [])]) handler(event);
    }

    append(...nodes) {
        for (const node of nodes) {
            node.parentElement = this;
            this.children.push(node);
        }
    }

    after(node) {
        if (this.parentElement) this.parentElement.append(node);
    }

    replaceWith(node) {
        if (this.parentElement) this.parentElement.append(node);
    }

    remove() {
        if (!this.parentElement) return;
        if (globalThis.document?.activeElement && this.contains(globalThis.document.activeElement)) {
            globalThis.document.activeElement = globalThis.document.body;
        }
        const i = this.parentElement.children.indexOf(this);
        if (i >= 0) this.parentElement.children.splice(i, 1);
        this.parentElement = null;
    }

    querySelectorAll(selector) {
        const parts = selector.split(',').map(s => s.trim());
        const out = [];
        const walk = node => {
            for (const child of node.children) {
                if (parts.some(sel => {
                    if (sel === 'a[target="_blank"]') return child.tagName === 'A' && child.target === '_blank';
                    if (sel === 'a[href]') return child.tagName === 'A' && typeof child.href === 'string' && child.href.length > 0;
                    return child.tagName === sel.toUpperCase();
                })) out.push(child);
                walk(child);
            }
        };
        walk(this);
        return out;
    }

    getBoundingClientRect() {
        return { left: 0, top: 0, right: 160, bottom: 80, width: 160, height: 80 };
    }

    getClientRects() {
        return [];
    }

    contains(node) {
        let cur = node;
        while (cur) {
            if (cur === this) return true;
            cur = cur.parentElement;
        }
        return false;
    }

    setSelectionRange(start, end, direction = 'forward') {
        const len = this._value.length;
        this.selectionStart = Math.max(0, Math.min(start, len));
        this.selectionEnd = Math.max(this.selectionStart, Math.min(end, len));
        this.selectionDirection = direction;
        if (this.selectionStart !== this.selectionEnd) {
            this.dispatchEvent({ type: 'select' });
        }
    }

    focus() {
        if (this._inert) return;
        const prev = globalThis.document.activeElement;
        if (prev === this) return;
        globalThis.document.activeElement = this;
        if (prev) prev.dispatchEvent({ type: 'blur' });
        if (this.onFocus) this.onFocus();
    }
}

let coarsePointer = false;

function installDom({ platform = 'Linux x86_64' } = {}) {
    const body = new FakeNode('body');
    const head = new FakeNode('head');
    const doc = new FakeNode('#document');
    doc.body = body;
    doc.head = head;
    doc.documentElement = new FakeNode('html');
    doc.activeElement = body;
    doc.hidden = false;
    doc.title = '';
    doc.createElement = tag => new FakeNode(tag);
    doc.createTextNode = text => Object.assign(new FakeNode('#text'), { textContent: String(text) });

    const win = new FakeNode('window');
    Object.defineProperty(globalThis, 'Node', { value: FakeNode, configurable: true });
    Object.defineProperty(globalThis, 'document', { value: doc, configurable: true });
    Object.defineProperty(globalThis, 'window', { value: win, configurable: true });
    Object.defineProperty(globalThis, 'navigator', {
        value: { platform, standalone: false },
        configurable: true,
    });
    Object.defineProperty(globalThis, 'matchMedia', {
        value: query => ({ matches: query === '(pointer: coarse)' ? coarsePointer : false }),
        configurable: true,
    });
    Object.defineProperty(globalThis, 'getComputedStyle', {
        value: () => ({ position: 'relative', direction: 'ltr' }),
        configurable: true,
    });
    Object.defineProperty(globalThis, 'innerWidth', { value: 800, writable: true, configurable: true });
    Object.defineProperty(globalThis, 'innerHeight', { value: 844, writable: true, configurable: true });
    return { doc, win, body };
}

installDom();
const bridge = await import('./bridge.js');

function setup({ embedded = false, fitHeight = false, page = false, coarse = false, platform = 'Linux x86_64' } = {}) {
    coarsePointer = coarse;
    const { doc, win, body } = installDom({ platform });
    globalThis.innerWidth = 800;
    const canvas = doc.createElement('canvas');
    body.append(canvas);
    const events = [];
    const h = bridge.mount(canvas, msg => events.push(JSON.parse(msg)), embedded, fitHeight, page);
    return { doc, win, body, canvas, events, h };
}

function insertAtCaret(textarea, text) {
    const before = textarea.value.slice(0, textarea.selectionStart);
    const after = textarea.value.slice(textarea.selectionEnd);
    const nextCaret = before.length + text.length;
    textarea.value = before + text + after;
    textarea.setSelectionRange(nextCaret, nextCaret, 'forward');
    textarea.dispatchEvent({ type: 'input', isComposing: false });
}

test('sync reclaims DOM focus and selection from canvas after a one-frame blur', async () => {
    const { doc, canvas, events, h } = setup();
    await Promise.resolve();
    assert.equal(doc.activeElement, canvas);

    // Focus field at end of "hello" via click.
    bridge.sync(h, 'field', 'hello', 0, 5, 5, false);
    bridge.activate(h);
    assert.equal(doc.activeElement, h.textarea);
    assert.equal(h.textarea.selectionStart, 5);
    assert.equal(h.textarea.selectionEnd, 5);

    // Frame blurs all fields (e.g. pressing an El::window_drag() element).
    bridge.sync(h, '', '', 0, 0, 0, false);
    assert.equal(doc.activeElement, canvas);
    assert.equal(h.textarea.value, '');

    // Next frame restores Rune focus to the field without activate() (sync_input(false)).
    // Simulate a browser collapsing the textarea selection when it takes focus.
    h.textarea.onFocus = () => {
        h.textarea.selectionStart = 0;
        h.textarea.selectionEnd = 0;
        h.textarea.selectionDirection = 'none';
    };
    bridge.sync(h, 'field', 'hello', 0, 5, 5, false);
    h.textarea.onFocus = null;

    assert.equal(doc.activeElement, h.textarea);
    assert.equal(h.textarea.selectionStart, 5);
    assert.equal(h.textarea.selectionEnd, 5);
    assert.equal(h.textarea.selectionDirection, 'forward');

    // Text inserted via IME / browser text services lands at the end, not index 0.
    insertAtCaret(h.textarea, '!');
    assert.equal(events.length, 1);
    assert.deepEqual(events[0], {
        kind: 'edit',
        key: 'field',
        base: 0,
        expected: 'hello',
        start: 5,
        end: 5,
        text: '!',
        anchor: 6,
        focus: 6,
    });
    bridge.destroy(h);
});

test('activate restores saved selection when sync is skipped after Chrome collapsed it to 0', () => {
    const { doc, canvas, events, h } = setup();
    bridge.sync(h, 'field', 'hello', 0, 5, 5, false);
    bridge.activate(h);
    assert.equal(doc.activeElement, h.textarea);

    // DOM focus moved back to canvas and Chrome collapsed the unfocused textarea's selection to 0.
    canvas.focus();
    h.textarea.selectionStart = 0;
    h.textarea.selectionEnd = 0;
    h.textarea.selectionDirection = 'none';

    // User clicks back at the exact same caret: mirror did not change, so sync() is skipped
    // and only activate() runs.
    bridge.activate(h);
    assert.equal(doc.activeElement, h.textarea);
    assert.equal(h.textarea.selectionStart, 5);
    assert.equal(h.textarea.selectionEnd, 5);
    assert.equal(h.textarea.selectionDirection, 'forward');

    insertAtCaret(h.textarea, ' world');
    assert.equal(events.length, 1);
    assert.equal(events[0].start, 5);
    assert.equal(events[0].end, 5);
    assert.equal(events[0].text, ' world');
    bridge.destroy(h);
});

test('activate restores backward non-collapsed selection without echoing a selection event', () => {
    const { doc, canvas, events, h } = setup();
    bridge.sync(h, 'field', 'hello', 0, 5, 1, false);
    bridge.activate(h);
    assert.equal(events.length, 0);

    canvas.focus();
    h.textarea.selectionStart = 0;
    h.textarea.selectionEnd = 0;
    h.textarea.selectionDirection = 'forward';

    bridge.activate(h);
    assert.equal(doc.activeElement, h.textarea);
    assert.equal(h.textarea.selectionStart, 1);
    assert.equal(h.textarea.selectionEnd, 5);
    assert.equal(h.textarea.selectionDirection, 'backward');
    assert.equal(events.length, 0, 'reselect must not echo a spurious selection event');
    bridge.destroy(h);
});

test('sync sets inputMode text before reclaiming focus from canvas on coarse pointers', async () => {
    const { doc, canvas, h } = setup({ coarse: true });
    await Promise.resolve();
    assert.equal(doc.activeElement, canvas);
    assert.equal(h.textarea.inputMode, 'none');

    bridge.sync(h, 'field', 'hello', 0, 5, 5, false);
    assert.equal(doc.activeElement, h.textarea);
    assert.equal(h.textarea.inputMode, 'text');
    assert.equal(h.textarea.selectionStart, 5);

    bridge.activate(h);
    assert.equal(h.textarea.inputMode, 'text');
    bridge.destroy(h);
});

test('sync does not steal focus from outside elements, and activate still restores selection', () => {
    const { doc, body, h } = setup({ embedded: true });
    const outside = doc.createElement('button');
    body.append(outside);
    outside.focus();

    bridge.sync(h, 'field', 'hello', 0, 5, 5, false);
    assert.equal(doc.activeElement, outside);

    h.textarea.selectionStart = 0;
    h.textarea.selectionEnd = 0;
    bridge.activate(h);
    assert.equal(doc.activeElement, h.textarea);
    assert.equal(h.textarea.selectionStart, 5);
    assert.equal(h.textarea.selectionEnd, 5);
    bridge.destroy(h);
});

test('activate leaves selection alone during IME composition', () => {
    const { doc, canvas, h } = setup();
    bridge.sync(h, 'field', 'hello', 0, 5, 5, false);
    bridge.activate(h);

    h.textarea.dispatchEvent({ type: 'compositionstart' });
    h.textarea.value = 'helloni';
    h.textarea.setSelectionRange(5, 7, 'forward');

    canvas.focus();
    bridge.activate(h);
    assert.equal(doc.activeElement, h.textarea);
    assert.equal(h.textarea.selectionStart, 5);
    assert.equal(h.textarea.selectionEnd, 7);
    bridge.destroy(h);
});

test('sync reclaims focus from canvas for secure fields without touching textarea selection', async () => {
    const { doc, canvas, h } = setup();
    await Promise.resolve();
    assert.equal(doc.activeElement, canvas);

    bridge.sync(h, 'secret', '', 0, 0, 0, true);
    assert.equal(doc.activeElement, h.password);
    assert.equal(h.textarea.value, '');
    bridge.destroy(h);
});

test('standalone launch focuses the synced field whether sync runs before or after mount microtask', async () => {
    const first = setup();
    bridge.sync(first.h, 'field', 'hello', 0, 5, 5, false);
    await Promise.resolve();
    assert.equal(first.doc.activeElement, first.h.textarea);
    assert.equal(first.h.textarea.selectionStart, 5);
    assert.equal(first.h.textarea.selectionEnd, 5);
    bridge.destroy(first.h);

    const second = setup();
    await Promise.resolve();
    assert.equal(second.doc.activeElement, second.canvas);
    bridge.sync(second.h, 'field', 'hello', 0, 5, 5, false);
    assert.equal(second.doc.activeElement, second.h.textarea);
    assert.equal(second.h.textarea.selectionStart, 5);
    assert.equal(second.h.textarea.selectionEnd, 5);
    bridge.destroy(second.h);
});

test('mount keeps both hidden editing proxies inert until a field is focused', async () => {
    for (const opts of [{}, { embedded: true }, { page: true }]) {
        const { h } = setup(opts);
        assert.equal(h.textarea.inert, true);
        assert.equal(h.password.inert, true);
        await Promise.resolve();
        assert.equal(h.textarea.inert, true);
        assert.equal(h.password.inert, true);
        bridge.destroy(h);
    }
});

test('sync exposes only the active editing proxy and returns both to inert on blur', async () => {
    const { doc, canvas, h } = setup();
    await Promise.resolve();
    assert.equal(doc.activeElement, canvas);
    assert.equal(h.textarea.inert, true);
    assert.equal(h.password.inert, true);

    // Focusing a plain text field clears inert on textarea only before focusing it.
    bridge.sync(h, 'plain', 'hello', 0, 5, 5, false);
    assert.equal(h.textarea.inert, false);
    assert.equal(h.password.inert, true);
    assert.equal(doc.activeElement, h.textarea);

    // Switching directly from plain text to password focuses password before making textarea inert.
    const textareaBlurTargets = [];
    h.textarea.addEventListener('blur', () => textareaBlurTargets.push(doc.activeElement));
    const passwordBlurTargets = [];
    h.password.addEventListener('blur', () => passwordBlurTargets.push(doc.activeElement));

    bridge.sync(h, 'secret', '', 0, 0, 0, true);
    assert.deepEqual(textareaBlurTargets, [h.password], 'focus must move to password before textarea becomes inert');
    assert.equal(doc.activeElement, h.password);
    assert.equal(h.textarea.inert, true);
    assert.equal(h.password.inert, false);

    // Switching directly from password back to plain text focuses textarea before making password inert.
    bridge.sync(h, 'plain', 'hello', 0, 5, 5, false);
    assert.deepEqual(passwordBlurTargets, [h.textarea], 'focus must move to textarea before password becomes inert');
    assert.equal(doc.activeElement, h.textarea);
    assert.equal(h.textarea.inert, false);
    assert.equal(h.password.inert, true);

    // Blurring returns focus to the canvas before making both proxies inert.
    bridge.sync(h, '', '', 0, 0, 0, false);
    assert.deepEqual(textareaBlurTargets, [h.password, canvas], 'focus must return to canvas before textarea becomes inert');
    assert.equal(doc.activeElement, canvas);
    assert.equal(h.textarea.inert, true);
    assert.equal(h.password.inert, true);

    // Blurring from a password field also returns focus to the canvas before making both inert.
    bridge.sync(h, 'secret', '', 0, 0, 0, true);
    assert.equal(doc.activeElement, h.password);
    assert.equal(h.textarea.inert, true);
    assert.equal(h.password.inert, false);
    bridge.sync(h, '', '', 0, 0, 0, false);
    assert.deepEqual(passwordBlurTargets, [h.textarea, canvas], 'focus must return to canvas before password becomes inert');
    assert.equal(doc.activeElement, canvas);
    assert.equal(h.textarea.inert, true);
    assert.equal(h.password.inert, true);

    bridge.destroy(h);
});

function dispatchKeydown(target, props) {
    let prevented = false;
    target.dispatchEvent({
        type: 'keydown',
        code: '',
        shiftKey: false,
        ctrlKey: false,
        altKey: false,
        metaKey: false,
        repeat: false,
        isComposing: false,
        keyCode: 0,
        preventDefault() { prevented = true; },
        ...props,
    });
    return prevented;
}

function dispatchBeforeInput(target, props) {
    let prevented = false;
    target.dispatchEvent({
        type: 'beforeinput',
        isComposing: false,
        preventDefault() { prevented = true; },
        ...props,
    });
    return prevented;
}

test('keydown forwards Backspace at offset 0 with empty selection on textarea', () => {
    const { events, h } = setup();

    // Empty field: caret at 0..0.
    bridge.sync(h, 'field', '', 0, 0, 0, false);
    bridge.activate(h);
    assert.equal(dispatchKeydown(h.textarea, { key: 'Backspace', code: 'Backspace', keyCode: 8 }), true);
    assert.deepEqual(events, [{
        kind: 'key',
        key: 'Backspace',
        code: 'Backspace',
        shift: false,
        ctrl: false,
        alt: false,
        meta: false,
        repeat: false,
    }]);

    // Non-empty field with caret at offset 0: modifiers and repeat pass through.
    events.length = 0;
    bridge.sync(h, 'field', 'hello', 0, 0, 0, false);
    assert.equal(dispatchKeydown(h.textarea, {
        key: 'Backspace',
        code: 'Backspace',
        altKey: true,
        repeat: true,
        keyCode: 8,
    }), true);
    assert.deepEqual(events, [{
        kind: 'key',
        key: 'Backspace',
        code: 'Backspace',
        shift: false,
        ctrl: false,
        alt: true,
        meta: false,
        repeat: true,
    }]);

    bridge.destroy(h);
});

test('keydown leaves Backspace to browser editing past offset 0, with a selection, or while composing', () => {
    const { events, h } = setup();

    // Caret after text: browser deletes via input event.
    bridge.sync(h, 'field', 'hello', 0, 5, 5, false);
    bridge.activate(h);
    assert.equal(dispatchKeydown(h.textarea, { key: 'Backspace', code: 'Backspace', keyCode: 8 }), false);
    assert.equal(events.length, 0);

    // Non-empty forward selection starting at 0.
    bridge.sync(h, 'field', 'hello', 0, 0, 3, false);
    assert.equal(dispatchKeydown(h.textarea, { key: 'Backspace', code: 'Backspace', keyCode: 8 }), false);
    assert.equal(events.length, 0);

    // Non-empty backward selection ending at 0.
    bridge.sync(h, 'field', 'hello', 0, 3, 0, false);
    assert.equal(dispatchKeydown(h.textarea, { key: 'Backspace', code: 'Backspace', keyCode: 8 }), false);
    assert.equal(events.length, 0);

    // IME composition at offset 0 must not be intercepted.
    bridge.sync(h, 'field', '', 0, 0, 0, false);
    assert.equal(dispatchKeydown(h.textarea, { key: 'Backspace', code: 'Backspace', isComposing: true, keyCode: 8 }), false);
    assert.equal(dispatchKeydown(h.textarea, { key: 'Backspace', code: 'Backspace', keyCode: 229 }), false);
    h.textarea.dispatchEvent({ type: 'compositionstart' });
    assert.equal(dispatchKeydown(h.textarea, { key: 'Backspace', code: 'Backspace', keyCode: 8 }), false);
    assert.equal(events.length, 0);

    bridge.destroy(h);
});

test('beforeinput forwards backward delete at offset 0 when soft keyboard bypasses keydown', () => {
    const { events, h } = setup();

    bridge.sync(h, 'field', 'hello', 0, 0, 0, false);
    bridge.activate(h);
    assert.equal(dispatchBeforeInput(h.textarea, { inputType: 'deleteContentBackward' }), true);
    assert.equal(dispatchBeforeInput(h.textarea, { inputType: 'deleteWordBackward' }), true);
    assert.deepEqual(events, [
        { kind: 'key', key: 'Backspace', code: 'Backspace', shift: false, ctrl: false, alt: false, meta: false, repeat: false },
        { kind: 'key', key: 'Backspace', code: 'Backspace', shift: false, ctrl: false, alt: true, meta: false, repeat: false },
    ]);

    // Must not intercept forward delete, non-zero caret, selection, or composition.
    events.length = 0;
    assert.equal(dispatchBeforeInput(h.textarea, { inputType: 'deleteContentForward' }), false);
    assert.equal(dispatchBeforeInput(h.textarea, { inputType: 'deleteContentBackward', isComposing: true }), false);
    bridge.sync(h, 'field', 'hello', 0, 1, 1, false);
    assert.equal(dispatchBeforeInput(h.textarea, { inputType: 'deleteContentBackward' }), false);
    bridge.sync(h, 'field', 'hello', 0, 0, 2, false);
    assert.equal(dispatchBeforeInput(h.textarea, { inputType: 'deleteContentBackward' }), false);
    bridge.sync(h, 'field', '', 0, 0, 0, false);
    h.textarea.dispatchEvent({ type: 'compositionstart' });
    assert.equal(dispatchBeforeInput(h.textarea, { inputType: 'deleteContentBackward' }), false);
    assert.equal(events.length, 0);

    bridge.destroy(h);
});

test('keydown forwards Delete at end of value with empty selection on textarea', () => {
    const { events, h } = setup();

    // Empty field: caret is at both start (0) and end (0).
    bridge.sync(h, 'field', '', 0, 0, 0, false);
    bridge.activate(h);
    assert.equal(dispatchKeydown(h.textarea, { key: 'Delete', code: 'Delete', keyCode: 46 }), true);
    assert.deepEqual(events, [{
        kind: 'key',
        key: 'Delete',
        code: 'Delete',
        shift: false,
        ctrl: false,
        alt: false,
        meta: false,
        repeat: false,
    }]);

    // Non-empty field with caret at the end: modifiers and repeat pass through.
    events.length = 0;
    bridge.sync(h, 'field', 'hello', 0, 5, 5, false);
    assert.equal(dispatchKeydown(h.textarea, {
        key: 'Delete',
        code: 'Delete',
        altKey: true,
        repeat: true,
        keyCode: 46,
    }), true);
    assert.deepEqual(events, [{
        kind: 'key',
        key: 'Delete',
        code: 'Delete',
        shift: false,
        ctrl: false,
        alt: true,
        meta: false,
        repeat: true,
    }]);

    // CRLF text: raw length is 7 while the LF-normalized textarea mirror has length 5.
    events.length = 0;
    const crlf = 'a\r\nb\r\nc';
    bridge.sync(h, 'field', crlf, 0, crlf.length, crlf.length, false);
    assert.equal(h.textarea.value.length, 5);
    assert.equal(h.textarea.selectionStart, 5);
    assert.equal(h.textarea.selectionEnd, 5);
    assert.equal(dispatchKeydown(h.textarea, { key: 'Delete', code: 'Delete', keyCode: 46 }), true);
    assert.deepEqual(events, [{
        kind: 'key',
        key: 'Delete',
        code: 'Delete',
        shift: false,
        ctrl: false,
        alt: false,
        meta: false,
        repeat: false,
    }]);

    bridge.destroy(h);
});

test('keydown leaves Delete to browser editing before end, with a selection, or while composing', () => {
    const { events, h } = setup();

    // Caret at start or middle: browser deletes via input event.
    bridge.sync(h, 'field', 'hello', 0, 0, 0, false);
    bridge.activate(h);
    assert.equal(dispatchKeydown(h.textarea, { key: 'Delete', code: 'Delete', keyCode: 46 }), false);
    bridge.sync(h, 'field', 'hello', 0, 2, 2, false);
    assert.equal(dispatchKeydown(h.textarea, { key: 'Delete', code: 'Delete', keyCode: 46 }), false);
    assert.equal(events.length, 0);

    // Caret before last character in CRLF text (raw 6 -> DOM 4 < 5).
    const crlf = 'a\r\nb\r\nc';
    bridge.sync(h, 'field', crlf, 0, 6, 6, false);
    assert.equal(dispatchKeydown(h.textarea, { key: 'Delete', code: 'Delete', keyCode: 46 }), false);
    assert.equal(events.length, 0);

    // Non-empty forward selection ending at the end.
    bridge.sync(h, 'field', 'hello', 0, 2, 5, false);
    assert.equal(dispatchKeydown(h.textarea, { key: 'Delete', code: 'Delete', keyCode: 46 }), false);
    assert.equal(events.length, 0);

    // Non-empty backward selection starting at the end.
    bridge.sync(h, 'field', 'hello', 0, 5, 2, false);
    assert.equal(dispatchKeydown(h.textarea, { key: 'Delete', code: 'Delete', keyCode: 46 }), false);
    assert.equal(events.length, 0);

    // IME composition at the end must not be intercepted.
    bridge.sync(h, 'field', 'hello', 0, 5, 5, false);
    assert.equal(dispatchKeydown(h.textarea, { key: 'Delete', code: 'Delete', isComposing: true, keyCode: 46 }), false);
    assert.equal(dispatchKeydown(h.textarea, { key: 'Delete', code: 'Delete', keyCode: 229 }), false);
    h.textarea.dispatchEvent({ type: 'compositionstart' });
    assert.equal(dispatchKeydown(h.textarea, { key: 'Delete', code: 'Delete', keyCode: 46 }), false);
    assert.equal(events.length, 0);

    bridge.destroy(h);
});

test('beforeinput forwards forward delete at end of value when soft keyboard bypasses keydown', () => {
    const { events, h } = setup();

    bridge.sync(h, 'field', 'hello', 0, 5, 5, false);
    bridge.activate(h);
    assert.equal(dispatchBeforeInput(h.textarea, { inputType: 'deleteContentForward' }), true);
    assert.equal(dispatchBeforeInput(h.textarea, { inputType: 'deleteWordForward' }), true);
    assert.deepEqual(events, [
        { kind: 'key', key: 'Delete', code: 'Delete', shift: false, ctrl: false, alt: false, meta: false, repeat: false },
        { kind: 'key', key: 'Delete', code: 'Delete', shift: false, ctrl: false, alt: true, meta: false, repeat: false },
    ]);

    // Empty field and CRLF field at end also forward deleteContentForward.
    events.length = 0;
    bridge.sync(h, 'field', '', 0, 0, 0, false);
    assert.equal(dispatchBeforeInput(h.textarea, { inputType: 'deleteContentForward' }), true);
    const crlf = 'a\r\nb';
    bridge.sync(h, 'field', crlf, 0, crlf.length, crlf.length, false);
    assert.equal(dispatchBeforeInput(h.textarea, { inputType: 'deleteContentForward' }), true);
    assert.deepEqual(events, [
        { kind: 'key', key: 'Delete', code: 'Delete', shift: false, ctrl: false, alt: false, meta: false, repeat: false },
        { kind: 'key', key: 'Delete', code: 'Delete', shift: false, ctrl: false, alt: false, meta: false, repeat: false },
    ]);

    // Must not intercept backward delete at end, non-end caret, selection, or composition.
    events.length = 0;
    bridge.sync(h, 'field', 'hello', 0, 5, 5, false);
    assert.equal(dispatchBeforeInput(h.textarea, { inputType: 'deleteContentBackward' }), false);
    assert.equal(dispatchBeforeInput(h.textarea, { inputType: 'deleteContentForward', isComposing: true }), false);
    bridge.sync(h, 'field', 'hello', 0, 4, 4, false);
    assert.equal(dispatchBeforeInput(h.textarea, { inputType: 'deleteContentForward' }), false);
    bridge.sync(h, 'field', 'hello', 0, 3, 5, false);
    assert.equal(dispatchBeforeInput(h.textarea, { inputType: 'deleteContentForward' }), false);
    bridge.sync(h, 'field', 'hello', 0, 5, 5, false);
    h.textarea.dispatchEvent({ type: 'compositionstart' });
    assert.equal(dispatchBeforeInput(h.textarea, { inputType: 'deleteContentForward' }), false);
    assert.equal(events.length, 0);

    bridge.destroy(h);
});

test('password input uses current-password autocomplete and classifies keystrokes vs autofill', () => {
    const { events, h } = setup();
    assert.equal(h.password.autocomplete, 'current-password');

    bridge.sync(h, 'secret', '', 0, 0, 0, true);
    bridge.activate(h);

    // Single-grapheme keystrokes (ASCII, accented, combining mark, ZWJ emoji) go as 'secure'.
    for (const ch of ['a', 'é', 'e\u0301', '👨‍👩‍👧‍👦']) {
        events.length = 0;
        h.password.value = ch;
        h.password.dispatchEvent({ type: 'input', inputType: 'insertText', data: ch });
        assert.equal(h.password.value, '', 'password input must be emptied after every event');
        assert.deepEqual(events, [{ kind: 'secure', key: 'secret', text: ch }]);
    }

    // Empty typing sends nothing.
    events.length = 0;
    h.password.value = '';
    h.password.dispatchEvent({ type: 'input', inputType: 'insertText', data: '' });
    assert.equal(events.length, 0);

    // Missing inputType (plain Event) is a fill, including an empty clear.
    events.length = 0;
    h.password.value = 'filled-pw-123';
    h.password.dispatchEvent({ type: 'input' });
    assert.equal(h.password.value, '');
    assert.deepEqual(events, [{ kind: 'secure_replace', key: 'secret', text: 'filled-pw-123' }]);

    events.length = 0;
    h.password.value = '';
    h.password.dispatchEvent({ type: 'input' });
    assert.deepEqual(events, [{ kind: 'secure_replace', key: 'secret', text: '' }]);

    // insertReplacementText is always a fill.
    events.length = 0;
    h.password.value = 'replacement-pw';
    h.password.dispatchEvent({ type: 'input', inputType: 'insertReplacementText', data: 'replacement-pw' });
    assert.equal(h.password.value, '');
    assert.deepEqual(events, [{ kind: 'secure_replace', key: 'secret', text: 'replacement-pw' }]);

    // Multi-grapheme insertText (via data or via value when data is null) is a fill.
    events.length = 0;
    h.password.value = 'multi-char';
    h.password.dispatchEvent({ type: 'input', inputType: 'insertText', data: 'multi-char' });
    assert.equal(h.password.value, '');
    assert.deepEqual(events, [{ kind: 'secure_replace', key: 'secret', text: 'multi-char' }]);

    events.length = 0;
    h.password.value = 'from-value';
    h.password.dispatchEvent({ type: 'input', inputType: 'insertText', data: null });
    assert.equal(h.password.value, '');
    assert.deepEqual(events, [{ kind: 'secure_replace', key: 'secret', text: 'from-value' }]);

    // An element matching :autofill or :-webkit-autofill is a fill even for 1 char.
    for (const sel of [':autofill', ':-webkit-autofill']) {
        events.length = 0;
        h.password.matches = s => s === sel;
        h.password.value = 'x';
        h.password.dispatchEvent({ type: 'input', inputType: 'insertText', data: 'x' });
        assert.equal(h.password.value, '');
        assert.deepEqual(events, [{ kind: 'secure_replace', key: 'secret', text: 'x' }]);
    }
    delete h.password.matches;

    // Nothing is sent while no Rune field has focus, and the DOM input is still emptied.
    events.length = 0;
    bridge.sync(h, '', '', 0, 0, 0, false);
    h.password.value = 'leaked';
    h.password.dispatchEvent({ type: 'input' });
    assert.equal(h.password.value, '');
    assert.equal(events.length, 0);

    bridge.destroy(h);
});

test('typing at the end of CRLF text emits a pure insertion without rewriting existing CRLFs', () => {
    const { events, h } = setup();
    const before = 'a\r\nb\r\nc';
    bridge.sync(h, 'field', before, 0, before.length, before.length, false);
    bridge.activate(h);
    assert.equal(h.textarea.value, 'a\nb\nc');
    assert.equal(h.textarea.selectionStart, 5);
    assert.equal(h.textarea.selectionEnd, 5);

    insertAtCaret(h.textarea, 'x');
    assert.equal(events.length, 1);
    assert.deepEqual(events[0], {
        kind: 'edit',
        key: 'field',
        base: 0,
        expected: before,
        start: 7,
        end: 7,
        text: 'x',
        anchor: 8,
        focus: 8,
    });
    const after = before.slice(0, events[0].start) + events[0].text + before.slice(events[0].end);
    assert.equal(after, 'a\r\nb\r\ncx');
    bridge.destroy(h);
});

test('CRLF edits translate middle insertions, CRLF deletions, replacements, and trailing selections', () => {
    const { events, h } = setup();
    const before = 'a\r\nb\r\nc';

    // Insert after 'b' (raw offset 4 -> DOM offset 3).
    bridge.sync(h, 'field', before, 0, 4, 4, false);
    bridge.activate(h);
    assert.equal(h.textarea.selectionStart, 3);
    assert.equal(h.textarea.selectionEnd, 3);
    insertAtCaret(h.textarea, 'x');
    assert.deepEqual(events.pop(), {
        kind: 'edit',
        key: 'field',
        base: 0,
        expected: before,
        start: 4,
        end: 4,
        text: 'x',
        anchor: 5,
        focus: 5,
    });

    // Delete the first CRLF via Backspace after it (raw offset 3 -> DOM offset 2).
    bridge.sync(h, 'field', before, 0, 3, 3, false);
    h.textarea.value = 'ab\nc';
    h.textarea.setSelectionRange(1, 1, 'forward');
    h.textarea.dispatchEvent({ type: 'input', isComposing: false });
    const del = events.pop();
    assert.deepEqual(del, {
        kind: 'edit',
        key: 'field',
        base: 0,
        expected: before,
        start: 1,
        end: 3,
        text: '',
        anchor: 1,
        focus: 1,
    });
    assert.equal(before.slice(0, del.start) + del.text + before.slice(del.end), 'ab\r\nc');

    // Replace across the first CRLF ('a\r\nb', raw 0..4 -> DOM 0..3) while leaving the second intact.
    bridge.sync(h, 'field', before, 0, 0, 4, false);
    assert.equal(h.textarea.selectionStart, 0);
    assert.equal(h.textarea.selectionEnd, 3);
    insertAtCaret(h.textarea, 'Z');
    const rep = events.pop();
    assert.deepEqual(rep, {
        kind: 'edit',
        key: 'field',
        base: 0,
        expected: before,
        start: 0,
        end: 4,
        text: 'Z',
        anchor: 1,
        focus: 1,
    });
    assert.equal(before.slice(0, rep.start) + rep.text + before.slice(rep.end), 'Z\r\nc');

    // Replacement before a trailing selection keeps anchor/focus aligned in post-edit raw coordinates.
    bridge.sync(h, 'field', before, 0, 4, 7, false);
    h.textarea.value = 'AA\nb\nc';
    h.textarea.setSelectionRange(4, 6, 'backward');
    h.textarea.dispatchEvent({ type: 'input', isComposing: false });
    assert.deepEqual(events.pop(), {
        kind: 'edit',
        key: 'field',
        base: 0,
        expected: before,
        start: 0,
        end: 1,
        text: 'AA',
        anchor: 8,
        focus: 5,
    });

    bridge.destroy(h);
});

test('CRLF selection sync, reselect, and DOM select events translate offsets cleanly', () => {
    const { canvas, events, h } = setup();
    const before = 'a\r\nb\r\nc';

    bridge.sync(h, 'field', before, 0, 7, 3, false);
    bridge.activate(h);
    assert.equal(h.textarea.selectionStart, 2);
    assert.equal(h.textarea.selectionEnd, 5);
    assert.equal(h.textarea.selectionDirection, 'backward');
    assert.equal(events.length, 0);

    // Chrome blur collapses textarea selection; activate() restores the DOM-translated range.
    canvas.focus();
    h.textarea.selectionStart = 0;
    h.textarea.selectionEnd = 0;
    h.textarea.selectionDirection = 'forward';
    bridge.activate(h);
    assert.equal(h.textarea.selectionStart, 2);
    assert.equal(h.textarea.selectionEnd, 5);
    assert.equal(h.textarea.selectionDirection, 'backward');
    assert.equal(events.length, 0);

    // User selects all in the hidden textarea: selection event maps back to raw UTF-16 offsets.
    h.textarea.setSelectionRange(0, 5, 'forward');
    assert.deepEqual(events.pop(), {
        kind: 'selection',
        key: 'field',
        base: 0,
        expected: before,
        anchor: 0,
        focus: 7,
    });

    h.textarea.setSelectionRange(1, 4, 'backward');
    assert.deepEqual(events.pop(), {
        kind: 'selection',
        key: 'field',
        base: 0,
        expected: before,
        anchor: 6,
        focus: 1,
    });

    bridge.destroy(h);
});

test('lone CR, consecutive CRLFs, surrogate pairs, and IME preedit preserve untouched breaks', async () => {
    const { events, h } = setup();

    // Lone CR normalizes 1-to-1 in <textarea> without rewriting it on edit.
    const loneCr = 'a\rb';
    bridge.sync(h, 'field', loneCr, 0, 3, 3, false);
    bridge.activate(h);
    assert.equal(h.textarea.value, 'a\nb');
    insertAtCaret(h.textarea, 'x');
    assert.deepEqual(events.pop(), {
        kind: 'edit',
        key: 'field',
        base: 0,
        expected: loneCr,
        start: 3,
        end: 3,
        text: 'x',
        anchor: 4,
        focus: 4,
    });

    // Consecutive CRLFs and mixed lone CR + CRLF map offsets accurately.
    const mixed = '\r\r\n\r\n';
    bridge.sync(h, 'field', mixed, 0, 3, 5, false);
    assert.equal(h.textarea.value, '\n\n\n');
    assert.equal(h.textarea.selectionStart, 2);
    assert.equal(h.textarea.selectionEnd, 3);
    insertAtCaret(h.textarea, 'k');
    assert.deepEqual(events.pop(), {
        kind: 'edit',
        key: 'field',
        base: 0,
        expected: mixed,
        start: 3,
        end: 5,
        text: 'k',
        anchor: 4,
        focus: 4,
    });

    // Mixed lone CR + CRLF and surrogate pairs keep UTF-16 boundaries aligned.
    const emojiCrlf = '🦀\r\n🦀';
    bridge.sync(h, 'field', emojiCrlf, 0, 6, 6, false);
    assert.equal(h.textarea.selectionStart, 5);
    h.textarea.value = '🦀\n🦁';
    h.textarea.setSelectionRange(5, 5, 'forward');
    h.textarea.dispatchEvent({ type: 'input', isComposing: false });
    const emojiEdit = events.pop();
    assert.deepEqual(emojiEdit, {
        kind: 'edit',
        key: 'field',
        base: 0,
        expected: emojiCrlf,
        start: 4,
        end: 6,
        text: '🦁',
        anchor: 6,
        focus: 6,
    });
    assert.equal(emojiCrlf.slice(0, emojiEdit.start) + emojiEdit.text + emojiCrlf.slice(emojiEdit.end), '🦀\r\n🦁');

    // IME composition at the end of CRLF text emits translated preedit and commit ranges.
    const crlf = 'a\r\nb\r\nc';
    bridge.sync(h, 'field', crlf, 0, crlf.length, crlf.length, false);
    h.textarea.dispatchEvent({ type: 'compositionstart' });
    h.textarea.value = 'a\nb\ncに';
    h.textarea.setSelectionRange(6, 6, 'forward');
    h.textarea.dispatchEvent({ type: 'compositionupdate', data: 'に' });
    await Promise.resolve();
    assert.deepEqual(events.pop(), {
        kind: 'preedit',
        key: 'field',
        base: 0,
        expected: crlf,
        start: 7,
        end: 7,
        text: 'に',
        anchor: 8,
        focus: 8,
    });
    h.textarea.dispatchEvent({ type: 'compositionend' });
    assert.deepEqual(events.shift(), {
        kind: 'edit',
        key: 'field',
        base: 0,
        expected: crlf,
        start: 7,
        end: 7,
        text: 'に',
        anchor: 8,
        focus: 8,
    });
    assert.deepEqual(events.shift(), { kind: 'sync' });

    bridge.destroy(h);
});

test('syncA11y writes aria-pressed, aria-checked, and aria-selected cleanly across updates', () => {
    const { h } = setup();

    bridge.syncA11y(h, JSON.stringify([
        { k: 'button', p: 'wrap', r: [0, 0, 80, 24], t: 'Wrap', pr: 'false' },
        { k: 'button', p: 'mixed', r: [0, 28, 80, 24], t: 'Bold', pr: 'mixed' },
        { k: 'button', p: 'check', r: [0, 56, 80, 24], t: 'All', o: 'checkbox', c: 'mixed' },
        { k: 'button', p: 'sw', r: [0, 84, 80, 24], t: 'Switch', o: 'switch', c: 'true' },
        { k: 'button', p: 'tab', r: [0, 112, 80, 24], t: 'Files', o: 'tab', se: true },
    ]));

    const wrap = h.a11yNodes.get('button:wrap');
    assert.equal(wrap.getAttribute('role'), null);
    assert.equal(wrap.getAttribute('aria-pressed'), 'false');
    assert.equal(wrap.getAttribute('aria-checked'), null);
    assert.equal(wrap.getAttribute('aria-selected'), null);

    const mixed = h.a11yNodes.get('button:mixed');
    assert.equal(mixed.getAttribute('aria-pressed'), 'mixed');
    assert.equal(mixed.getAttribute('aria-checked'), null);

    const check = h.a11yNodes.get('button:check');
    assert.equal(check.getAttribute('role'), 'checkbox');
    assert.equal(check.getAttribute('aria-checked'), 'mixed');
    assert.equal(check.getAttribute('aria-pressed'), null);

    const sw = h.a11yNodes.get('button:sw');
    assert.equal(sw.getAttribute('role'), 'switch');
    assert.equal(sw.getAttribute('aria-checked'), 'true');
    assert.equal(sw.getAttribute('aria-pressed'), null);

    const tab = h.a11yNodes.get('button:tab');
    assert.equal(tab.getAttribute('role'), 'tab');
    assert.equal(tab.getAttribute('aria-selected'), 'true');
    assert.equal(tab.getAttribute('aria-checked'), null);
    assert.equal(tab.getAttribute('aria-pressed'), null);

    // Flip toggle button to pressed=true, then clear pressed, and switch checkbox to toggle button.
    bridge.syncA11y(h, JSON.stringify([
        { k: 'button', p: 'wrap', r: [0, 0, 80, 24], t: 'Wrap', pr: 'true' },
        { k: 'button', p: 'mixed', r: [0, 28, 80, 24], t: 'Bold' },
        { k: 'button', p: 'check', r: [0, 56, 80, 24], t: 'All', pr: 'false' },
    ]));
    assert.equal(wrap.getAttribute('aria-pressed'), 'true');
    assert.equal(mixed.getAttribute('aria-pressed'), null);
    assert.equal(check.getAttribute('role'), null);
    assert.equal(check.getAttribute('aria-checked'), null);
    assert.equal(check.getAttribute('aria-pressed'), 'false');

    bridge.destroy(h);
});

test('syncA11y sets aria-description on inline link anchors while preserving text and href', () => {
    const { h } = setup();

    bridge.syncA11y(h, JSON.stringify([
        {
            k: 'text',
            p: 'root/p',
            r: [0, 0, 240, 20],
            u: [
                { t: 'Open ' },
                { t: 'Local app', h: 'http://localhost:8123/', p: 'root/p', i: 0, d: 'This address opens on your device.' },
                { t: ' or ' },
                { t: 'notes', h: '/notes', p: 'root/p', i: 1, d: 'Local notes' },
                { t: ' or ' },
                { t: 'plain', h: 'https://example.com/', p: 'root/p', i: 2 },
            ],
        },
    ]));

    const p = h.a11yNodes.get('p:root/p');
    const anchors = p.children.filter(c => c.tagName === 'A');
    assert.equal(anchors.length, 3);

    assert.equal(anchors[0].textContent, 'Local app');
    assert.equal(anchors[0].href, 'http://localhost:8123/');
    assert.equal(anchors[0].target, '_blank');
    assert.equal(anchors[0].getAttribute('aria-description'), 'This address opens on your device.');
    assert.equal(anchors[0].getAttribute('title'), null);

    assert.equal(anchors[1].textContent, 'notes');
    assert.equal(anchors[1].href, '/notes');
    assert.equal(anchors[1].target, undefined);
    assert.equal(anchors[1].getAttribute('aria-description'), 'Local notes');

    assert.equal(anchors[2].textContent, 'plain');
    assert.equal(anchors[2].href, 'https://example.com/');
    assert.equal(anchors[2].getAttribute('aria-description'), null);

    // Updating or clearing a run description rebuilds the anchor attributes cleanly.
    bridge.syncA11y(h, JSON.stringify([
        {
            k: 'text',
            p: 'root/p',
            r: [0, 0, 240, 20],
            u: [
                { t: 'Local app', h: 'http://localhost:8123/', p: 'root/p', i: 0 },
            ],
        },
    ]));
    const updated = p.children.filter(c => c.tagName === 'A');
    assert.equal(updated.length, 1);
    assert.equal(updated[0].textContent, 'Local app');
    assert.equal(updated[0].href, 'http://localhost:8123/');
    assert.equal(updated[0].getAttribute('aria-description'), null);

    bridge.destroy(h);
});

test('focusA11y writes aria-pressed on focused toggle buttons and keeps aria-checked/aria-selected separate', () => {
    const { h } = setup();

    bridge.focusA11y(h, JSON.stringify({ o: 'button', t: 'Wrap', pr: 'true' }));
    let proxy = h.focusProxies[h.focusTurn];
    assert.equal(proxy.getAttribute('role'), 'button');
    assert.equal(proxy.getAttribute('aria-pressed'), 'true');
    assert.equal(proxy.getAttribute('aria-checked'), null);
    assert.equal(proxy.getAttribute('aria-selected'), null);

    bridge.focusA11y(h, JSON.stringify({ o: 'checkbox', t: 'All', c: 'mixed' }));
    proxy = h.focusProxies[h.focusTurn];
    assert.equal(proxy.getAttribute('role'), 'checkbox');
    assert.equal(proxy.getAttribute('aria-checked'), 'mixed');
    assert.equal(proxy.getAttribute('aria-pressed'), null);

    bridge.focusA11y(h, JSON.stringify({ o: 'tab', t: 'Files', s: true }));
    proxy = h.focusProxies[h.focusTurn];
    assert.equal(proxy.getAttribute('role'), 'tab');
    assert.equal(proxy.getAttribute('aria-selected'), 'true');
    assert.equal(proxy.getAttribute('aria-checked'), null);
    assert.equal(proxy.getAttribute('aria-pressed'), null);

    bridge.destroy(h);
});

test('focusA11y exposes combobox values via textContent, range values via aria-valuetext, and clears stale values', () => {
    const { h } = setup();

    bridge.focusA11y(h, JSON.stringify({ o: 'combobox', t: 'Message content preset', x: false, v: 'Balanced' }));
    const first = h.focusProxies[h.focusTurn];
    assert.equal(first.getAttribute('role'), 'combobox');
    assert.equal(first.getAttribute('aria-label'), 'Message content preset');
    assert.equal(first.getAttribute('aria-expanded'), 'false');
    assert.equal(first.getAttribute('aria-valuetext'), null);
    assert.equal(first.textContent, 'Balanced');

    bridge.focusA11y(h, JSON.stringify({ o: 'slider', t: 'Volume', v: '50%' }));
    const second = h.focusProxies[h.focusTurn];
    assert.equal(second.getAttribute('role'), 'slider');
    assert.equal(second.getAttribute('aria-label'), 'Volume');
    assert.equal(second.getAttribute('aria-valuetext'), '50%');
    assert.equal(second.textContent, '');

    // Reusing the first proxy slot for a control without a value clears the old textContent.
    bridge.focusA11y(h, JSON.stringify({ o: 'button', t: 'Save' }));
    assert.equal(h.focusProxies[h.focusTurn], first);
    assert.equal(first.getAttribute('role'), 'button');
    assert.equal(first.getAttribute('aria-label'), 'Save');
    assert.equal(first.getAttribute('aria-valuetext'), null);
    assert.equal(first.textContent, '');

    // Reusing the second proxy slot clears aria-valuetext too.
    bridge.focusA11y(h, JSON.stringify({ o: 'combobox', t: 'Message content preset', x: false }));
    assert.equal(h.focusProxies[h.focusTurn], second);
    assert.equal(second.getAttribute('role'), 'combobox');
    assert.equal(second.getAttribute('aria-label'), 'Message content preset');
    assert.equal(second.getAttribute('aria-valuetext'), null);
    assert.equal(second.textContent, '');

    bridge.destroy(h);
});

test('syncA11y mirrors combobox name in aria-label and selected value in textContent across updates', () => {
    const { h } = setup();

    bridge.syncA11y(h, JSON.stringify([
        { k: 'button', p: 'preset', r: [0, 0, 140, 28], t: 'Message content preset', o: 'combobox', x: false, v: 'Balanced' },
    ]));
    const el = h.a11yNodes.get('button:preset');
    assert.equal(el.getAttribute('role'), 'combobox');
    assert.equal(el.getAttribute('aria-label'), 'Message content preset');
    assert.equal(el.getAttribute('aria-expanded'), 'false');
    assert.equal(el.textContent, 'Balanced');

    // Updating the selected value updates textContent while preserving the label.
    bridge.syncA11y(h, JSON.stringify([
        { k: 'button', p: 'preset', r: [0, 0, 140, 28], t: 'Message content preset', o: 'combobox', x: false, v: 'Everything' },
    ]));
    assert.equal(el.getAttribute('aria-label'), 'Message content preset');
    assert.equal(el.textContent, 'Everything');

    // Clearing the value empties textContent without turning the label into the value.
    bridge.syncA11y(h, JSON.stringify([
        { k: 'button', p: 'preset', r: [0, 0, 140, 28], t: 'Message content preset', o: 'combobox', x: false },
    ]));
    assert.equal(el.getAttribute('aria-label'), 'Message content preset');
    assert.equal(el.textContent, '');

    // Changing back to a plain button clears aria-label and restores label textContent.
    bridge.syncA11y(h, JSON.stringify([
        { k: 'button', p: 'preset', r: [0, 0, 140, 28], t: 'Apply' },
    ]));
    assert.equal(el.getAttribute('role'), null);
    assert.equal(el.getAttribute('aria-label'), null);
    assert.equal(el.textContent, 'Apply');

    bridge.destroy(h);
});

test('secureKind falls back to typing when Intl.Segmenter is unavailable', () => {
    const fakeInput = { value: '👨‍👩‍👧‍👦', matches: () => false };
    assert.equal(bridge.secureKind({ inputType: 'insertText', data: '👨‍👩‍👧‍👦' }, fakeInput, null), 'secure');
    assert.equal(bridge.secureKind({ inputType: 'insertText', data: 'ab' }, fakeInput, null), 'secure');
    assert.equal(bridge.secureKind({ inputType: 'insertReplacementText', data: 'ab' }, fakeInput, null), 'secure_replace');
    assert.equal(bridge.secureKind({}, fakeInput, null), 'secure_replace');
    assert.equal(
        bridge.secureKind({ inputType: 'insertText', data: 'x' }, { value: 'x', matches: s => s === ':autofill' }, null),
        'secure_replace',
    );
});

test('syncA11y preserves status/alert roles and aria-live, keeps node identity, and skips unchanged writes', () => {
    const { h } = setup();

    bridge.syncA11y(h, JSON.stringify([
        { k: 'text', p: '#0/save-status', r: [0, 0, 120, 20], t: 'Changes saved', o: 'status', lv: 'polite' },
        { k: 'text', p: '#0/err', r: [0, 24, 120, 20], t: 'Failed', o: 'alert', lv: 'assertive' },
        { k: 'text', p: '#0/muted', r: [0, 48, 120, 20], t: 'Muted', o: 'alert', lv: 'off' },
        { k: 'text', p: '#0/live-only', r: [0, 72, 120, 20], t: 'Hint', lv: 'assertive' },
        { k: 'group', g: 'div', p: '#0/toast', r: [0, 96, 200, 40], t: 'Notice', o: 'status', lv: 'polite' },
        { k: 'text', p: '#0/toast/body', r: [0, 100, 180, 20], t: '3 files written', a: 4 },
    ]));

    const statusEl = h.a11yNodes.get('p:#0/save-status');
    assert.equal(statusEl.getAttribute('role'), 'status');
    assert.equal(statusEl.getAttribute('aria-live'), 'polite');
    assert.equal(statusEl.textContent, 'Changes saved');

    const alertEl = h.a11yNodes.get('p:#0/err');
    assert.equal(alertEl.getAttribute('role'), 'alert');
    assert.equal(alertEl.getAttribute('aria-live'), 'assertive');
    assert.equal(alertEl.textContent, 'Failed');

    const mutedEl = h.a11yNodes.get('p:#0/muted');
    assert.equal(mutedEl.getAttribute('role'), 'alert');
    assert.equal(mutedEl.getAttribute('aria-live'), 'off');

    const liveOnlyEl = h.a11yNodes.get('p:#0/live-only');
    assert.equal(liveOnlyEl.getAttribute('role'), null);
    assert.equal(liveOnlyEl.getAttribute('aria-live'), 'assertive');

    const toastEl = h.a11yNodes.get('div:#0/toast');
    assert.equal(toastEl.getAttribute('role'), 'status');
    assert.equal(toastEl.getAttribute('aria-live'), 'polite');
    assert.equal(toastEl.getAttribute('aria-label'), 'Notice');
    assert.equal(toastEl.children.length, 1);
    assert.equal(toastEl.children[0].textContent, '3 files written');

    // Spy on textContent writes and replaceChildren calls to prove unchanged repaints do not rewrite.
    let textWrites = 0;
    let replaceCalls = 0;
    let currentText = statusEl.textContent;
    Object.defineProperty(statusEl, 'textContent', {
        get() { return currentText; },
        set(v) { textWrites += 1; currentText = v; },
        configurable: true,
    });
    const origReplace = statusEl.replaceChildren.bind(statusEl);
    statusEl.replaceChildren = (...args) => { replaceCalls += 1; return origReplace(...args); };

    // Repaint with unchanged status text (only another node changed): no rewrite.
    bridge.syncA11y(h, JSON.stringify([
        { k: 'text', p: '#0/save-status', r: [0, 0, 120, 20], t: 'Changes saved', o: 'status', lv: 'polite' },
        { k: 'text', p: '#0/err', r: [0, 24, 120, 20], t: 'Retry failed', o: 'alert', lv: 'assertive' },
    ]));
    assert.equal(h.a11yNodes.get('p:#0/save-status'), statusEl);
    assert.equal(textWrites, 0, 'unchanged status text must not be rewritten on repaint');
    assert.equal(replaceCalls, 0, 'unchanged status node must not call replaceChildren on repaint');
    assert.equal(h.a11yNodes.get('p:#0/err'), alertEl);
    assert.equal(alertEl.textContent, 'Retry failed');

    // Update status text: same DOM node, single textContent write.
    bridge.syncA11y(h, JSON.stringify([
        { k: 'text', p: '#0/save-status', r: [0, 0, 120, 20], t: 'All changes saved', o: 'status', lv: 'polite' },
    ]));
    assert.equal(h.a11yNodes.get('p:#0/save-status'), statusEl, 'status updates must keep the same DOM node');
    assert.equal(statusEl.textContent, 'All changes saved');
    assert.equal(textWrites, 1);
    assert.equal(replaceCalls, 0, 'plain text status update writes textContent directly without extra replaceChildren');

    // Dropping role and live removes the attributes cleanly.
    bridge.syncA11y(h, JSON.stringify([
        { k: 'text', p: '#0/save-status', r: [0, 0, 120, 20], t: 'All changes saved' },
    ]));
    assert.equal(h.a11yNodes.get('p:#0/save-status'), statusEl);
    assert.equal(statusEl.getAttribute('role'), null);
    assert.equal(statusEl.getAttribute('aria-live'), null);

    bridge.destroy(h);
});

test('syncA11y mirrors radiogroup container with child radios and determinate/indeterminate progressbar', () => {
    const { h } = setup();

    bridge.syncA11y(h, JSON.stringify([
        { k: 'group', g: 'div', p: '#0/theme', r: [0, 0, 200, 60], o: 'radiogroup', t: 'Theme' },
        { k: 'button', p: '#0/theme/r0', r: [0, 0, 100, 24], o: 'radio', t: 'Light', c: 'true', a: 0 },
        { k: 'button', p: '#0/theme/r1', r: [0, 28, 100, 24], o: 'radio', t: 'Dark', c: 'false', a: 0 },
        { k: 'progressbar', p: '#0/usage', r: [0, 68, 240, 24], o: 'progressbar', t: '5 hours plan usage', rg: [0, 1, 0.42], vt: '42%' },
        { k: 'progressbar', p: '#0/loading', r: [0, 100, 240, 24], o: 'progressbar', t: 'Syncing' },
    ]));

    const group = h.a11yNodes.get('div:#0/theme');
    assert.equal(group.getAttribute('role'), 'radiogroup');
    assert.equal(group.getAttribute('aria-label'), 'Theme');
    assert.equal(group.children.length, 2);
    assert.equal(group.children[0].getAttribute('role'), 'radio');
    assert.equal(group.children[0].getAttribute('aria-checked'), 'true');
    assert.equal(group.children[1].getAttribute('role'), 'radio');
    assert.equal(group.children[1].getAttribute('aria-checked'), 'false');

    const usage = h.a11yNodes.get('div:#0/usage');
    assert.equal(usage.getAttribute('role'), 'progressbar');
    assert.equal(usage.getAttribute('aria-label'), '5 hours plan usage');
    assert.equal(usage.getAttribute('aria-valuemin'), '0');
    assert.equal(usage.getAttribute('aria-valuemax'), '1');
    assert.equal(usage.getAttribute('aria-valuenow'), '0.42');
    assert.equal(usage.getAttribute('aria-valuetext'), '42%');
    assert.equal(usage.textContent, undefined);

    const loading = h.a11yNodes.get('div:#0/loading');
    assert.equal(loading.getAttribute('role'), 'progressbar');
    assert.equal(loading.getAttribute('aria-label'), 'Syncing');
    assert.equal(loading.getAttribute('aria-valuemin'), null);
    assert.equal(loading.getAttribute('aria-valuemax'), null);
    assert.equal(loading.getAttribute('aria-valuenow'), null);
    assert.equal(loading.getAttribute('aria-valuetext'), null);

    // Update progressbar value in-place, then switch it to indeterminate.
    bridge.syncA11y(h, JSON.stringify([
        { k: 'progressbar', p: '#0/usage', r: [0, 68, 240, 24], o: 'progressbar', t: '5 hours plan usage', rg: [0, 1, 0.75], vt: '75%' },
    ]));
    assert.equal(h.a11yNodes.get('div:#0/usage'), usage);
    assert.equal(usage.getAttribute('aria-valuenow'), '0.75');
    assert.equal(usage.getAttribute('aria-valuetext'), '75%');

    bridge.syncA11y(h, JSON.stringify([
        { k: 'progressbar', p: '#0/usage', r: [0, 68, 240, 24], o: 'progressbar', t: '5 hours plan usage' },
    ]));
    assert.equal(h.a11yNodes.get('div:#0/usage'), usage);
    assert.equal(usage.getAttribute('aria-valuemin'), null);
    assert.equal(usage.getAttribute('aria-valuemax'), null);
    assert.equal(usage.getAttribute('aria-valuenow'), null);
    assert.equal(usage.getAttribute('aria-valuetext'), null);

    bridge.destroy(h);
});

test('keydown forwards non-browser function keys on focused textarea and password fields', () => {
    const { events, h } = setup();

    bridge.sync(h, 'field', 'hello', 0, 5, 5, false);
    bridge.activate(h);

    // Every F1–F24 except browser-reserved F5 and F12 is prevented and forwarded.
    for (let n = 1; n <= 24; n++) {
        if (n === 5 || n === 12) continue;
        const name = `F${n}`;
        events.length = 0;
        assert.equal(dispatchKeydown(h.textarea, { key: name, code: name }), true, `${name} should be prevented`);
        assert.deepEqual(events, [{
            kind: 'key',
            key: name,
            code: name,
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
            repeat: false,
        }]);
    }

    // Shift/Alt modifiers and repeat pass through for non-chord function keys.
    events.length = 0;
    assert.equal(dispatchKeydown(h.textarea, { key: 'F10', code: 'F10', shiftKey: true }), true);
    assert.equal(dispatchKeydown(h.textarea, { key: 'F8', code: 'F8', altKey: true, repeat: true }), true);
    assert.deepEqual(events, [
        { kind: 'key', key: 'F10', code: 'F10', shift: true, ctrl: false, alt: false, meta: false, repeat: false },
        { kind: 'key', key: 'F8', code: 'F8', shift: false, ctrl: false, alt: true, meta: false, repeat: true },
    ]);

    // Focused password fields forward non-browser function keys too.
    events.length = 0;
    bridge.sync(h, 'secret', '', 0, 0, 0, true);
    bridge.activate(h);
    assert.equal(dispatchKeydown(h.password, { key: 'F8', code: 'F8' }), true);
    assert.deepEqual(events, [{
        kind: 'key',
        key: 'F8',
        code: 'F8',
        shift: false,
        ctrl: false,
        alt: false,
        meta: false,
        repeat: false,
    }]);

    bridge.destroy(h);
});

test('keydown leaves browser-reserved function keys, non-function keys, and composing function keys untouched', () => {
    const { events, h } = setup();

    bridge.sync(h, 'field', 'hello', 0, 5, 5, false);
    bridge.activate(h);

    // Browser-reserved F5 and F12 stay un-prevented and unforwarded on both fields.
    for (const key of ['F5', 'F12']) {
        assert.equal(dispatchKeydown(h.textarea, { key, code: key }), false);
        assert.equal(dispatchKeydown(h.textarea, { key, code: key, shiftKey: true }), false);
    }
    bridge.sync(h, 'secret', '', 0, 0, 0, true);
    bridge.activate(h);
    for (const key of ['F5', 'F12']) {
        assert.equal(dispatchKeydown(h.password, { key, code: key }), false);
    }

    // Out-of-range F-names and regular keys must not be intercepted.
    bridge.sync(h, 'field', 'hello', 0, 5, 5, false);
    bridge.activate(h);
    for (const key of ['F', 'f8', 'F0', 'F01', 'F25', 'Fn', 'a']) {
        assert.equal(dispatchKeydown(h.textarea, { key, code: key }), false);
    }

    // IME composition owns function keys (e.g. Japanese kana conversion F6–F10).
    assert.equal(dispatchKeydown(h.textarea, { key: 'F8', code: 'F8', isComposing: true }), false);
    assert.equal(dispatchKeydown(h.textarea, { key: 'F8', code: 'F8', keyCode: 229 }), false);
    h.textarea.dispatchEvent({ type: 'compositionstart' });
    assert.equal(dispatchKeydown(h.textarea, { key: 'F8', code: 'F8' }), false);

    assert.equal(events.length, 0);
    bridge.destroy(h);
});

test('page mode mouse release focusing a field via sync + activate focuses textarea and restores selection', () => {
    const { doc, body, events, h } = setup({ page: true });
    assert.equal(doc.activeElement, body);

    // PrimaryDown on a button in page mode calls activate() with no focused field; it leaves
    // document.activeElement on the page (not the canvas).
    bridge.activate(h);
    assert.equal(doc.activeElement, body);

    // Without activate() (sync_input(false)), sync() updates value/selection but leaves textarea
    // unfocused because activeElement is not canvas.
    bridge.sync(h, 'editor', 'Recalled text', 0, 13, 13, false);
    assert.equal(doc.activeElement, body);
    assert.equal(h.textarea.inert, false);
    assert.equal(h.textarea.value, 'Recalled text');

    // Simulate Chrome collapsing an unfocused textarea's selection to 0 when focus moves to it.
    h.textarea.onFocus = () => {
        h.textarea.selectionStart = 0;
        h.textarea.selectionEnd = 0;
        h.textarea.selectionDirection = 'none';
    };
    bridge.activate(h);
    h.textarea.onFocus = null;

    assert.equal(doc.activeElement, h.textarea);
    assert.equal(h.textarea.inputMode, 'text');
    assert.equal(h.textarea.selectionStart, 13);
    assert.equal(h.textarea.selectionEnd, 13);
    assert.equal(h.textarea.selectionDirection, 'forward');

    insertAtCaret(h.textarea, '!');
    assert.equal(events.length, 1);
    assert.deepEqual(events[0], {
        kind: 'edit',
        key: 'editor',
        base: 0,
        expected: 'Recalled text',
        start: 13,
        end: 13,
        text: '!',
        anchor: 14,
        focus: 14,
    });

    // Blur back to body and focus a secure field on release.
    bridge.sync(h, '', '', 0, 0, 0, false);
    body.focus();
    bridge.sync(h, 'secret', '', 0, 0, 0, true);
    assert.equal(doc.activeElement, body);
    bridge.activate(h);
    assert.equal(doc.activeElement, h.password);
    assert.equal(h.password.inert, false);
    assert.equal(h.password.inputMode, 'text');
    bridge.destroy(h);
});

test('coarse pointer release focusing a field via sync + activate has inputMode text at focus time', async () => {
    const { doc, canvas, h } = setup({ coarse: true });
    await Promise.resolve();
    assert.equal(doc.activeElement, canvas);

    // Blur resets inputMode to none on coarse pointers.
    bridge.sync(h, '', '', 0, 0, 0, false);
    assert.equal(h.textarea.inputMode, 'none');

    // sync() sets inputMode text before focusing so iOS Safari opens the virtual keyboard at focus time.
    bridge.sync(h, 'editor', 'Recalled text', 0, 13, 13, false);
    assert.equal(doc.activeElement, h.textarea);
    assert.equal(h.textarea.inputMode, 'text');

    bridge.activate(h);
    assert.equal(doc.activeElement, h.textarea);
    assert.equal(h.textarea.inputMode, 'text');
    assert.equal(h.textarea.selectionStart, 13);
    assert.equal(h.textarea.selectionEnd, 13);
    bridge.destroy(h);
});

function snapshotDom(node) {
    const attrs = [...node._attrs.entries()].sort((a, b) => a[0].localeCompare(b[0]));
    return {
        tag: node.tagName,
        attrs,
        cssText: node.style.cssText,
        transform: node.style.transform ?? '',
        text: node.children.length === 0 ? (node.textContent ?? '') : '',
        children: node.children.map(snapshotDom),
    };
}

function parseCssPx(css, prop) {
    const m = new RegExp('(?:^|;)' + prop + ':(-?\\d+(?:\\.\\d+)?)px').exec(css);
    return m ? Number(m[1]) : 0;
}

function parseTranslate3d(tr) {
    if (!tr) return [0, 0];
    const m = /translate3d\((-?\d+(?:\.\d+)?)px,\s*(-?\d+(?:\.\d+)?)px/.exec(tr);
    return m ? [Number(m[1]), Number(m[2])] : [0, 0];
}

function collectScreenBounds(node, origin = [0, 0], out = []) {
    const [tx, ty] = parseTranslate3d(node.style.transform);
    const x = Math.round((origin[0] + parseCssPx(node.style.cssText, 'left') + tx) * 10) / 10;
    const y = Math.round((origin[1] + parseCssPx(node.style.cssText, 'top') + ty) * 10) / 10;
    const w = parseCssPx(node.style.cssText, 'width');
    const h = parseCssPx(node.style.cssText, 'height');
    if (node.getAttribute('data-rune-scroll-content') === null) {
        out.push({ tag: node.tagName, text: node.children.length === 0 ? (node.textContent ?? '') : '', rect: [x, y, w, h] });
    }
    for (const child of node.children) {
        if (child.tagName === '#TEXT' || child.tagName === 'STYLE') continue;
        collectScreenBounds(child, [x, y], out);
    }
    return out;
}

function makePatcher() {
    let sent = new Map();
    let orders = new Map();
    const TAGS = { heading: null, button: 'button', link: 'a', image: 'div', progressbar: 'div', text: 'p', scroll_x: 'div' };
    const keyOf = item => {
        const tag = item.k === 'group' ? (item.g || 'div') : item.k === 'heading' ? `h${item.l || 2}` : (TAGS[item.k] ?? 'p');
        return `${tag}:${item.p}`;
    };
    const r1 = v => { const r = Math.round(v * 10) / 10; return r === 0 ? 0 : r; };
    return items => {
        const keys = items.map(keyOf);
        const nextSent = new Map();
        const nextOrders = new Map();
        for (let i = 0; i < items.length; i++) {
            const item = items[i];
            const key = keys[i];
            const parentKey = item.a === undefined ? '' : keys[item.a];
            const origin = item.a === undefined ? [0, 0] : items[item.a].r;
            const rel = { ...item, r: [r1(item.r[0] - origin[0]), r1(item.r[1] - origin[1]), item.r[2], item.r[3]] };
            delete rel.a;
            nextSent.set(key, JSON.stringify(rel));
            if (!nextOrders.has(parentKey)) nextOrders.set(parentKey, []);
            nextOrders.get(parentKey).push(key);
        }
        const d = [...sent.keys()].filter(k => !nextSent.has(k));
        const u = [];
        for (const [k, raw] of nextSent) {
            if (sent.get(k) !== raw) u.push(JSON.parse(raw));
        }
        const o = {};
        for (const [pk, list] of nextOrders) {
            const prev = orders.get(pk);
            if (!prev || prev.length !== list.length || prev.some((v, idx) => v !== list[idx])) o[pk] = list;
        }
        sent = nextSent;
        orders = nextOrders;
        if (!d.length && !u.length && !Object.keys(o).length) return null;
        const patch = { p: 1 };
        if (d.length) patch.d = d;
        if (u.length) patch.u = u;
        if (Object.keys(o).length) patch.o = o;
        return patch;
    };
}

test('syncA11y incremental patches match full rebuild across virtual shifts, reparenting, tag changes, and sticky headers without mutating staying siblings', () => {
    const { h } = setup();
    const diff = makePatcher();

    const compareWithFull = items => {
        const patch = diff(items);
        if (patch) bridge.syncA11y(h, JSON.stringify(patch));
        const fresh = setup();
        bridge.syncA11y(fresh.h, JSON.stringify(items));
        assert.deepEqual(snapshotDom(h.a11y), snapshotDom(fresh.h.a11y));
        assert.deepEqual(collectScreenBounds(h.a11y), collectScreenBounds(fresh.h.a11y));
        bridge.destroy(fresh.h);
        return patch;
    };

    // Step 1: initial virtual scroll pane with sticky header + 4 rows.
    compareWithFull([
        { k: 'group', g: 'div', p: '#0/pane', r: [0, 20, 300, 200], sc: [0, 0] },
        { k: 'heading', l: 2, p: '#0/pane/sticky', r: [0, 20, 300, 28], t: 'Section', a: 0 },
        { k: 'text', p: '#0/pane/r0', r: [0, 48, 300, 32], t: 'Row 0', a: 0 },
        { k: 'text', p: '#0/pane/r1', r: [0, 80, 300, 32], t: 'Row 1', a: 0 },
        { k: 'text', p: '#0/pane/r2', r: [0, 112, 300, 32], t: 'Row 2', a: 0 },
        { k: 'text', p: '#0/pane/r3', r: [0, 144, 300, 32], t: 'Row 3', a: 0 },
    ]);

    // Spy on staying sibling r1 and r2 to prove zero mutations during scroll + virtual window shift.
    const r1 = h.a11yNodes.get('p:#0/pane/r1');
    const r2 = h.a11yNodes.get('p:#0/pane/r2');
    let r1Writes = 0;
    let r2Writes = 0;
    let r1Css = r1.style.cssText;
    let r2Css = r2.style.cssText;
    Object.defineProperty(r1.style, 'cssText', { get() { return r1Css; }, set(v) { r1Writes++; r1Css = v; }, configurable: true });
    Object.defineProperty(r2.style, 'cssText', { get() { return r2Css; }, set(v) { r2Writes++; r2Css = v; }, configurable: true });

    // Step 2: scroll down 64px, cull r0, mount r4, counter-shift sticky header by +64px.
    const scrollPatch = compareWithFull([
        { k: 'group', g: 'div', p: '#0/pane', r: [0, 20, 300, 200], sc: [0, 64] },
        { k: 'heading', l: 2, p: '#0/pane/sticky', r: [0, 84, 300, 28], t: 'Section', a: 0 },
        { k: 'text', p: '#0/pane/r1', r: [0, 80, 300, 32], t: 'Row 1', a: 0 },
        { k: 'text', p: '#0/pane/r2', r: [0, 112, 300, 32], t: 'Row 2', a: 0 },
        { k: 'text', p: '#0/pane/r3', r: [0, 144, 300, 32], t: 'Row 3', a: 0 },
        { k: 'text', p: '#0/pane/r4', r: [0, 176, 300, 32], t: 'Row 4', a: 0 },
    ]);
    assert.deepEqual(scrollPatch.d, ['p:#0/pane/r0']);
    assert.equal(r1Writes, 0, 'staying row r1 must not rewrite cssText on scroll');
    assert.equal(r2Writes, 0, 'staying row r2 must not rewrite cssText on scroll');
    assert.equal(h.a11yNodes.get('div:#0/pane')._scrollContent.style.transform, 'translate3d(0px,-64px,0)');

    // Check transformed screen bounds: sticky stays at y=20, r1 is at 80-64=16.
    const bounds = collectScreenBounds(h.a11y);
    assert.deepEqual(bounds.find(b => b.text === 'Section').rect, [0, 20, 300, 28]);
    assert.deepEqual(bounds.find(b => b.text === 'Row 1').rect, [0, 16, 300, 32]);

    // Step 3: reparent r2 out of pane into a new group, change sticky heading h2 -> h3,
    // and change r3 from text to button at the same path.
    compareWithFull([
        { k: 'group', g: 'section', p: '#0/box', r: [10, 0, 280, 100], t: 'Pinned' },
        { k: 'text', p: '#0/pane/r2', r: [14, 24, 260, 32], t: 'Row 2', a: 0 },
        { k: 'group', g: 'div', p: '#0/pane', r: [0, 20, 300, 200], sc: [0, 64] },
        { k: 'heading', l: 3, p: '#0/pane/sticky', r: [0, 84, 300, 28], t: 'Section', a: 2 },
        { k: 'text', p: '#0/pane/r1', r: [0, 80, 300, 32], t: 'Row 1', a: 2 },
        { k: 'button', p: '#0/pane/r3', r: [0, 144, 300, 32], t: 'Row 3 Action', a: 2 },
        { k: 'text', p: '#0/pane/r4', r: [0, 176, 300, 32], t: 'Row 4', a: 2 },
    ]);
    assert.equal(r1Writes, 0, 'staying row r1 still unmutated when sibling reparents/morphs');

    // Step 4: delete #0/box while reparenting r2 back to root, and toggle pane scroll wrapper off.
    compareWithFull([
        { k: 'text', p: '#0/pane/r2', r: [0, 0, 260, 32], t: 'Row 2' },
        { k: 'group', g: 'div', p: '#0/pane', r: [0, 20, 300, 200] },
        { k: 'text', p: '#0/pane/r1', r: [0, 80, 300, 32], t: 'Row 1', a: 1 },
        { k: 'button', p: '#0/pane/r3', r: [0, 144, 300, 32], t: 'Row 3 Action', a: 1 },
    ]);
    assert.equal(h.a11yNodes.get('div:#0/pane')._scrollContent, null);

    // Step 5: toggle pane scroll wrapper back on and reorder children.
    compareWithFull([
        { k: 'group', g: 'div', p: '#0/pane', r: [0, 20, 300, 200], sc: [12, 30] },
        { k: 'button', p: '#0/pane/r3', r: [0, 144, 300, 32], t: 'Row 3 Action', a: 0 },
        { k: 'text', p: '#0/pane/r1', r: [0, 80, 300, 32], t: 'Row 1', a: 0 },
    ]);

    // Step 6: 60 deterministic randomized transitions exercising virtual shifts, sticky headers,
    // reparenting (including out of deleted parents), tag morphing, and scroll wrapper toggles.
    let rng = 0x12345678;
    const next = () => { rng = (Math.imul(rng, 1664525) + 1013904223) >>> 0; return rng; };
    for (let step = 0; step < 60; step++) {
        const items = [];
        const hasBox = (next() % 2) === 0;
        let boxIdx;
        if (hasBox) {
            boxIdx = items.length;
            items.push({ k: 'group', g: (next() % 2) ? 'section' : 'div', p: '#0/box', r: [10, 10, 280, 60], t: 'Box' });
        }
        const paneIdx = items.length;
        const paneScroll = (next() % 4) !== 0;
        const sy = (next() % 30) * 16;
        const pane = { k: 'group', g: 'div', p: '#0/pane', r: [0, 80, 320, 240] };
        if (paneScroll) pane.sc = [(next() % 3) * 8, sy];
        items.push(pane);
        if ((next() % 3) !== 0) {
            items.push({ k: 'heading', l: 1 + (next() % 3), p: '#0/pane/sticky', r: [0, 80 + (paneScroll ? sy : 0), 300, 24], t: `H${step % 3}`, a: paneIdx });
        }
        const start = next() % 6;
        const len = next() % 6;
        const rows = Array.from({ length: len }, (_, k) => start + k);
        if (rows.length >= 2 && (next() % 2)) {
            const a = next() % rows.length;
            const b = next() % rows.length;
            [rows[a], rows[b]] = [rows[b], rows[a]];
        }
        for (const r of rows) {
            items.push({ k: 'text', p: `#0/pane/r${r}`, r: [8, 120 + r * 32, 300, 28], t: `R${r}-${(next() % 3) === 0 ? step : 0}`, a: paneIdx });
        }
        const morphParent = (next() % 3 === 0 && boxIdx !== undefined) ? boxIdx : (next() % 2 === 0 ? paneIdx : undefined);
        const mode = next() % 4;
        const morph = mode === 0
            ? { k: 'button', p: '#0/morph', r: [12, 90, 100, 24], o: 'combobox', t: 'Preset', x: (next() % 2) === 0, v: `V${next() % 3}` }
            : mode === 1
                ? { k: 'text', p: '#0/morph', r: [12, 90, 100, 24], o: 'status', lv: 'polite', t: `S${step % 2}` }
                : mode === 2
                    ? { k: 'progressbar', p: '#0/morph', r: [12, 90, 100, 24], o: 'progressbar', t: 'Load', rg: [0, 1, 0.5], vt: '50%' }
                    : { k: 'image', p: '#0/morph', r: [12, 90, 100, 24], t: 'Icon' };
        if (morphParent !== undefined) morph.a = morphParent;
        items.push(morph);
        compareWithFull(items);
    }

    bridge.destroy(h);
});

test('consumer demand triggers flush the mirror and timer helpers pass attempt and cancel cleanly', () => {
    const { win, canvas, events, h } = setup();

    // focusin inside h.a11y flushes immediately.
    h.a11y.dispatchEvent({ type: 'focusin' });
    assert.deepEqual(events.pop(), { kind: 'a11y_flush' });

    // beforeprint flushes immediately.
    win.dispatchEvent({ type: 'beforeprint' });
    assert.deepEqual(events.pop(), { kind: 'a11y_flush' });

    // Ctrl+F / Cmd+F flushes immediately.
    win.dispatchEvent({ type: 'keydown', key: 'f', ctrlKey: true, metaKey: false, altKey: false, target: canvas });
    assert.deepEqual(events.pop(), { kind: 'a11y_flush' });
    win.dispatchEvent({ type: 'keydown', key: 'F', ctrlKey: false, metaKey: true, altKey: false, target: canvas });
    assert.deepEqual(events.pop(), { kind: 'a11y_flush' });

    // Right-click armMenu flushes synchronously before secondary_browser.
    events.length = 0;
    win.dispatchEvent({ type: 'pointerup', button: 2, pointerType: 'mouse', target: canvas, clientX: 10, clientY: 10, shiftKey: false });
    assert.deepEqual(events.filter(e => e.kind !== 'mods'), [{ kind: 'a11y_flush' }, { kind: 'secondary_browser' }]);
    win.dispatchEvent({ type: 'pointerdown', button: 0, pointerType: 'mouse', target: canvas });

    // scheduleA11y / scheduleA11yIdle / cancelA11y carry attempt and cancel cleanly.
    const timers = [];
    const origSetTimeout = globalThis.setTimeout;
    const origClearTimeout = globalThis.clearTimeout;
    const origRic = globalThis.requestIdleCallback;
    const origCic = globalThis.cancelIdleCallback;
    let nextId = 1;
    const active = new Map();
    globalThis.setTimeout = (fn, ms) => { const id = nextId++; active.set(id, { fn, ms }); timers.push({ kind: 'timeout', id, ms }); return id; };
    globalThis.clearTimeout = id => { active.delete(id); };
    globalThis.requestIdleCallback = (fn, opts) => { const id = nextId++; active.set(id, { fn, opts }); timers.push({ kind: 'idle', id, opts }); return id; };
    globalThis.cancelIdleCallback = id => { active.delete(id); };
    try {
        events.length = 0;
        bridge.scheduleA11y(h, 4, 150);
        assert.equal(timers.at(-1).kind, 'timeout');
        assert.equal(timers.at(-1).ms, 150);
        const settleId = timers.at(-1).id;
        active.get(settleId).fn();
        assert.deepEqual(events.pop(), { kind: 'a11y_settle', attempt: 4 });

        bridge.scheduleA11yIdle(h, 5, 100);
        assert.equal(timers.at(-1).kind, 'idle');
        assert.deepEqual(timers.at(-1).opts, { timeout: 100 });
        const idleId = timers.at(-1).id;
        bridge.cancelA11y(h);
        assert.equal(active.has(idleId), false, 'cancelA11y cancels pending requestIdleCallback');

        timers.length = 0;
        bridge.scheduleA11yIdle(h, 6, 100);
        const backstop6 = timers.find(t => t.kind === 'timeout');
        const idleId2 = timers.at(-1).id;
        active.get(idleId2).fn();
        assert.deepEqual(events.pop(), { kind: 'a11y_idle', attempt: 6 });
        assert.equal(active.has(backstop6.id), false, 'firing requestIdleCallback clears its setTimeout backstop');

        // Stalled requestIdleCallback (e.g. idle WebGL2 frame with no next rAF): the bounded
        // setTimeout backstop fires a11y_idle once and cancels the stalled idle callback.
        timers.length = 0;
        bridge.scheduleA11yIdle(h, 7, 100);
        const backstop = timers.find(t => t.kind === 'timeout');
        const stalledIdle = timers.find(t => t.kind === 'idle');
        assert.ok(backstop && stalledIdle, 'scheduleA11yIdle arms both requestIdleCallback and setTimeout backstop');
        assert.ok(backstop.ms <= 16, `backstop fires within one 60 Hz frame, got ${backstop.ms}ms`);
        active.get(backstop.id).fn();
        assert.deepEqual(events.pop(), { kind: 'a11y_idle', attempt: 7 });
        assert.equal(active.has(stalledIdle.id), false, 'backstop cancels stalled requestIdleCallback');

        // cancelA11y cancels both requestIdleCallback and its setTimeout backstop.
        timers.length = 0;
        bridge.scheduleA11yIdle(h, 8, 100);
        const b8 = timers.find(t => t.kind === 'timeout');
        const i8 = timers.find(t => t.kind === 'idle');
        bridge.cancelA11y(h);
        assert.equal(active.has(b8.id), false, 'cancelA11y clears backstop timer');
        assert.equal(active.has(i8.id), false, 'cancelA11y clears requestIdleCallback');
    } finally {
        globalThis.setTimeout = origSetTimeout;
        globalThis.clearTimeout = origClearTimeout;
        globalThis.requestIdleCallback = origRic;
        globalThis.cancelIdleCallback = origCic;
    }

    bridge.destroy(h);
});

test('showMenu blocks activate while open and restores focus and selection to active field on close', () => {
    const { doc, win, body, events, h } = setup({ coarse: true });
    const spec = JSON.stringify({
        items: [
            { id: 'copy', title: 'Copy' },
            { id: 'paste', title: 'Paste' },
        ],
    });

    bridge.sync(h, 'editor', 'hello', 0, 5, 5, false);
    bridge.activate(h);
    assert.equal(doc.activeElement, h.textarea);

    bridge.showMenu(h, spec, 20, 40, 160, false);
    const menu = body.children.find(el => el.getAttribute('data-rune-menu') !== null);
    assert.ok(menu, 'popup menu should be mounted');
    assert.equal(doc.activeElement, menu);

    // Chrome collapses an unfocused textarea's selection to 0.
    h.textarea.selectionStart = 0;
    h.textarea.selectionEnd = 0;
    h.textarea.selectionDirection = 'none';

    // A stray activate() while the menu is open (such as lifting a finger after a long press)
    // must keep DOM focus in the menu.
    bridge.activate(h);
    assert.equal(doc.activeElement, menu);

    // Viewport resize dismisses the menu and must return DOM focus + selection to h.textarea.
    win.dispatchEvent({ type: 'resize' });
    assert.equal(body.children.find(el => el.getAttribute('data-rune-menu') !== null), undefined);
    assert.deepEqual(events, [{ kind: 'menu', id: null }]);
    assert.equal(doc.activeElement, h.textarea);
    assert.equal(h.textarea.inputMode, 'text');
    assert.equal(h.textarea.selectionStart, 5);
    assert.equal(h.textarea.selectionEnd, 5);
    assert.equal(h.textarea.selectionDirection, 'forward');

    // Picking a menu item also returns DOM focus + selection to h.textarea.
    events.length = 0;
    bridge.showMenu(h, spec, 20, 40, 160, false);
    const menu2 = body.children.find(el => el.getAttribute('data-rune-menu') !== null);
    assert.equal(doc.activeElement, menu2);
    h.textarea.selectionStart = 0;
    h.textarea.selectionEnd = 0;
    menu2.children[0].dispatchEvent({ type: 'click', preventDefault() {} });
    assert.deepEqual(events, [{ kind: 'menu', id: 'copy' }]);
    assert.equal(doc.activeElement, h.textarea);
    assert.equal(h.textarea.selectionStart, 5);
    assert.equal(h.textarea.selectionEnd, 5);

    // Secure field: dismissing the menu returns DOM focus to h.password.
    events.length = 0;
    bridge.sync(h, 'secret', '', 0, 0, 0, true);
    bridge.activate(h);
    assert.equal(doc.activeElement, h.password);
    bridge.showMenu(h, spec, 20, 40, 160, false);
    assert.notEqual(doc.activeElement, h.password);
    doc.dispatchEvent({ type: 'keydown', key: 'Escape', preventDefault() {}, stopPropagation() {} });
    assert.deepEqual(events, [{ kind: 'menu', id: null }]);
    assert.equal(doc.activeElement, h.password);
    assert.equal(h.password.inputMode, 'text');

    bridge.destroy(h);
});

test('showMenu dismissal falls back to canvas only when no field is focused and respects page mode', () => {
    const spec = JSON.stringify({ items: [{ id: 'a', title: 'Action' }] });
    const escape = () => ({ type: 'keydown', key: 'Escape', preventDefault() {}, stopPropagation() {} });

    // Standalone/app mode without a focused field: dismissal focuses the canvas.
    {
        const { doc, canvas, events, h } = setup();
        bridge.showMenu(h, spec, 10, 10, 160, false);
        assert.notEqual(doc.activeElement, canvas);
        doc.dispatchEvent(escape());
        assert.deepEqual(events, [{ kind: 'menu', id: null }]);
        assert.equal(doc.activeElement, canvas);
        bridge.destroy(h);
    }

    // Page mode without a focused field: dismissal must not focus the canvas.
    {
        const { doc, body, canvas, events, h } = setup({ page: true });
        bridge.showMenu(h, spec, 10, 10, 160, false);
        doc.dispatchEvent(escape());
        assert.deepEqual(events, [{ kind: 'menu', id: null }]);
        assert.notEqual(doc.activeElement, canvas);
        assert.equal(doc.activeElement, body);
        bridge.destroy(h);
    }

    // Page mode with a focused field: dismissal restores DOM focus and selection to h.textarea.
    {
        const { doc, events, h } = setup({ page: true });
        bridge.sync(h, 'editor', 'hello', 0, 2, 5, false);
        bridge.activate(h);
        bridge.showMenu(h, spec, 10, 10, 160, false);
        h.textarea.selectionStart = 0;
        h.textarea.selectionEnd = 0;
        doc.dispatchEvent(escape());
        assert.deepEqual(events, [{ kind: 'menu', id: null }]);
        assert.equal(doc.activeElement, h.textarea);
        assert.equal(h.textarea.selectionStart, 2);
        assert.equal(h.textarea.selectionEnd, 5);
        bridge.destroy(h);
    }
});

test('showMenu keeps its defaults without a style and draws with what a style sets', () => {
    const items = [{ id: 'a', title: 'Action' }, { separator: true }, { id: 'b', title: 'Off', enabled: false }];
    const { body, h } = setup();
    const mounted = () => body.children.find(el => el.getAttribute('data-rune-menu') !== null);

    bridge.showMenu(h, JSON.stringify({ items }), 10, 10, 160, true);
    let menu = mounted();
    assert.match(menu.style.cssText, /padding:4px;/);
    assert.match(menu.style.cssText, /background:#2b2b2b;/);
    assert.match(menu.style.cssText, /border-radius:8px;/);
    assert.match(menu.style.cssText, /font:13px\/18px system-ui,sans-serif;/);
    assert.match(menu.children[0].style.cssText, /border-radius:4px;/);
    bridge.hideMenu(h);

    const style = { bg: '#102030ff', fg: '#fafafaff', muted: '#808080ff', line: '#334455ff', fontFamily: 'Inter, sans-serif',
        fontSize: 15, lineHeight: 22, padding: 6, radius: 0, itemRadius: 2 };
    bridge.showMenu(h, JSON.stringify({ items, style }), 10, 10, 160, false);
    menu = mounted();
    assert.match(menu.style.cssText, /padding:6px;/);
    assert.match(menu.style.cssText, /background:#102030ff;color:#fafafaff;border:1px solid #334455ff;border-radius:0px;/);
    assert.match(menu.style.cssText, /font:13px\/18px system-ui,sans-serif;user-select:none;/);
    assert.equal(menu.style.fontSize, '15px');
    assert.equal(menu.style.lineHeight, '22px');
    assert.equal(menu.style.fontFamily, 'Inter, sans-serif');
    assert.match(menu.children[0].style.cssText, /border-radius:2px;.*color:#fafafaff;/);
    assert.match(menu.children[1].style.cssText, /background:#334455ff/);
    assert.match(menu.children[2].style.cssText, /color:#808080ff;/);
    assert.equal(menu.getAttribute('role'), 'menu');
    // An unset colour keeps the light default.
    menu.children[0].dispatchEvent({ type: 'pointerenter' });
    assert.equal(menu.children[0].style.background, '#ebebeb');
    bridge.hideMenu(h);

    // A family with an unclosed quote or comment stays out of cssText, so it can't swallow the
    // declarations after it; zero font sizes and colours that aren't `#rrggbbaa` keep defaults.
    for (const fontFamily of ['"Inter', "/* Inter"]) {
        bridge.showMenu(h, JSON.stringify({ items, style: { fontFamily, fontSize: 0, lineHeight: 0, bg: 'red;x:y' } }), 10, 10, 160, false);
        menu = mounted();
        assert.ok(!menu.style.cssText.includes(fontFamily));
        assert.match(menu.style.cssText, /background:#ffffff;/);
        assert.match(menu.style.cssText, /font:13px\/18px system-ui,sans-serif;user-select:none;-webkit-user-select:none;outline:none$/);
        assert.equal(menu.style.fontFamily, fontFamily);
        assert.equal(menu.style.fontSize, undefined);
        assert.equal(menu.style.lineHeight, undefined);
        bridge.hideMenu(h);
    }
    bridge.destroy(h);
});

test('takeTouchTimestamp captures PointerEvent.timeStamp in FIFO order, expands coalesced moves, clamps and bounds to 64', () => {
    const { win, canvas, h } = setup();
    const origPerf = globalThis.performance;
    let perfNow = 1000;
    Object.defineProperty(globalThis, 'performance', {
        value: { now: () => perfNow },
        configurable: true,
    });
    try {
        // Non-touch and non-canvas events are ignored.
        win.dispatchEvent({ type: 'pointerdown', target: canvas, pointerType: 'mouse', pointerId: 1, timeStamp: 100 });
        win.dispatchEvent({ type: 'pointerdown', target: {}, pointerType: 'touch', pointerId: 1, timeStamp: 100 });
        assert.ok(Number.isNaN(bridge.takeTouchTimestamp(h, 1)));

        // Coalesced pointermove expands in order; chorded button !== -1 is skipped.
        win.dispatchEvent({ type: 'pointerdown', target: canvas, pointerType: 'touch', pointerId: 7, timeStamp: 200 });
        win.dispatchEvent({
            type: 'pointermove',
            target: canvas,
            pointerType: 'touch',
            pointerId: 7,
            button: -1,
            timeStamp: 230,
            getCoalescedEvents: () => [{ timeStamp: 210 }, { timeStamp: 220 }],
        });
        win.dispatchEvent({ type: 'pointermove', target: canvas, pointerType: 'touch', pointerId: 7, button: 0, timeStamp: 225 });
        win.dispatchEvent({ type: 'pointerup', target: canvas, pointerType: 'touch', pointerId: 7, timeStamp: 1500 });
        assert.equal(bridge.takeTouchTimestamp(h, 7), 200);
        assert.equal(bridge.takeTouchTimestamp(h, 7), 210);
        assert.equal(bridge.takeTouchTimestamp(h, 7), 220);
        // Future timestamp (1500 > perfNow 1000) is clamped to performance.now().
        assert.equal(bridge.takeTouchTimestamp(h, 7), 1000);
        assert.ok(Number.isNaN(bridge.takeTouchTimestamp(h, 7)));

        // Malformed / non-positive timestamps return NaN.
        win.dispatchEvent({ type: 'pointerdown', target: canvas, pointerType: 'touch', pointerId: 9, timeStamp: 0 });
        win.dispatchEvent({ type: 'pointerup', target: canvas, pointerType: 'touch', pointerId: 9, timeStamp: -5 });
        assert.ok(Number.isNaN(bridge.takeTouchTimestamp(h, 9)));
        assert.ok(Number.isNaN(bridge.takeTouchTimestamp(h, 9)));

        // Bounded FIFO (max 64): pushing 70 drops the oldest 6.
        for (let i = 1; i <= 70; i++) {
            win.dispatchEvent({ type: 'pointermove', target: canvas, pointerType: 'touch', pointerId: 3, button: -1, timeStamp: i * 10 });
        }
        assert.equal(bridge.takeTouchTimestamp(h, 3), 70);
    } finally {
        Object.defineProperty(globalThis, 'performance', { value: origPerf, configurable: true });
        bridge.destroy(h);
    }
});

test('scheduleTouchHoldConfirm uses background postTask, re-yields up to 2 turns on isInputPending, and cancels cleanly', async () => {
    const { events, h } = setup();
    const origScheduler = globalThis.scheduler;
    const origNav = globalThis.navigator;
    const tasks = [];
    let pendingChecks = 0;
    let inputPendingCalls = 0;
    Object.defineProperty(globalThis, 'scheduler', {
        value: {
            postTask(fn, opts) {
                const entry = { fn, opts, aborted: false };
                opts?.signal?.addEventListener?.('abort', () => { entry.aborted = true; });
                tasks.push(entry);
                return new Promise((resolve, reject) => {
                    entry.run = () => {
                        if (entry.aborted) { reject(new Error('aborted')); return; }
                        fn();
                        resolve();
                    };
                });
            },
        },
        configurable: true,
    });
    Object.defineProperty(globalThis, 'navigator', {
        value: {
            ...origNav,
            scheduling: {
                isInputPending(opts) {
                    assert.deepEqual(opts, { includeContinuous: true });
                    inputPendingCalls += 1;
                    return pendingChecks-- > 0;
                },
            },
        },
        configurable: true,
    });
    try {
        // Re-yields at most 2 times when input stays pending, then confirms on turn 3.
        pendingChecks = 5;
        bridge.scheduleTouchHoldConfirm(h, 42);
        assert.equal(tasks.length, 1);
        assert.equal(tasks[0].opts.priority, 'background');
        tasks[0].run();
        assert.equal(events.length, 0);
        assert.equal(tasks.length, 2);
        tasks[1].run();
        assert.equal(events.length, 0);
        assert.equal(tasks.length, 3);
        tasks[2].run();
        assert.deepEqual(events, [{ kind: 'touch_hold_confirm', attempt: 42 }]);
        assert.equal(inputPendingCalls, 2);

        // Simulated 700 ms stall where queued pointerup drains and cancels confirmation before it runs.
        events.length = 0;
        tasks.length = 0;
        pendingChecks = 0;
        bridge.scheduleTouchHoldConfirm(h, 43);
        assert.equal(tasks.length, 1);
        bridge.cancelTouchHoldConfirm(h);
        assert.equal(tasks[0].aborted, true);
        tasks[0].run();
        assert.deepEqual(events, []);
    } finally {
        Object.defineProperty(globalThis, 'scheduler', { value: origScheduler, configurable: true });
        Object.defineProperty(globalThis, 'navigator', { value: origNav, configurable: true });
        bridge.destroy(h);
    }
});

test('scheduleTouchHoldConfirm falls back to setTimeout(0) when scheduler.postTask is unavailable and clears on destroy', async () => {
    const { events, h } = setup();
    const origScheduler = globalThis.scheduler;
    Object.defineProperty(globalThis, 'scheduler', { value: undefined, configurable: true });
    try {
        bridge.scheduleTouchHoldConfirm(h, 7);
        assert.equal(events.length, 0, 'must not confirm synchronously');
        await new Promise(resolve => setTimeout(resolve, 15));
        assert.deepEqual(events, [{ kind: 'touch_hold_confirm', attempt: 7 }]);

        // destroy cancels any in-flight fallback timer.
        events.length = 0;
        bridge.scheduleTouchHoldConfirm(h, 8);
        bridge.destroy(h);
        await new Promise(resolve => setTimeout(resolve, 15));
        assert.deepEqual(events, []);
    } finally {
        Object.defineProperty(globalThis, 'scheduler', { value: origScheduler, configurable: true });
    }
});

test('supportsLiveTouchContextMenu classifies Android/Linux/ChromeOS vs Windows/iOS/iPadOS/macOS', () => {
    const cases = [
        [{ userAgentData: { platform: 'Android' }, platform: 'Linux armv8l', userAgent: 'Mozilla/5.0 (Linux; Android 15)' }, true],
        [{ userAgentData: { platform: 'Chrome OS' }, platform: 'Linux x86_64', userAgent: 'Mozilla/5.0 (X11; CrOS x86_64)' }, true],
        [{ userAgentData: { platform: 'Linux' }, platform: 'Linux x86_64', userAgent: 'Mozilla/5.0 (X11; Linux x86_64)' }, true],
        [{ platform: 'Linux armv8l', userAgent: 'Mozilla/5.0 (Android 14; Mobile; rv:130.0)' }, true],
        // Windows Chrome defers touch contextmenu until finger lift -> false.
        [{ userAgentData: { platform: 'Windows' }, platform: 'Win32', userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)' }, false],
        // Spoofed Android UA on Windows Chrome where userAgentData still reports Windows -> false.
        [{ userAgentData: { platform: 'Windows' }, platform: 'Linux armv8l', userAgent: 'Mozilla/5.0 (Linux; Android 14)' }, false],
        // iOS / iPadOS (including iPadOS desktop mode MacIntel + maxTouchPoints > 1) -> false.
        [{ platform: 'iPhone', userAgent: 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)', maxTouchPoints: 5 }, false],
        [{ platform: 'MacIntel', userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)', maxTouchPoints: 5 }, false],
        // macOS -> false.
        [{ userAgentData: { platform: 'macOS' }, platform: 'MacIntel', userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)', maxTouchPoints: 0 }, false],
        // Missing / empty navigator -> false.
        [null, false],
        [{}, false],
    ];
    for (const [nav, want] of cases) {
        assert.equal(bridge.supportsLiveTouchContextMenu(nav), want, JSON.stringify(nav));
    }
});

test('document contextmenu forwards touch_hold only while a canvas touch is active and leaves mouse right-click alone', () => {
    const { doc, win, canvas, events, h } = setup();
    try {
        // Stray contextmenu with no active touch on canvas sends no touch_hold.
        let prevented = false;
        doc.dispatchEvent({ type: 'contextmenu', target: canvas, pointerType: 'touch', pointerId: 5, button: 0, preventDefault() { prevented = true; } });
        assert.equal(prevented, true);
        assert.deepEqual(events, []);

        // Active touch on canvas: contextmenu prevents default and forwards touch_hold with pointerId.
        win.dispatchEvent({ type: 'pointerdown', target: canvas, pointerType: 'touch', pointerId: 11, timeStamp: 100 });
        prevented = false;
        doc.dispatchEvent({ type: 'contextmenu', target: canvas, pointerType: 'touch', pointerId: 11, button: -1, preventDefault() { prevented = true; } });
        assert.equal(prevented, true);
        assert.deepEqual(events, [{ kind: 'touch_hold', id: 11 }]);

        // Mouse right-click contextmenu while touch is active is NOT treated as touch_hold.
        events.length = 0;
        doc.dispatchEvent({ type: 'contextmenu', target: canvas, pointerType: 'mouse', pointerId: 1, button: 2, preventDefault() {} });
        assert.deepEqual(events, []);

        // After pointerup clears activeTouchId, a deferred post-lift contextmenu sends no touch_hold.
        win.dispatchEvent({ type: 'pointerup', target: canvas, pointerType: 'touch', pointerId: 11, timeStamp: 900 });
        doc.dispatchEvent({ type: 'contextmenu', target: canvas, pointerType: 'touch', pointerId: 11, button: 0, preventDefault() {} });
        assert.deepEqual(events, []);
    } finally {
        bridge.destroy(h);
    }
});

test('keydown leaves media keys untouched until syncMediaKeys(true), then prevents and forwards them', () => {
    const { canvas, events, h } = setup();
    const mediaKeys = ['MediaPlayPause', 'MediaPlay', 'MediaPause', 'MediaTrackNext', 'MediaTrackPrevious', 'MediaStop', 'MediaFastForward', 'MediaRewind'];
    try {
        // Off by default: neither canvas nor focused fields prevent default or forward media keys.
        for (const name of mediaKeys) {
            assert.equal(dispatchKeydown(canvas, { key: name, code: name }), false, `canvas opt-out ${name}`);
        }
        bridge.sync(h, 'field', 'hello', 0, 5, 5, false);
        bridge.activate(h);
        for (const name of mediaKeys) {
            assert.equal(dispatchKeydown(h.textarea, { key: name, code: name }), false, `textarea opt-out ${name}`);
        }
        assert.deepEqual(events, []);

        // Opted in: canvas prevents default and focused textarea/password forward every media key.
        bridge.syncMediaKeys(h, true);
        for (const name of mediaKeys) {
            events.length = 0;
            assert.equal(dispatchKeydown(canvas, { key: name, code: name }), true, `canvas opt-in ${name}`);
            assert.equal(dispatchKeydown(h.textarea, { key: name, code: name }), true, `textarea opt-in ${name}`);
            assert.deepEqual(events, [{
                kind: 'key',
                key: name,
                code: name,
                shift: false,
                ctrl: false,
                alt: false,
                meta: false,
                repeat: false,
            }]);
        }
        events.length = 0;
        bridge.sync(h, 'secret', '', 0, 0, 0, true);
        bridge.activate(h);
        assert.equal(dispatchKeydown(h.password, { key: 'MediaPlayPause', code: 'MediaPlayPause' }), true);
        assert.equal(events.length, 1);
        assert.equal(events[0].key, 'MediaPlayPause');
    } finally {
        bridge.destroy(h);
    }
});

test('scheduleTouchHoldConfirm waits two animation frames before confirming so pointerup delivered after raf1 cancels the hold', () => {
    const { events, h } = setup();
    const origRAF = globalThis.requestAnimationFrame;
    const origCAF = globalThis.cancelAnimationFrame;
    const origScheduler = globalThis.scheduler;
    const origNav = globalThis.navigator;
    let nextRafId = 1;
    const rafs = new Map();
    const tasks = [];
    Object.defineProperty(globalThis, 'requestAnimationFrame', {
        value: cb => { const id = nextRafId++; rafs.set(id, cb); return id; },
        configurable: true,
    });
    Object.defineProperty(globalThis, 'cancelAnimationFrame', {
        value: id => { rafs.delete(id); },
        configurable: true,
    });
    Object.defineProperty(globalThis, 'scheduler', {
        value: {
            postTask(fn, opts) {
                const entry = { fn, opts, aborted: false };
                opts?.signal?.addEventListener?.('abort', () => { entry.aborted = true; });
                tasks.push(entry);
                return new Promise((resolve, reject) => {
                    entry.run = () => {
                        if (entry.aborted) { reject(new Error('aborted')); return; }
                        fn();
                        resolve();
                    };
                });
            },
        },
        configurable: true,
    });
    Object.defineProperty(globalThis, 'navigator', {
        value: { ...origNav, scheduling: { isInputPending: () => false } },
        configurable: true,
    });
    const flushOneRaf = () => {
        const batch = [...rafs.entries()];
        rafs.clear();
        for (const [, cb] of batch) cb(performance.now());
    };
    const flushTasks = () => {
        const batch = tasks.splice(0);
        for (const t of batch) t.run();
    };
    try {
        // Reproduce the measured Chromium post-stall order:
        // 1. Stale rAF schedules confirmation.
        // 2. Background tasks + raf1 + post-raf1 tasks all run with isInputPending() === false.
        // 3. Waiting pointerup arrives AFTER raf1 and calls cancelTouchHoldConfirm(h).
        // 4. raf2 + trailing tasks run: touch_hold_confirm must NOT have fired.
        bridge.scheduleTouchHoldConfirm(h, 99);
        flushTasks();
        assert.deepEqual(events, [], 'must not confirm on a background task before raf1');
        flushOneRaf(); // raf1 (overdue post-stall frame)
        flushTasks();
        assert.deepEqual(events, [], 'must not confirm after only raf1 before fresh post-stall raf2');

        // Waiting pointerup arrives after raf1 commits and cancels the hold.
        bridge.cancelTouchHoldConfirm(h);
        assert.equal(rafs.size, 0, 'cancelling must cancel pending raf2 so idle page sleeps');
        flushOneRaf();
        flushTasks();
        assert.deepEqual(events, []);

        // Genuine hold (no pointerup): confirms after raf1 -> raf2 -> background task.
        bridge.scheduleTouchHoldConfirm(h, 100);
        flushOneRaf(); // raf1
        assert.deepEqual(events, []);
        flushOneRaf(); // raf2
        assert.deepEqual(events, [], 'must wait for trailing background task after raf2');
        flushTasks();
        assert.deepEqual(events, [{ kind: 'touch_hold_confirm', attempt: 100 }]);
        assert.equal(rafs.size, 0);
    } finally {
        Object.defineProperty(globalThis, 'requestAnimationFrame', { value: origRAF, configurable: true });
        Object.defineProperty(globalThis, 'cancelAnimationFrame', { value: origCAF, configurable: true });
        Object.defineProperty(globalThis, 'scheduler', { value: origScheduler, configurable: true });
        Object.defineProperty(globalThis, 'navigator', { value: origNav, configurable: true });
        bridge.destroy(h);
    }
});

test('syncA11y mirrors container roles, aria-disabled without HTML disabled, aria-busy, aria-level, and range controls', () => {
    const { events, h } = setup();

    bridge.syncA11y(h, JSON.stringify([
        { k: 'group', g: 'div', p: '#0/menu', r: [0, 0, 180, 80], o: 'menu', t: 'Actions' },
        { k: 'button', p: '#0/menu/open', r: [0, 0, 180, 28], o: 'menuitem', t: 'Open', a: 0 },
        { k: 'button', p: '#0/menu/off', r: [0, 28, 180, 28], o: 'menuitem', t: 'Disabled item', dis: true, a: 0 },
        { k: 'group', g: 'div', p: '#0/tree', r: [0, 90, 240, 80], o: 'tree', t: 'Files', b: true },
        { k: 'button', p: '#0/tree/src', r: [0, 90, 240, 28], o: 'treeitem', t: 'src', x: true, b: true, l: 1, se: false, a: 3 },
        { k: 'group', g: 'div', p: '#0/panel', r: [0, 180, 240, 60], o: 'tabpanel', t: 'Day' },
        { k: 'button', p: '#0/panel/vol', r: [0, 184, 140, 24], o: 'slider', t: 'Volume', rg: [0, 100, 40], vt: '40%', a: 5 },
        { k: 'button', p: '#0/panel/retries', r: [0, 212, 140, 24], o: 'spinbutton', t: 'Retries', rg: [1, 10, 3], vt: '3 retries', dis: true, a: 5 },
    ]));

    const menu = h.a11yNodes.get('div:#0/menu');
    assert.equal(menu.getAttribute('role'), 'menu');
    assert.equal(menu.getAttribute('aria-label'), 'Actions');
    assert.equal(menu.children.length, 2);

    const open = h.a11yNodes.get('button:#0/menu/open');
    assert.equal(open.getAttribute('role'), 'menuitem');
    assert.equal(open.getAttribute('aria-disabled'), null);
    assert.equal(open.disabled, undefined);
    open.dispatchEvent({ type: 'click', preventDefault() {} });
    assert.deepEqual(events.pop(), { kind: 'a11y_press', path: '#0/menu/open' });

    const off = h.a11yNodes.get('button:#0/menu/off');
    assert.equal(off.getAttribute('role'), 'menuitem');
    assert.equal(off.getAttribute('aria-disabled'), 'true');
    assert.equal(off.disabled, undefined, 'disabled mirror controls stay discoverable without HTML disabled');
    events.length = 0;
    off.dispatchEvent({ type: 'click', preventDefault() {} });
    assert.deepEqual(events, [], 'clicking an aria-disabled mirror button sends no a11y_press');

    const tree = h.a11yNodes.get('div:#0/tree');
    assert.equal(tree.getAttribute('role'), 'tree');
    assert.equal(tree.getAttribute('aria-label'), 'Files');
    assert.equal(tree.getAttribute('aria-busy'), 'true');

    const src = h.a11yNodes.get('button:#0/tree/src');
    assert.equal(src.getAttribute('role'), 'treeitem');
    assert.equal(src.getAttribute('aria-expanded'), 'true');
    assert.equal(src.getAttribute('aria-busy'), 'true');
    assert.equal(src.getAttribute('aria-level'), '1');
    assert.equal(src.getAttribute('aria-selected'), 'false');

    const panel = h.a11yNodes.get('div:#0/panel');
    assert.equal(panel.getAttribute('role'), 'tabpanel');
    assert.equal(panel.getAttribute('aria-label'), 'Day');

    const vol = h.a11yNodes.get('button:#0/panel/vol');
    assert.equal(vol.getAttribute('role'), 'slider');
    assert.equal(vol.getAttribute('aria-label'), 'Volume');
    assert.equal(vol.getAttribute('aria-valuemin'), '0');
    assert.equal(vol.getAttribute('aria-valuemax'), '100');
    assert.equal(vol.getAttribute('aria-valuenow'), '40');
    assert.equal(vol.getAttribute('aria-valuetext'), '40%');
    assert.equal(vol.textContent, '');

    const retries = h.a11yNodes.get('button:#0/panel/retries');
    assert.equal(retries.getAttribute('role'), 'spinbutton');
    assert.equal(retries.getAttribute('aria-label'), 'Retries');
    assert.equal(retries.getAttribute('aria-valuemin'), '1');
    assert.equal(retries.getAttribute('aria-valuemax'), '10');
    assert.equal(retries.getAttribute('aria-valuenow'), '3');
    assert.equal(retries.getAttribute('aria-valuetext'), '3 retries');
    assert.equal(retries.getAttribute('aria-disabled'), 'true');
    assert.equal(retries.textContent, '');

    // Updating clears aria-disabled, aria-busy, aria-level, and range attributes cleanly in-place.
    bridge.syncA11y(h, JSON.stringify([
        { k: 'group', g: 'div', p: '#0/menu', r: [0, 0, 180, 80], o: 'menu', t: 'Actions' },
        { k: 'button', p: '#0/menu/off', r: [0, 28, 180, 28], o: 'menuitem', t: 'Enabled item', a: 0 },
        { k: 'group', g: 'div', p: '#0/tree', r: [0, 90, 240, 80], o: 'tree', t: 'Files' },
        { k: 'button', p: '#0/tree/src', r: [0, 90, 240, 28], o: 'treeitem', t: 'src', x: false, se: true, a: 2 },
        { k: 'group', g: 'div', p: '#0/panel', r: [0, 180, 240, 60], o: 'tabpanel', t: 'Day' },
        { k: 'button', p: '#0/panel/vol', r: [0, 184, 140, 24], t: 'Mute', a: 4 },
    ]));
    assert.equal(h.a11yNodes.get('button:#0/menu/off'), off);
    assert.equal(off.getAttribute('aria-disabled'), null);
    assert.equal(tree.getAttribute('aria-busy'), null);
    assert.equal(src.getAttribute('aria-busy'), null);
    assert.equal(src.getAttribute('aria-level'), null);
    assert.equal(vol.getAttribute('role'), null);
    assert.equal(vol.getAttribute('aria-label'), null);
    assert.equal(vol.getAttribute('aria-valuenow'), null);
    assert.equal(vol.getAttribute('aria-valuetext'), null);
    assert.equal(vol.textContent, 'Mute');

    bridge.destroy(h);
});

test('focusA11y sets and clears aria-disabled, aria-busy, and numeric range attributes on the focus proxy', () => {
    const { h } = setup();

    bridge.focusA11y(h, JSON.stringify({ o: 'slider', t: 'Volume', rg: [0, 100, 65], v: '65%', dis: true, b: true }));
    const first = h.focusProxies[h.focusTurn];
    assert.equal(first.getAttribute('role'), 'slider');
    assert.equal(first.getAttribute('aria-label'), 'Volume');
    assert.equal(first.getAttribute('aria-valuemin'), '0');
    assert.equal(first.getAttribute('aria-valuemax'), '100');
    assert.equal(first.getAttribute('aria-valuenow'), '65');
    assert.equal(first.getAttribute('aria-valuetext'), '65%');
    assert.equal(first.getAttribute('aria-disabled'), 'true');
    assert.equal(first.getAttribute('aria-busy'), 'true');
    assert.equal(first.textContent, '');

    bridge.focusA11y(h, JSON.stringify({ o: 'spinbutton', t: 'Retries', rg: [1, 10, 4], vt: '4 retries' }));
    const second = h.focusProxies[h.focusTurn];
    assert.equal(second.getAttribute('role'), 'spinbutton');
    assert.equal(second.getAttribute('aria-valuemin'), '1');
    assert.equal(second.getAttribute('aria-valuemax'), '10');
    assert.equal(second.getAttribute('aria-valuenow'), '4');
    assert.equal(second.getAttribute('aria-valuetext'), '4 retries');
    assert.equal(second.getAttribute('aria-disabled'), null);
    assert.equal(second.getAttribute('aria-busy'), null);

    // Reusing the first proxy slot clears disabled, busy, and range attributes.
    bridge.focusA11y(h, JSON.stringify({ o: 'button', t: 'Apply' }));
    assert.equal(h.focusProxies[h.focusTurn], first);
    assert.equal(first.getAttribute('role'), 'button');
    assert.equal(first.getAttribute('aria-label'), 'Apply');
    assert.equal(first.getAttribute('aria-disabled'), null);
    assert.equal(first.getAttribute('aria-busy'), null);
    assert.equal(first.getAttribute('aria-valuemin'), null);
    assert.equal(first.getAttribute('aria-valuemax'), null);
    assert.equal(first.getAttribute('aria-valuenow'), null);
    assert.equal(first.getAttribute('aria-valuetext'), null);

    bridge.destroy(h);
});

test('syncA11y applies root dir/lang, per-item dir overrides, and logical text-align across patches', () => {
    const { h } = setup();

    bridge.syncA11y(h, JSON.stringify({
        p: 1,
        dir: 'rtl',
        lang: 'ar-EG',
        u: [
            { k: 'text', p: '#0/rtl', r: [0, 0, 200, 24], t: 'مرحبا', f: ['sans', 400, 20, 0, false, 'start'] },
            { k: 'text', p: '#0/end', r: [0, 28, 200, 24], t: 'نهاية', f: ['sans', 400, 20, 0, false, 'end'] },
            { k: 'text', p: '#0/island', r: [0, 56, 200, 24], t: 'README.md', dir: 'ltr', f: ['mono', 400, 20, 0, false, 'start'] },
            { k: 'group', g: 'div', p: '#0/bar', r: [0, 84, 200, 32], o: 'toolbar', t: 'Actions', dir: 'ltr' },
            { k: 'text', p: '#0/bar/inner', r: [0, 0, 80, 24], t: 'إرسال', dir: 'rtl' },
        ],
        o: {
            '': ['p:#0/rtl', 'p:#0/end', 'p:#0/island', 'div:#0/bar'],
            'div:#0/bar': ['p:#0/bar/inner'],
        },
    }));

    assert.equal(h.a11y.getAttribute('dir'), 'rtl');
    assert.equal(h.a11y.getAttribute('lang'), 'ar-EG');
    const rtl = h.a11yNodes.get('p:#0/rtl');
    const end = h.a11yNodes.get('p:#0/end');
    const island = h.a11yNodes.get('p:#0/island');
    const bar = h.a11yNodes.get('div:#0/bar');
    const inner = h.a11yNodes.get('p:#0/bar/inner');
    assert.equal(rtl.getAttribute('dir'), null);
    assert.match(rtl.style.cssText, /text-align:start;/);
    assert.match(end.style.cssText, /text-align:end;/);
    assert.equal(island.getAttribute('dir'), 'ltr');
    assert.equal(bar.getAttribute('dir'), 'ltr');
    assert.equal(inner.getAttribute('dir'), 'rtl');

    // Flip root to ltr, update lang, and clear island/bar dir when they match the new parent.
    bridge.syncA11y(h, JSON.stringify({
        p: 1,
        dir: 'ltr',
        lang: 'en-US',
        u: [
            { k: 'text', p: '#0/island', r: [0, 56, 200, 24], t: 'README.md', f: ['mono', 400, 20, 0, false, 'start'] },
            { k: 'group', g: 'div', p: '#0/bar', r: [0, 84, 200, 32], o: 'toolbar', t: 'Actions' },
        ],
    }));
    assert.equal(h.a11y.getAttribute('dir'), 'ltr');
    assert.equal(h.a11y.getAttribute('lang'), 'en-US');
    assert.equal(island.getAttribute('dir'), null);
    assert.equal(bar.getAttribute('dir'), null);
    assert.equal(inner.getAttribute('dir'), 'rtl');

    // Clearing lang with an empty string removes the attribute cleanly.
    bridge.syncA11y(h, JSON.stringify({ p: 1, lang: '' }));
    assert.equal(h.a11y.getAttribute('dir'), 'ltr');
    assert.equal(h.a11y.getAttribute('lang'), null);

    bridge.destroy(h);
});

test('syncA11y and focusA11y mirror, patch, and clear aria-autocomplete on mirror nodes, proxy, and textarea', () => {
    const { h } = setup();

    // Full snapshot sets aria-autocomplete on a mirrored combobox.
    bridge.syncA11y(h, JSON.stringify([
        { k: 'button', p: '#0/fruit', r: [0, 0, 200, 28], t: 'Fruit', o: 'combobox', x: false, hp: 'listbox', ac: 'list', v: 'Ap' },
    ]));
    const combo = h.a11yNodes.get('button:#0/fruit');
    assert.equal(combo.getAttribute('role'), 'combobox');
    assert.equal(combo.getAttribute('aria-autocomplete'), 'list');
    assert.equal(combo.getAttribute('aria-haspopup'), 'listbox');
    assert.equal(combo.textContent, 'Ap');

    // Incremental patch updates aria-autocomplete to both, then clears it when omitted.
    bridge.syncA11y(h, JSON.stringify({
        p: 1,
        u: [
            { k: 'button', p: '#0/fruit', r: [0, 0, 200, 28], t: 'Fruit', o: 'combobox', x: true, hp: 'listbox', ct: 'fruit-list', ac: 'both', v: 'Ap' },
        ],
    }));
    assert.equal(combo.getAttribute('aria-autocomplete'), 'both');
    assert.equal(combo.getAttribute('aria-expanded'), 'true');
    assert.equal(combo.getAttribute('aria-controls'), 'fruit-list');

    bridge.syncA11y(h, JSON.stringify({
        p: 1,
        u: [
            { k: 'button', p: '#0/fruit', r: [0, 0, 200, 28], t: 'Fruit', o: 'combobox', x: false, hp: 'listbox', v: 'Ap' },
        ],
    }));
    assert.equal(combo.getAttribute('aria-autocomplete'), null);

    // focusA11y sets aria-autocomplete on both focus proxy and hidden textarea for combobox/textbox.
    bridge.focusA11y(h, JSON.stringify({ o: 'combobox', t: 'Fruit', x: false, hp: 'listbox', ac: 'inline' }));
    let proxy = h.focusProxies[h.focusTurn];
    assert.equal(proxy.getAttribute('aria-autocomplete'), 'inline');
    assert.equal(h.textarea.getAttribute('aria-autocomplete'), 'inline');

    // Active descendant focus keeps the field's aria-autocomplete (f.fac) on textarea while option proxy omits it.
    bridge.focusA11y(h, JSON.stringify({ o: 'option', t: 'Apple', f: 'Search fruit', fac: 'list' }));
    proxy = h.focusProxies[h.focusTurn];
    assert.equal(proxy.getAttribute('role'), 'option');
    assert.equal(proxy.getAttribute('aria-autocomplete'), null);
    assert.equal(h.textarea.getAttribute('aria-label'), 'Search fruit');
    assert.equal(h.textarea.getAttribute('aria-autocomplete'), 'list');

    // Focusing an unrelated button clears textarea aria-autocomplete; clearing focus also removes it.
    bridge.focusA11y(h, JSON.stringify({ o: 'button', t: 'Save' }));
    assert.equal(h.textarea.getAttribute('aria-autocomplete'), null);
    bridge.focusA11y(h, JSON.stringify({ o: 'textbox', t: 'Filter', ac: 'both' }));
    assert.equal(h.textarea.getAttribute('aria-autocomplete'), 'both');
    bridge.focusA11y(h, '');
    assert.equal(h.textarea.getAttribute('aria-autocomplete'), null);

    bridge.destroy(h);
});

test('focusA11y mirrors read-only caret-browsing selection onto proxy attributes and DOM Selection', () => {
    const { h } = setup();
    let baseAndExtent = null;
    let cleared = 0;
    const prevGetSelection = globalThis.getSelection;
    globalThis.getSelection = () => ({
        anchorNode: baseAndExtent ? baseAndExtent[0] : null,
        setBaseAndExtent(an, ao, fn, fo) { baseAndExtent = [an, ao, fn, fo]; },
        removeAllRanges() { cleared++; baseAndExtent = null; },
    });
    try {
        bridge.syncA11y(h, JSON.stringify([
            { k: 'text', p: '#0/doc/p1', r: [0, 0, 200, 20], t: 'Hello world' },
            { k: 'text', p: '#0/doc/p2', r: [0, 24, 200, 20], t: 'Second line' },
        ]));
        const p1 = h.a11yByPath.get('#0/doc/p1');
        const p2 = h.a11yByPath.get('#0/doc/p2');
        p1.childNodes = [{ nodeType: 3, nodeValue: 'Hello world' }];
        p2.childNodes = [{ nodeType: 3, nodeValue: 'Second line' }];

        bridge.focusA11y(h, JSON.stringify({
            o: 'group', t: 'Document', ro: true,
            sel: { ap: '#0/doc/p1', a: 5, fp: '#0/doc/p1', f: 5 },
        }));
        let proxy = h.focusProxies[h.focusTurn];
        assert.equal(proxy.getAttribute('aria-readonly'), 'true');
        assert.deepEqual(JSON.parse(proxy.getAttribute('data-rune-selection')), {
            ap: '#0/doc/p1', a: 5, fp: '#0/doc/p1', f: 5,
        });
        assert.deepEqual(baseAndExtent, [p1.childNodes[0], 5, p1.childNodes[0], 5]);

        bridge.focusA11y(h, JSON.stringify({
            o: 'group', t: 'Document', ro: true,
            sel: { ap: '#0/doc/p1', a: 5, fp: '#0/doc/p2', f: 6 },
        }));
        proxy = h.focusProxies[h.focusTurn];
        assert.deepEqual(baseAndExtent, [p1.childNodes[0], 5, p2.childNodes[0], 6]);

        const origContains = h.a11y.contains;
        h.a11y.contains = node => node === p1.childNodes[0] || origContains.call(h.a11y, node);
        bridge.focusA11y(h, '');
        assert.equal(cleared, 1);
        assert.equal(baseAndExtent, null);
    } finally {
        globalThis.getSelection = prevGetSelection;
        bridge.destroy(h);
    }
});

test('armed external link forwards wheel scroll and Ctrl+wheel pinch to canvas and disarms when scrolled away', () => {
    const { win, canvas, events, h } = setup();
    const wheels = [];
    // Simulate winit's wheel listener registered before bridge.mount by putting it at the front.
    const canvasWheels = canvas.listeners.get('wheel') ?? [];
    canvasWheels.unshift(ev => wheels.push({ dx: ev.deltaX, dy: ev.deltaY, mode: ev.deltaMode, ctrl: ev.ctrlKey }));
    canvas.listeners.set('wheel', canvasWheels);
    try {
        bridge.syncA11y(h, JSON.stringify([
            {
                k: 'group', g: 'div', p: '#0/sc', r: [0, 0, 320, 140], sc: [0, 0],
            },
            {
                k: 'text', p: '#0/sc/t', a: 0, r: [12, 24, 160, 20], t: 'Open docs',
                u: [{ t: 'Open ' }, { t: 'docs', h: 'https://example.com/docs', p: '#0/sc/t', i: 0 }],
            },
        ]));
        const sc = h.a11yNodes.get('div:#0/sc');
        sc.style.overflow = 'hidden';
        sc.getBoundingClientRect = () => ({ left: 0, top: 0, right: 320, bottom: 140, width: 320, height: 140 });
        const a = h.a11y.querySelectorAll('a[target="_blank"]')[0];
        assert.ok(a, 'external link mirrored as a[target="_blank"]');
        let linkRect = { left: 60, top: 24, right: 100, bottom: 44, width: 40, height: 20 };
        a.getClientRects = () => [linkRect];

        // Hover arms the external link so browser status bar / clicks work.
        canvas.dispatchEvent({ type: 'pointermove', pointerType: 'mouse', buttons: 0, clientX: 80, clientY: 32 });
        assert.equal(h.hoverLink, a);
        assert.equal(a.style.pointerEvents, 'auto');

        // Normal wheel over the armed link forwards to canvas and prevents default.
        let prevented = false;
        h.a11y.dispatchEvent({
            type: 'wheel', target: a, deltaX: 0, deltaY: 240, deltaZ: 0, deltaMode: 0,
            clientX: 80, clientY: 32, shiftKey: false, ctrlKey: false, altKey: false, metaKey: false,
            preventDefault() { prevented = true; },
        });
        assert.equal(prevented, true);
        assert.deepEqual(wheels, [{ dx: 0, dy: 240, mode: 0, ctrl: false }]);

        // Ctrl+wheel (pinch) leaves browser zoom un-prevented when Rune did not consume it,
        // and prevents default when Rune sets syncWheel(h, true).
        wheels.length = 0;
        prevented = false;
        bridge.syncWheel(h, false);
        h.a11y.dispatchEvent({
            type: 'wheel', target: a, deltaX: 0, deltaY: -12, deltaZ: 0, deltaMode: 0,
            clientX: 80, clientY: 32, ctrlKey: true,
            preventDefault() { prevented = true; },
        });
        assert.equal(prevented, false, 'unhandled Ctrl+wheel stays with browser zoom');
        assert.deepEqual(wheels, [{ dx: 0, dy: -12, mode: 0, ctrl: true }]);

        wheels.length = 0;
        prevented = false;
        bridge.syncWheel(h, true);
        h.a11y.dispatchEvent({
            type: 'wheel', target: a, deltaX: 0, deltaY: -12, deltaZ: 0, deltaMode: 0,
            clientX: 80, clientY: 32, ctrlKey: true,
            preventDefault() { prevented = true; },
        });
        assert.equal(prevented, true, 'Rune-handled Ctrl+wheel prevents browser zoom');

        // DOM_DELTA_PAGE (deltaMode === 2) also forwards wheel_page through canvas.
        events.length = 0;
        prevented = false;
        h.a11y.dispatchEvent({
            type: 'wheel', target: a, deltaX: 0, deltaY: 1, deltaZ: 0, deltaMode: 2,
            clientX: 80, clientY: 32,
            preventDefault() { prevented = true; },
        });
        assert.equal(prevented, true);
        assert.ok(events.some(e => e.kind === 'wheel_page' && e.dy === -1));

        // After scroll moves the link out from under the stationary pointer, syncA11yPatch disarms it.
        linkRect = { left: 60, top: -200, right: 100, bottom: -180, width: 40, height: 20 };
        bridge.syncA11y(h, JSON.stringify({ p: 1, u: [{ k: 'group', g: 'div', p: '#0/sc', r: [0, 0, 320, 140], sc: [0, 224] }] }));
        assert.equal(h.hoverLink, null);
        assert.equal(a.style.pointerEvents, 'none');

        // Re-arm and verify window blur clears hoverLink cleanly.
        linkRect = { left: 60, top: 24, right: 100, bottom: 44, width: 40, height: 20 };
        canvas.dispatchEvent({ type: 'pointermove', pointerType: 'mouse', buttons: 0, clientX: 80, clientY: 32 });
        assert.equal(h.hoverLink, a);
        win.dispatchEvent({ type: 'blur' });
        assert.equal(h.hoverLink, null);
        assert.equal(a.style.pointerEvents, 'none');
    } finally {
        bridge.destroy(h);
    }
});

test('armed external link keeps clicks/middle/modified/right-click with browser, promotes primary drag and forwards touch to canvas', () => {
    const { win, canvas, h } = setup();
    canvas.getBoundingClientRect = () => ({ left: 10, top: 20, right: 330, bottom: 260, width: 320, height: 240 });
    const canvasPtr = [];
    for (const type of ['pointerdown', 'pointermove', 'pointerup', 'pointercancel']) {
        canvas.addEventListener(type, ev => {
            canvasPtr.push({
                type: ev.type, pointerType: ev.pointerType, button: ev.button,
                ox: ev.offsetX, oy: ev.offsetY, id: ev.pointerId,
                coalesced: typeof ev.getCoalescedEvents === 'function' ? ev.getCoalescedEvents().length : 0,
            });
        });
    }
    try {
        bridge.syncA11y(h, JSON.stringify([
            {
                k: 'text', p: '#0/t', r: [10, 20, 200, 20], t: 'Open docs',
                u: [{ t: 'Open ' }, { t: 'docs', h: 'https://example.com/docs', p: '#0/t', i: 0 }],
            },
            {
                k: 'link', p: '#0/dis', r: [10, 50, 100, 20], t: 'Disabled', h: 'https://example.com/dis', dis: true,
            },
        ]));
        // Disabled block El::href uses pointer-events:none.
        const dis = h.a11yNodes.get('a:#0/dis');
        assert.match(dis.style.cssText, /pointer-events:none;/);

        const a = h.a11y.querySelectorAll('a[target="_blank"]')[0];
        a.getClientRects = () => [{ left: 70, top: 40, right: 110, bottom: 60, width: 40, height: 20 }];

        // 1. Plain primary click (<= 4px travel) stays with the browser: canvas sees nothing and click is un-prevented.
        canvas.dispatchEvent({ type: 'pointermove', pointerType: 'mouse', buttons: 0, clientX: 80, clientY: 50 });
        canvasPtr.length = 0;
        assert.equal(h.hoverLink, a);
        h.a11y.dispatchEvent({ type: 'pointerdown', target: a, pointerType: 'mouse', pointerId: 1, button: 0, buttons: 1, clientX: 80, clientY: 50 });
        win.dispatchEvent({ type: 'pointermove', target: a, pointerType: 'mouse', pointerId: 1, button: -1, buttons: 1, clientX: 82, clientY: 51 });
        win.dispatchEvent({ type: 'pointerup', target: a, pointerType: 'mouse', pointerId: 1, button: 0, buttons: 0, clientX: 82, clientY: 51 });
        let clickPrevented = false;
        h.a11y.dispatchEvent({ type: 'click', target: a, preventDefault() { clickPrevented = true; } });
        assert.deepEqual(canvasPtr, [], 'plain click must not forward to canvas');
        assert.equal(clickPrevented, false, 'plain click must stay un-prevented for browser navigation');

        // 2. Middle click, Ctrl+click, Meta+click, and right-click stay with the browser.
        for (const ev of [
            { button: 1, buttons: 4 },
            { button: 0, buttons: 1, ctrlKey: true },
            { button: 0, buttons: 1, metaKey: true },
            { button: 2, buttons: 2 },
        ]) {
            h.a11y.dispatchEvent({ type: 'pointerdown', target: a, pointerType: 'mouse', pointerId: 2, clientX: 80, clientY: 50, ...ev });
            win.dispatchEvent({ type: 'pointermove', target: a, pointerType: 'mouse', pointerId: 2, clientX: 120, clientY: 50, ...ev });
            win.dispatchEvent({ type: 'pointerup', target: a, pointerType: 'mouse', pointerId: 2, clientX: 120, clientY: 50, ...ev });
        }
        assert.deepEqual(canvasPtr, [], 'middle, modified, and secondary presses stay with browser');

        // 3. Primary drag (> 4px travel) promotes to canvas (with canvas-relative offsetX/Y), blocks dragstart,
        //    disarms link, and suppresses the trailing click.
        let dragPrevented = false;
        h.a11y.dispatchEvent({ type: 'dragstart', target: a, preventDefault() { dragPrevented = true; } });
        assert.equal(dragPrevented, true);
        h.a11y.dispatchEvent({ type: 'pointerdown', target: a, pointerType: 'mouse', pointerId: 3, button: 0, buttons: 1, clientX: 80, clientY: 50 });
        win.dispatchEvent({ type: 'pointermove', target: a, pointerType: 'mouse', pointerId: 3, button: -1, buttons: 1, clientX: 100, clientY: 50 });
        win.dispatchEvent({ type: 'pointerup', target: a, pointerType: 'mouse', pointerId: 3, button: 0, buttons: 0, clientX: 100, clientY: 50 });
        assert.equal(h.hoverLink, null, 'drag promotion disarms link');
        assert.deepEqual(canvasPtr, [
            { type: 'pointerdown', pointerType: 'mouse', button: 0, ox: 70, oy: 30, id: 3, coalesced: 0 },
            { type: 'pointermove', pointerType: 'mouse', button: -1, ox: 90, oy: 30, id: 3, coalesced: 1 },
            { type: 'pointerup', pointerType: 'mouse', button: 0, ox: 90, oy: 30, id: 3, coalesced: 0 },
        ]);
        clickPrevented = false;
        h.a11y.dispatchEvent({ type: 'click', target: a, preventDefault() { clickPrevented = true; } });
        assert.equal(clickPrevented, true, 'trailing click after promoted drag is suppressed');

        // 4. Touch starting on an armed link disarms it, prevents touchstart, and forwards to canvas.
        canvas.dispatchEvent({ type: 'pointermove', pointerType: 'mouse', buttons: 0, clientX: 80, clientY: 50 });
        assert.equal(h.hoverLink, a);
        canvasPtr.length = 0;
        let touchPrevented = false;
        h.a11y.dispatchEvent({ type: 'touchstart', target: a, preventDefault() { touchPrevented = true; } });
        assert.equal(touchPrevented, true);
        h.a11y.dispatchEvent({ type: 'pointerdown', target: a, pointerType: 'touch', pointerId: 9, button: 0, buttons: 1, clientX: 80, clientY: 50, preventDefault() {} });
        assert.equal(h.hoverLink, null, 'touch down disarms mouse hoverLink');
        h.a11y.dispatchEvent({ type: 'pointermove', target: a, pointerType: 'touch', pointerId: 9, button: -1, buttons: 1, clientX: 80, clientY: 110 });
        h.a11y.dispatchEvent({ type: 'pointerup', target: a, pointerType: 'touch', pointerId: 9, button: 0, buttons: 0, clientX: 80, clientY: 110 });
        assert.deepEqual(canvasPtr, [
            { type: 'pointerdown', pointerType: 'touch', button: 0, ox: 70, oy: 30, id: 9, coalesced: 0 },
            { type: 'pointermove', pointerType: 'touch', button: -1, ox: 70, oy: 90, id: 9, coalesced: 1 },
            { type: 'pointerup', pointerType: 'touch', button: 0, ox: 70, oy: 90, id: 9, coalesced: 0 },
        ]);
    } finally {
        bridge.destroy(h);
    }
});

test('requestPointerLock requests plain lock synchronously, upgrades unadjustedMovement in place, and handles exit and refusal', async () => {
    const { events, doc, canvas, h } = setup();
    const flushMicrotasks = () => new Promise(r => queueMicrotask(r));
    const calls = [];
    let exitCalls = 0;
    doc.pointerLockElement = null;
    doc.exitPointerLock = () => {
        exitCalls++;
        if (doc.pointerLockElement) {
            doc.pointerLockElement = null;
            doc.dispatchEvent({ type: 'pointerlockchange' });
        }
    };
    try {
        // 1. Plain requestPointerLock() runs synchronously on the first click; once locked,
        // in-place { unadjustedMovement: true } upgrade rejecting with NotSupportedError
        // (even alongside pointerlockerror) keeps the plain lock intact and caches fallback.
        canvas.requestPointerLock = opts => {
            calls.push(opts);
            if (opts && opts.unadjustedMovement) {
                doc.dispatchEvent({ type: 'pointerlockerror' });
                const err = new Error('not supported');
                err.name = 'NotSupportedError';
                return Promise.reject(err);
            }
            return Promise.resolve().then(() => {
                doc.pointerLockElement = canvas;
                // Even a stray pointerlockerror arriving between promise resolution and
                // pointerlockchange must not abort the pending lock.
                doc.dispatchEvent({ type: 'pointerlockerror' });
                doc.dispatchEvent({ type: 'pointerlockchange' });
            });
        };
        bridge.requestPointerLock(h);
        assert.deepEqual(calls, [undefined], 'first call is synchronous plain requestPointerLock()');
        await flushMicrotasks();
        await flushMicrotasks();
        await flushMicrotasks();
        assert.deepEqual(calls, [undefined, { unadjustedMovement: true }]);
        assert.equal(doc.pointerLockElement, canvas, 'plain lock stays intact after NotSupportedError upgrade');
        assert.equal(exitCalls, 0);
        assert.deepEqual(events, [{ kind: 'pointer_lock', locked: true }]);

        // 2. While locked, mouse pointermove sends movementX/movementY; touch and zero deltas are ignored.
        events.length = 0;
        doc.dispatchEvent({ type: 'pointermove', pointerType: 'mouse', movementX: 12, movementY: -4 });
        doc.dispatchEvent({ type: 'pointermove', pointerType: 'mouse', movementX: 0, movementY: 0 });
        doc.dispatchEvent({ type: 'pointermove', pointerType: 'touch', movementX: 9, movementY: 9 });
        assert.deepEqual(events, [{ kind: 'pointer_motion', dx: 12, dy: -4 }]);

        // 3. Browser exits lock (Escape / tab switch): pointerlockchange sends locked: false,
        // and subsequent pointermove sends nothing.
        events.length = 0;
        doc.pointerLockElement = null;
        doc.dispatchEvent({ type: 'pointerlockchange' });
        doc.dispatchEvent({ type: 'pointermove', pointerType: 'mouse', movementX: 25, movementY: 5 });
        assert.deepEqual(events, [{ kind: 'pointer_lock', locked: false }]);

        // 4. Subsequent grant skips unadjustedMovement upgrade (cached), and SecurityError refusal
        // (e.g. Chrome cooldown or unfocused document) reports locked: false without retrying.
        calls.length = 0;
        events.length = 0;
        canvas.requestPointerLock = opts => {
            calls.push(opts);
            const err = new Error('cooldown');
            err.name = 'SecurityError';
            return Promise.reject(err);
        };
        bridge.requestPointerLock(h);
        await flushMicrotasks();
        await flushMicrotasks();
        assert.deepEqual(calls, [undefined]);
        assert.deepEqual(events, [{ kind: 'pointer_lock', locked: false, refused: true }]);

        // 5. On a browser that DOES support unadjustedMovement, the in-place upgrade succeeds
        // and its second pointerlockchange does not emit a duplicate locked: true event.
        h.pointerLockUnadjustedFailed = false;
        calls.length = 0;
        events.length = 0;
        canvas.requestPointerLock = opts => {
            calls.push(opts);
            return Promise.resolve().then(() => {
                doc.pointerLockElement = canvas;
                doc.dispatchEvent({ type: 'pointerlockchange' });
            });
        };
        bridge.requestPointerLock(h);
        await flushMicrotasks();
        await flushMicrotasks();
        await flushMicrotasks();
        assert.deepEqual(calls, [undefined, { unadjustedMovement: true }]);
        assert.deepEqual(events, [{ kind: 'pointer_lock', locked: true }]);
        doc.pointerLockElement = null;
        doc.dispatchEvent({ type: 'pointerlockchange' });

        // 6. Legacy non-Promise browser: pointerlockerror reports refusal.
        events.length = 0;
        canvas.requestPointerLock = () => undefined;
        bridge.requestPointerLock(h);
        doc.dispatchEvent({ type: 'pointerlockerror' });
        assert.deepEqual(events, [{ kind: 'pointer_lock', locked: false, refused: true }]);

        // 7. exitPointerLock cancels a pending request so a stale late grant exits immediately,
        // and destroy() exits an active lock.
        events.length = 0;
        canvas.requestPointerLock = () => new Promise(() => {});
        bridge.requestPointerLock(h);
        bridge.exitPointerLock(h);
        doc.pointerLockElement = canvas;
        doc.dispatchEvent({ type: 'pointerlockchange' });
        assert.equal(exitCalls, 1, 'stale grant after exitPointerLock must exit immediately');
        assert.deepEqual(events, []);

        // Active lock is released on destroy().
        doc.pointerLockElement = canvas;
        h.pointerLocked = true;
    } finally {
        bridge.destroy(h);
    }
    assert.equal(doc.pointerLockElement, null);
});

test('coarse-pointer focus sites set inputMode=text and inert=false before focus() on plain and password inputs, and reset when unfocused', async () => {
    const captureFocus = (h, log) => {
        const opacityOf = el => el.style.cssText.match(/opacity:([^;]+)/)?.[1] ?? '';
        for (const [name, el] of [['textarea', h.textarea], ['password', h.password]]) {
            const orig = el.focus.bind(el);
            el.focus = opts => {
                log.push({
                    field: name,
                    inputMode: el.inputMode,
                    inert: el.inert,
                    opacity: opacityOf(el),
                });
                return orig(opts);
            };
        }
    };

    // Site 1: mount startup microtask with an initially focused plain field, then password field.
    for (const [secure, expectedField] of [[false, 'textarea'], [true, 'password']]) {
        const { doc, h } = setup({ coarse: true });
        const log = [];
        try {
            captureFocus(h, log);
            assert.equal(h.textarea.inputMode, 'none');
            assert.equal(h.password.inputMode, 'none');
            assert.equal(h.textarea.inert, true);
            assert.equal(h.password.inert, true);
            // Simulate an autofocused field synced before mount's microtask runs (activeElement is still body).
            bridge.sync(h, 'auto', secure ? '' : 'hi', 0, secure ? 0 : 2, secure ? 0 : 2, secure);
            assert.equal(log.length, 0, 'body activeElement waits for mount microtask');
            await Promise.resolve();
            assert.deepEqual(log, [{ field: expectedField, inputMode: 'text', inert: false, opacity: '0' }]);
            assert.equal(doc.activeElement, secure ? h.password : h.textarea);
        } finally {
            bridge.destroy(h);
        }
    }

    // Site 2: sync() when activeElement === canvas (tap path on iOS Safari), for plain & password fields,
    // plus Site 3: activate(), and reset when no editable field is focused (!key).
    const { doc, canvas, h } = setup({ coarse: true });
    const log = [];
    try {
        captureFocus(h, log);
        await Promise.resolve();
        assert.equal(doc.activeElement, canvas);

        // Plain field via sync() while canvas is active.
        bridge.sync(h, 'plain', 'hello', 0, 5, 5, false);
        assert.deepEqual(log, [{ field: 'textarea', inputMode: 'text', inert: false, opacity: '0' }]);
        assert.equal(doc.activeElement, h.textarea);
        assert.equal(h.password.inert, true);

        // Blur / non-editable focus resets both hidden inputs back to inputMode='none' and inert=true.
        log.length = 0;
        bridge.sync(h, '', '', 0, 0, 0, false);
        assert.equal(doc.activeElement, canvas);
        assert.equal(h.textarea.inputMode, 'none');
        assert.equal(h.password.inputMode, 'none');
        assert.equal(h.textarea.inert, true);
        assert.equal(h.password.inert, true);

        // Password field via sync() while canvas is active.
        bridge.sync(h, 'pw', '', 0, 0, 0, true);
        assert.deepEqual(log, [{ field: 'password', inputMode: 'text', inert: false, opacity: '0' }]);
        assert.equal(doc.activeElement, h.password);
        assert.equal(h.password.value, '', 'password input value stays empty');
        assert.equal(h.textarea.inert, true);

        // Switching directly from password field to plain field via sync() -> activate().
        log.length = 0;
        bridge.sync(h, 'plain', 'abc', 0, 3, 3, false);
        assert.deepEqual(log, [{ field: 'textarea', inputMode: 'text', inert: false, opacity: '0' }]);
        assert.equal(doc.activeElement, h.textarea);

        // Site 3: direct activate() from an unfocused body element.
        log.length = 0;
        bridge.sync(h, '', '', 0, 0, 0, false);
        doc.activeElement = doc.body;
        bridge.sync(h, 'pw', '', 0, 0, 0, true);
        assert.equal(log.length, 0);
        bridge.activate(h);
        assert.deepEqual(log, [{ field: 'password', inputMode: 'text', inert: false, opacity: '0' }]);
        assert.equal(doc.activeElement, h.password);
    } finally {
        bridge.destroy(h);
    }
});

test('syncA11y mirrors unfocused textboxes, forwards click/focus to a11y_press, and focusA11y syncs textarea/password with zero duplicate proxies', () => {
    const { events, h } = setup();
    try {
        bridge.syncA11y(h, JSON.stringify([
            { k: 'textbox', p: '#0/name', r: [16, 16, 240, 32], o: 'textbox', t: 'Name', v: 'Ada', rq: true, ph: 'Your name' },
            { k: 'textbox', p: '#0/reason', r: [16, 56, 240, 72], o: 'textbox', t: 'Reason', v: 'Line 1\nLine 2', ml: true, ph: 'Explain why' },
            { k: 'textbox', p: '#0/pw', r: [16, 136, 240, 32], o: 'textbox', t: 'Password', rq: true, ph: 'Enter password' },
            { k: 'textbox', p: '#0/ro', r: [16, 176, 240, 32], o: 'textbox', t: 'Token', v: 'fixed', ro: true },
            { k: 'textbox', p: '#0/dis', r: [16, 216, 240, 32], o: 'textbox', t: 'Locked', v: 'nope', dis: true },
            { k: 'textbox', p: '#0/bad', r: [16, 256, 240, 32], o: 'textbox', t: 'Email', v: 'bad@', inv: true },
        ]));

        const name = h.a11yNodes.get('div:#0/name');
        assert.equal(name.getAttribute('role'), 'textbox');
        assert.equal(name.getAttribute('aria-label'), 'Name');
        assert.equal(name.getAttribute('aria-required'), 'true');
        assert.equal(name.getAttribute('aria-placeholder'), 'Your name');
        assert.equal(name.getAttribute('aria-multiline'), null);
        assert.equal(name.textContent, 'Ada');

        const reason = h.a11yNodes.get('div:#0/reason');
        assert.equal(reason.getAttribute('aria-multiline'), 'true');
        assert.equal(reason.textContent, 'Line 1\nLine 2');

        const pw = h.a11yNodes.get('div:#0/pw');
        assert.equal(pw.getAttribute('role'), 'textbox');
        assert.equal(pw.getAttribute('aria-label'), 'Password');
        assert.equal(pw.textContent, '', 'password mirror node carries no secret value');

        assert.equal(h.a11yNodes.get('div:#0/ro').getAttribute('aria-readonly'), 'true');
        assert.equal(h.a11yNodes.get('div:#0/dis').getAttribute('aria-disabled'), 'true');
        assert.equal(h.a11yNodes.get('div:#0/bad').getAttribute('aria-invalid'), 'true');

        // Click and focus on an enabled mirror textbox send a11y_press; disabled ignores both.
        name.dispatchEvent({ type: 'click', preventDefault() {} });
        reason.dispatchEvent({ type: 'focus' });
        const dis = h.a11yNodes.get('div:#0/dis');
        dis.dispatchEvent({ type: 'click', preventDefault() {} });
        dis.dispatchEvent({ type: 'focus' });
        assert.deepEqual(events, [
            { kind: 'a11y_press', path: '#0/name' },
            { kind: 'a11y_press', path: '#0/reason' },
        ]);

        // Focusing multiline Reason syncs aria-label + aria-multiline onto h.textarea and clears both focus proxies.
        bridge.focusA11y(h, JSON.stringify({ o: 'textbox', t: 'Reason', ml: true, ph: 'Explain why', inv: true }));
        assert.equal(h.textarea.getAttribute('aria-label'), 'Reason');
        assert.equal(h.textarea.getAttribute('aria-multiline'), 'true');
        assert.equal(h.textarea.getAttribute('aria-placeholder'), 'Explain why');
        assert.equal(h.textarea.getAttribute('aria-invalid'), 'true');
        assert.equal(h.canvas.getAttribute('aria-activedescendant'), null);
        for (const p of h.focusProxies) {
            assert.equal(p.getAttribute('role'), null, 'canvas focus proxies hold no duplicate textbox role');
            assert.equal(p.getAttribute('aria-label'), null);
        }

        // Focusing password field syncs aria-label + states onto h.password and resets h.textarea.
        bridge.focusA11y(h, JSON.stringify({ o: 'textbox', t: 'Password', sec: true, rq: true, ph: 'Enter password' }));
        assert.equal(h.password.getAttribute('aria-label'), 'Password');
        assert.equal(h.password.getAttribute('aria-required'), 'true');
        assert.equal(h.password.getAttribute('aria-placeholder'), 'Enter password');
        assert.equal(h.password.value, '');
        assert.equal(h.textarea.getAttribute('aria-label'), 'Text input');
        assert.equal(h.textarea.getAttribute('aria-multiline'), null);
        for (const p of h.focusProxies) {
            assert.equal(p.getAttribute('role'), null);
        }

        // Clearing focus resets both hidden fields cleanly.
        bridge.focusA11y(h, '');
        assert.equal(h.password.getAttribute('aria-label'), 'Password input');
        assert.equal(h.password.getAttribute('aria-required'), null);
    } finally {
        bridge.destroy(h);
    }
});

test('syncVisualViewport shifts covered focused fields above the keyboard without blurring or resizing, and clears on retract or blur', () => {
    const { doc, body, canvas, h } = setup({ coarsePointer: true });
    const cpu = doc.createElement('canvas');
    cpu.setAttribute('data-rune-cpu', '');
    body.insertBefore(cpu, canvas);
    try {
        // Focus password field at mid-screen (y=260..284), already visible above h=362.
        bridge.sync(h, 'pw', '', 0, 0, 0, true);
        bridge.activate(h);
        bridge.syncCaret(h, 24, 260, 2, 24);
        assert.equal(doc.activeElement, h.password);
        assert.equal(h.password.inert, false);
        assert.equal(h.password.inputMode, 'text');

        bridge.syncVisualViewport(h, 0, 389, 699);
        bridge.syncVisualViewport(h, 0, 362, 699);
        assert.equal(doc.activeElement, h.password, 'visualViewport shrink must not blur focused password input');
        assert.equal(h.password.inert, false);
        assert.equal(h.password.inputMode, 'text');
        assert.equal(canvas.style.transform || '', '', 'already-visible field needs no canvas shift');
        assert.equal(cpu.style.transform || '', '');
        assert.equal(h.a11y.style.transform || '', '');

        // Move focus to a lower plain field (y=520..544) covered by the keyboard (visBottom=362).
        bridge.sync(h, 'bottom', 'hi', 0, 2, 2, false);
        bridge.syncCaret(h, 24, 520, 2, 24);
        assert.equal(doc.activeElement, h.textarea);
        // shift = min(544 + 16 - 362, 520 - 16, 699 - 362) = 198
        assert.equal(canvas.style.transform, 'translate3d(0, -198px, 0)');
        assert.equal(cpu.style.transform, 'translate3d(0, -198px, 0)');
        assert.equal(h.a11y.style.transform, 'translate3d(0, -198px, 0)');
        assert.equal(h.textarea.style.top, '322px', 'hidden input tracks visual shifted caret position');

        // Keyboard retracts back to full layout height: transform clears cleanly while keeping focus.
        bridge.syncVisualViewport(h, 0, 699, 699);
        assert.equal(canvas.style.transform, '');
        assert.equal(cpu.style.transform, '');
        assert.equal(h.a11y.style.transform, '');
        assert.equal(h.textarea.style.top, '520px');
        assert.equal(doc.activeElement, h.textarea);

        // Re-open keyboard and blur: clearing focus resets transform immediately.
        bridge.syncVisualViewport(h, 0, 362, 699);
        assert.equal(canvas.style.transform, 'translate3d(0, -198px, 0)');
        bridge.sync(h, '', '', 0, 0, 0, false);
        assert.equal(canvas.style.transform, '');
        assert.equal(cpu.style.transform, '');
        assert.equal(h.a11y.style.transform, '');
        assert.equal(h.textarea.inert, true);
        assert.equal(h.password.inert, true);
    } finally {
        bridge.destroy(h);
        cpu.remove();
    }

    // Embedded and document (page) modes must never overwrite canvas transform.
    for (const opts of [{ embedded: true }, { page: true }]) {
        const ctx = setup(opts);
        try {
            bridge.sync(ctx.h, 'f', 'abc', 0, 3, 3, false);
            bridge.syncCaret(ctx.h, 24, 520, 2, 24);
            bridge.syncVisualViewport(ctx.h, 0, 362, 699);
            assert.equal(ctx.canvas.style.transform || '', '');
        } finally {
            bridge.destroy(ctx.h);
        }
    }
});

test('external hidden-input blur reports field_blur, ignores window/menu/sync blurs, and activate refocuses already-focused input', async () => {
    const { doc, body, canvas, win, events, h } = setup({ coarse: true });
    let focusedDoc = true;
    doc.hasFocus = () => focusedDoc;
    const focusCalls = [];
    for (const [name, el] of [['textarea', h.textarea], ['password', h.password]]) {
        const orig = el.focus.bind(el);
        el.focus = opts => {
            focusCalls.push({ name, wasActive: doc.activeElement === el, inputMode: el.inputMode, inert: el.inert });
            return orig(opts);
        };
    }
    try {
        await Promise.resolve();
        // 1. Focus password field, then blur externally to body (iOS keyboard dismissal with no pointer event).
        bridge.sync(h, 'pw', '', 0, 0, 0, true);
        bridge.activate(h);
        assert.equal(doc.activeElement, h.password);
        events.length = 0;

        doc.activeElement = body;
        h.password.dispatchEvent({ type: 'blur' });
        await Promise.resolve();
        assert.deepEqual(events, [{ kind: 'field_blur', key: 'pw' }]);

        // Rust answers with sync(h, '', ...) -> clears key and sets inert without stealing focus to canvas.
        bridge.sync(h, '', '', 0, 0, 0, false);
        assert.equal(doc.activeElement, body);
        assert.equal(h.password.inert, true);
        assert.equal(h.password.inputMode, 'none');

        // 2. External blur to canvas (Lyzev trace: focusout next=BODY, focusin <CANVAS>) emits field_blur,
        // and a non-gesture sync() before the microtask must not silently reclaim focus from canvas.
        bridge.sync(h, 'pw', '', 0, 0, 0, true);
        bridge.activate(h);
        events.length = 0;
        focusCalls.length = 0;

        canvas.focus();
        bridge.sync(h, 'pw', '', 0, 0, 0, true);
        assert.equal(doc.activeElement, canvas, 'sync must not reclaim focus while an external blur is pending');
        assert.equal(focusCalls.length, 0);
        await Promise.resolve();
        assert.deepEqual(events, [{ kind: 'field_blur', key: 'pw' }]);
        bridge.sync(h, '', '', 0, 0, 0, false);

        // 3. Re-tapping an already-focused field still invokes focus() inside activate() so WebKit runs elementDidRefocus.
        bridge.sync(h, 'pw', '', 0, 0, 0, true);
        bridge.activate(h);
        assert.equal(doc.activeElement, h.password);
        focusCalls.length = 0;
        bridge.activate(h);
        assert.deepEqual(focusCalls, [{ name: 'password', wasActive: true, inputMode: 'text', inert: false }]);

        // 4. Window blur (!document.hasFocus()) does NOT emit field_blur (handled by window_focus).
        events.length = 0;
        focusedDoc = false;
        doc.activeElement = body;
        h.password.dispatchEvent({ type: 'blur' });
        win.dispatchEvent({ type: 'blur' });
        await Promise.resolve();
        assert.deepEqual(events, []);
        focusedDoc = true;

        // 5. Internal sync() clearing focus or switching fields does NOT emit field_blur.
        bridge.activate(h);
        events.length = 0;
        bridge.sync(h, 'plain', 'hi', 0, 2, 2, false);
        bridge.sync(h, '', '', 0, 0, 0, false);
        await Promise.resolve();
        assert.deepEqual(events, []);

        // 6. Blur during an active touch defers until pointerup: activate() suppresses it, otherwise it emits field_blur.
        bridge.sync(h, 'plain', 'hi', 0, 2, 2, false);
        bridge.activate(h);
        events.length = 0;
        win.dispatchEvent({ type: 'pointerdown', target: canvas, pointerType: 'touch', pointerId: 7, timeStamp: 10 });
        canvas.focus();
        await Promise.resolve();
        assert.deepEqual(events, [], 'blur held while touch is down');
        win.dispatchEvent({ type: 'pointerup', target: canvas, pointerType: 'touch', pointerId: 7, timeStamp: 20 });
        bridge.activate(h);
        await Promise.resolve();
        assert.deepEqual(events, [], 'activate() inside pointerup cancels deferred field_blur');

        // Touch cancelled without activate() flushes the deferred field_blur.
        win.dispatchEvent({ type: 'pointerdown', target: canvas, pointerType: 'touch', pointerId: 8, timeStamp: 30 });
        canvas.focus();
        win.dispatchEvent({ type: 'pointercancel', target: canvas, pointerType: 'touch', pointerId: 8, timeStamp: 40 });
        await Promise.resolve();
        assert.deepEqual(events, [{ kind: 'field_blur', key: 'plain' }]);

        // 7. Holding keyboard shift across pointerdown -> pointerup for mouse, pen, and touch.
        for (const [pointerType, pid] of [['mouse', 11], ['pen', 12], ['touch', 13]]) {
            bridge.sync(h, 'bottom', 'hi', 0, 2, 2, false);
            bridge.activate(h);
            bridge.syncCaret(h, 24, 520, 2, 24);
            bridge.syncVisualViewport(h, 0, 362, 699);
            assert.equal(canvas.style.transform, 'translate3d(0, -198px, 0)');

            // pointerdown blurs the field in Rune -> sync(h, '', ...) runs while pointer is held.
            win.dispatchEvent({ type: 'pointerdown', target: canvas, pointerType, pointerId: pid, timeStamp: 50 });
            bridge.sync(h, '', '', 0, 0, 0, false);
            assert.equal(canvas.style.transform, 'translate3d(0, -198px, 0)', `${pointerType}: transform stays held until pointerup`);

            // Window bubble pointerup clears transform after target-phase canvas pointerup has finished.
            win.dispatchEvent({ type: 'pointerup', target: canvas, pointerType, pointerId: pid, timeStamp: 60 });
            assert.equal(canvas.style.transform, '', `${pointerType}: transform clears after pointerup`);
        }
    } finally {
        bridge.destroy(h);
    }
});

test('next-frame focus reconciliation clears stale caret across transient window blur, delayed activeElement, visualViewport retract, and visibility return with zero idle rAFs', async () => {
    const origRaf = Object.getOwnPropertyDescriptor(globalThis, 'requestAnimationFrame');
    const origCancel = Object.getOwnPropertyDescriptor(globalThis, 'cancelAnimationFrame');
    const rafQueue = [];
    let nextRafId = 1;
    let rafScheduled = 0;
    Object.defineProperty(globalThis, 'requestAnimationFrame', {
        configurable: true,
        value: cb => {
            rafScheduled += 1;
            const id = nextRafId++;
            rafQueue.push({ id, cb });
            return id;
        },
    });
    Object.defineProperty(globalThis, 'cancelAnimationFrame', {
        configurable: true,
        value: id => {
            const idx = rafQueue.findIndex(item => item.id === id);
            if (idx >= 0) rafQueue.splice(idx, 1);
        },
    });
    const flushRaf = () => {
        const batch = rafQueue.splice(0, rafQueue.length);
        for (const item of batch) item.cb(0);
    };

    const { doc, body, win, events, h } = setup({ coarse: true });
    let focusedDoc = true;
    doc.hasFocus = () => focusedDoc;
    try {
        await Promise.resolve();

        // 1. Transient window blur + field blur -> window focus with activeElement === body.
        bridge.sync(h, 'pw', '', 0, 0, 0, true);
        bridge.activate(h);
        events.length = 0;
        focusedDoc = false;
        doc.activeElement = body;
        h.password.dispatchEvent({ type: 'blur' });
        win.dispatchEvent({ type: 'blur' });
        await Promise.resolve();
        assert.deepEqual(events, [], 'window blur alone does not emit field_blur');
        focusedDoc = true;
        win.dispatchEvent({ type: 'focus' });
        flushRaf();
        assert.deepEqual(events, [{ kind: 'field_blur', key: 'pw' }]);
        bridge.sync(h, '', '', 0, 0, 0, false);

        // 2. blur fires while document.activeElement briefly still reports h.password, then settles to body before rAF.
        bridge.sync(h, 'pw', '', 0, 0, 0, true);
        bridge.activate(h);
        events.length = 0;
        h.password.dispatchEvent({ type: 'blur' });
        await Promise.resolve();
        assert.deepEqual(events, [], 'microtask defers when activeElement is still h.password');
        doc.activeElement = body;
        flushRaf();
        assert.deepEqual(events, [{ kind: 'field_blur', key: 'pw' }]);
        bridge.sync(h, '', '', 0, 0, 0, false);

        // 3. visualViewport retracts to 699 after a suppressed blur left activeElement === body.
        bridge.sync(h, 'pw', '', 0, 0, 0, true);
        bridge.activate(h);
        events.length = 0;
        bridge.syncVisualViewport(h, 0, 362, 699);
        focusedDoc = false;
        doc.activeElement = body;
        h.password.dispatchEvent({ type: 'blur' });
        win.dispatchEvent({ type: 'blur' });
        await Promise.resolve();
        focusedDoc = true;
        bridge.syncVisualViewport(h, 0, 699, 699);
        flushRaf();
        assert.deepEqual(events, [{ kind: 'field_blur', key: 'pw' }]);
        bridge.sync(h, '', '', 0, 0, 0, false);

        // 4. visibilitychange returning from hidden when DOM focus left h.password.
        bridge.sync(h, 'pw', '', 0, 0, 0, true);
        bridge.activate(h);
        events.length = 0;
        doc.hidden = true;
        doc.dispatchEvent({ type: 'visibilitychange' });
        doc.activeElement = body;
        h.password.dispatchEvent({ type: 'blur' });
        await Promise.resolve();
        assert.deepEqual(events, []);
        doc.hidden = false;
        doc.dispatchEvent({ type: 'visibilitychange' });
        flushRaf();
        assert.deepEqual(events, [{ kind: 'field_blur', key: 'pw' }]);
        bridge.sync(h, '', '', 0, 0, 0, false);

        // 5. No blur (DOM input stays active across keyboard open/retract) keeps focus and schedules 0 rAFs.
        bridge.sync(h, 'pw', '', 0, 0, 0, true);
        bridge.activate(h);
        events.length = 0;
        const rafBefore = rafScheduled;
        bridge.syncVisualViewport(h, 0, 362, 699);
        bridge.syncVisualViewport(h, 0, 699, 699);
        assert.equal(rafScheduled, rafBefore, 'focused input across visualViewport resize schedules zero rAFs');
        assert.deepEqual(events, []);
    } finally {
        bridge.destroy(h);
        if (origRaf) Object.defineProperty(globalThis, 'requestAnimationFrame', origRaf);
        else delete globalThis.requestAnimationFrame;
        if (origCancel) Object.defineProperty(globalThis, 'cancelAnimationFrame', origCancel);
        else delete globalThis.cancelAnimationFrame;
    }
});

test('rapid mouse clicks reusing pointerId=1 cancel stale pointer-end fallback timer so second press keeps keyboard shift', () => {
    const origSetTimeout = globalThis.setTimeout;
    const origClearTimeout = globalThis.clearTimeout;
    const timers = new Map();
    let nextTimer = 1;
    Object.defineProperty(globalThis, 'setTimeout', {
        configurable: true,
        value: (fn, _ms) => {
            const id = nextTimer++;
            timers.set(id, fn);
            return id;
        },
    });
    Object.defineProperty(globalThis, 'clearTimeout', {
        configurable: true,
        value: id => { timers.delete(id); },
    });
    const flushTimers = () => {
        const batch = [...timers.entries()];
        timers.clear();
        for (const [, fn] of batch) fn();
    };

    const { canvas, win, h } = setup({ coarse: true });
    try {
        // Press 1 (mouse pointerId=1): focus bottom field, shift up, click and release.
        bridge.sync(h, 'bottom', 'hi', 0, 2, 2, false);
        bridge.activate(h);
        bridge.syncCaret(h, 24, 520, 2, 24);
        bridge.syncVisualViewport(h, 0, 362, 699);
        assert.equal(canvas.style.transform, 'translate3d(0, -198px, 0)');

        win.dispatchEvent({ type: 'pointerdown', target: canvas, pointerType: 'mouse', pointerId: 1, timeStamp: 10 });
        win.dispatchEvent({ type: 'pointerup', target: canvas, pointerType: 'mouse', pointerId: 1, timeStamp: 20 });

        // Press 2 (same mouse pointerId=1) starts BEFORE Press 1's fallback setTimeout(0) has fired.
        // Press 2 blurs the bottom field on pointerdown -> sync('', ...) runs while pointer is held.
        win.dispatchEvent({ type: 'pointerdown', target: canvas, pointerType: 'mouse', pointerId: 1, timeStamp: 25 });
        bridge.sync(h, '', '', 0, 0, 0, false);
        assert.equal(canvas.style.transform, 'translate3d(0, -198px, 0)');

        // Stale timer from Press 1 fires mid-Press 2: must NOT delete Press 2's pid=1 or drop shift!
        flushTimers();
        assert.equal(canvas.style.transform, 'translate3d(0, -198px, 0)', 'stale timer from first press must not drop keyboard shift during second press');

        // Press 2 releases: transform now clears cleanly.
        win.dispatchEvent({ type: 'pointerup', target: canvas, pointerType: 'mouse', pointerId: 1, timeStamp: 35 });
        assert.equal(canvas.style.transform, '');
    } finally {
        bridge.destroy(h);
        Object.defineProperty(globalThis, 'setTimeout', { configurable: true, value: origSetTimeout });
        Object.defineProperty(globalThis, 'clearTimeout', { configurable: true, value: origClearTimeout });
    }
});

test('blur/focusout with activeElement lagging past first rAF still reconciles on trailing check, and a new focus cancels it', async () => {
    const origRaf = Object.getOwnPropertyDescriptor(globalThis, 'requestAnimationFrame');
    const origCancel = Object.getOwnPropertyDescriptor(globalThis, 'cancelAnimationFrame');
    const origSetTimeout = globalThis.setTimeout;
    const origClearTimeout = globalThis.clearTimeout;
    const rafQueue = [];
    const timers = new Map();
    let nextId = 1;
    Object.defineProperty(globalThis, 'requestAnimationFrame', {
        configurable: true,
        value: cb => { const id = nextId++; rafQueue.push({ id, cb }); return id; },
    });
    Object.defineProperty(globalThis, 'cancelAnimationFrame', {
        configurable: true,
        value: id => { const i = rafQueue.findIndex(x => x.id === id); if (i >= 0) rafQueue.splice(i, 1); },
    });
    Object.defineProperty(globalThis, 'setTimeout', {
        configurable: true,
        value: (fn, _ms) => { const id = nextId++; timers.set(id, fn); return id; },
    });
    Object.defineProperty(globalThis, 'clearTimeout', {
        configurable: true,
        value: id => { timers.delete(id); },
    });
    const flushRaf = () => { for (const x of rafQueue.splice(0)) x.cb(0); };
    const flushTimers = () => { const batch = [...timers.entries()]; timers.clear(); for (const [, fn] of batch) fn(); };

    const { doc, body, events, h } = setup({ coarse: true });
    doc.hasFocus = () => true;
    try {
        await Promise.resolve();
        // 1. blur/focusout fires while activeElement lags past the first rAF, then settles to body.
        bridge.sync(h, 'pw', '', 0, 0, 0, true);
        bridge.activate(h);
        events.length = 0;
        h.password.dispatchEvent({ type: 'blur' });
        h.password.dispatchEvent({ type: 'focusout' });
        await Promise.resolve();
        flushRaf(); // rAF1 fires while activeElement is still h.password (e.g. 16.6ms < 20ms lag)
        assert.deepEqual(events, []);
        doc.activeElement = body; // activeElement settles after rAF1
        flushTimers();
        assert.deepEqual(events, [{ kind: 'field_blur', key: 'pw' }]);
        bridge.sync(h, '', '', 0, 0, 0, false);

        // 2. If a new focus/activate arrives before the trailing check fires, the stale check is cancelled.
        bridge.sync(h, 'pw', '', 0, 0, 0, true);
        bridge.activate(h);
        events.length = 0;
        h.password.dispatchEvent({ type: 'blur' });
        await Promise.resolve();
        flushRaf();
        bridge.activate(h);
        flushTimers();
        assert.deepEqual(events, []);
    } finally {
        bridge.destroy(h);
        if (origRaf) Object.defineProperty(globalThis, 'requestAnimationFrame', origRaf);
        else delete globalThis.requestAnimationFrame;
        if (origCancel) Object.defineProperty(globalThis, 'cancelAnimationFrame', origCancel);
        else delete globalThis.cancelAnimationFrame;
        Object.defineProperty(globalThis, 'setTimeout', { configurable: true, value: origSetTimeout });
        Object.defineProperty(globalThis, 'clearTimeout', { configurable: true, value: origClearTimeout });
    }
});

test('delayed activeElement settle at 60ms, 500ms, 2s, and 5s+input reconciles with zero idle timers and preserves protected focus', async () => {
    const origRaf = Object.getOwnPropertyDescriptor(globalThis, 'requestAnimationFrame');
    const origCancel = Object.getOwnPropertyDescriptor(globalThis, 'cancelAnimationFrame');
    const origSetTimeout = globalThis.setTimeout;
    const origClearTimeout = globalThis.clearTimeout;
    let now = 0;
    let nextId = 1;
    const scheduled = new Map();
    const scheduleAt = (fn, delay) => {
        const id = nextId++;
        scheduled.set(id, { at: now + Math.max(0, delay || 0), fn });
        return id;
    };
    const cancelId = id => { scheduled.delete(id); };
    Object.defineProperty(globalThis, 'requestAnimationFrame', {
        configurable: true,
        value: cb => scheduleAt(() => cb(now), 16),
    });
    Object.defineProperty(globalThis, 'cancelAnimationFrame', {
        configurable: true,
        value: cancelId,
    });
    Object.defineProperty(globalThis, 'setTimeout', {
        configurable: true,
        value: (fn, ms) => scheduleAt(fn, ms),
    });
    Object.defineProperty(globalThis, 'clearTimeout', {
        configurable: true,
        value: cancelId,
    });
    const advanceTo = async targetMs => {
        while (true) {
            let earliest = null;
            for (const [id, entry] of scheduled) {
                if (entry.at <= targetMs && (!earliest || entry.at < earliest.at || (entry.at === earliest.at && id < earliest.id))) {
                    earliest = { id, at: entry.at, fn: entry.fn };
                }
            }
            if (!earliest) break;
            scheduled.delete(earliest.id);
            now = earliest.at;
            earliest.fn();
            await Promise.resolve();
        }
        now = Math.max(now, targetMs);
    };

    const { doc, win, body, canvas, events, h } = setup({ coarse: true });
    doc.hasFocus = () => true;
    try {
        await Promise.resolve();
        // Idle page with no focused field schedules zero timers or rAFs.
        assert.equal(scheduled.size, 0, 'idle unfocused page schedules zero timers');

        // 1. Settle at 60 ms, 500 ms, and 2,000 ms after blur/focusout.
        for (const settleMs of [60, 500, 2000]) {
            bridge.sync(h, 'pw', '', 0, 0, 0, true);
            bridge.activate(h);
            events.length = 0;
            const start = now;
            h.password.dispatchEvent({ type: 'blur' });
            h.password.dispatchEvent({ type: 'focusout' });
            await Promise.resolve();
            // Advance just before settleMs: activeElement is still h.password -> no premature blur.
            await advanceTo(start + settleMs - 1);
            assert.deepEqual(events, [], `no blur before activeElement settles at ${settleMs}ms`);
            // activeElement settles to body at settleMs.
            await advanceTo(start + settleMs);
            doc.activeElement = body;
            // Reconciles within bounded back-off without any extra user event.
            await advanceTo(start + settleMs + 1300);
            assert.deepEqual(events, [{ kind: 'field_blur', key: 'pw' }], `reconciles settle at ${settleMs}ms`);
            assert.equal(scheduled.size, 0, `no lingering timers after ${settleMs}ms reconcile`);
            bridge.sync(h, '', '', 0, 0, 0, false);
        }

        // 2. If iOS keeps h.password focused across the entire back-off, it never blurs and stops with 0 timers.
        bridge.sync(h, 'pw', '', 0, 0, 0, true);
        bridge.activate(h);
        events.length = 0;
        const keepStart = now;
        h.password.dispatchEvent({ type: 'blur' });
        await Promise.resolve();
        await advanceTo(keepStart + 10000);
        assert.deepEqual(events, [], 'keeping h.password as activeElement never emits field_blur');
        assert.equal(scheduled.size, 0, 'bounded back-off stops completely with 0 timers');

        // 3. Settle arbitrarily late (5,000 ms, past the bounded back-off): caught by focusin or next user input.
        doc.activeElement = canvas;
        doc.dispatchEvent({ type: 'focusin', target: canvas });
        await Promise.resolve();
        assert.deepEqual(events, [{ kind: 'field_blur', key: 'pw' }], 'document focusin catches arbitrarily late focus transfer');
        bridge.sync(h, '', '', 0, 0, 0, false);

        // And when late settle goes to body (no focusin), next user input (keydown / wheel / pointermove) catches it.
        for (const [label, fire] of [
            ['keydown', () => win.dispatchEvent({ type: 'keydown', target: body, key: 'Escape' })],
            ['wheel', () => win.dispatchEvent({ type: 'wheel', target: canvas, deltaY: 10 })],
            ['pointermove', () => win.dispatchEvent({ type: 'pointermove', target: canvas, pointerType: 'mouse', pointerId: 1 })],
        ]) {
            bridge.sync(h, 'pw', '', 0, 0, 0, true);
            bridge.activate(h);
            events.length = 0;
            const lateStart = now;
            h.password.dispatchEvent({ type: 'blur' });
            await Promise.resolve();
            await advanceTo(lateStart + 5000);
            assert.equal(scheduled.size, 0, `back-off stopped before 5s (${label})`);
            assert.deepEqual(events, []);
            doc.activeElement = body;
            fire();
            await Promise.resolve();
            assert.deepEqual(events, [{ kind: 'field_blur', key: 'pw' }], `5s late settle to body caught by ${label}`);
            assert.equal(scheduled.size, 0);
            bridge.sync(h, '', '', 0, 0, 0, false);
        }

        // 4. Protected visualViewport relayout while activeElement stays h.password schedules 0 timers and emits 0 blurs.
        bridge.sync(h, 'pw', '', 0, 0, 0, true);
        bridge.activate(h);
        events.length = 0;
        bridge.syncVisualViewport(h, 0, 362, 699);
        bridge.syncVisualViewport(h, 0, 699, 699);
        await Promise.resolve();
        assert.equal(scheduled.size, 0, 'protected visualViewport resize schedules 0 timers');
        assert.deepEqual(events, []);

        // 5. In-gesture refocus cancels an active back-off immediately with 0 blurs and 0 timers.
        const tapStart = now;
        h.password.dispatchEvent({ type: 'blur' });
        await Promise.resolve();
        await advanceTo(tapStart + 60);
        assert.ok(scheduled.size > 0, 'back-off is active before refocus');
        bridge.activate(h);
        assert.equal(scheduled.size, 0, 'activate() cancels active back-off immediately');
        await advanceTo(tapStart + 5000);
        assert.deepEqual(events, []);
    } finally {
        bridge.destroy(h);
        if (origRaf) Object.defineProperty(globalThis, 'requestAnimationFrame', origRaf);
        else delete globalThis.requestAnimationFrame;
        if (origCancel) Object.defineProperty(globalThis, 'cancelAnimationFrame', origCancel);
        else delete globalThis.cancelAnimationFrame;
        Object.defineProperty(globalThis, 'setTimeout', { configurable: true, value: origSetTimeout });
        Object.defineProperty(globalThis, 'clearTimeout', { configurable: true, value: origClearTimeout });
    }
});

test('syncA11y mirrors ordered lists as <ol> with start attribute and preserves native list markers on <ol>/<li> while keeping <ul> list-style:none', () => {
    const { h } = setup();
    try {
        bridge.syncA11y(h, JSON.stringify([
            { k: 'group', g: 'ol', p: '#0/ord1', r: [0, 0, 200, 40] },
            { k: 'group', g: 'li', p: '#0/ord1/i0', r: [0, 0, 200, 20], a: 0 },
            { k: 'text', p: '#0/ord1/i0/b0', r: [20, 0, 180, 20], t: 'First', a: 1 },
            { k: 'group', g: 'ol', p: '#0/ord3', r: [0, 50, 200, 40], st: 3 },
            { k: 'group', g: 'li', p: '#0/ord3/i0', r: [0, 50, 200, 20], a: 3 },
            { k: 'text', p: '#0/ord3/i0/b0', r: [20, 50, 180, 20], t: 'Third', a: 4 },
            { k: 'group', g: 'ul', p: '#0/bul', r: [0, 100, 200, 40] },
            { k: 'group', g: 'li', p: '#0/bul/i0', r: [0, 100, 200, 20], a: 6 },
            { k: 'text', p: '#0/bul/i0/b0', r: [20, 100, 180, 20], t: 'Bullet', a: 7 },
        ]));

        const [ol1, ol3, ul] = h.a11y.children;
        assert.equal(ol1.tagName, 'OL');
        assert.equal(ol1.getAttribute('start'), null, 'default start=1 omits start attribute');
        assert.equal(ol1.children[0].tagName, 'LI');
        assert.ok(!ol1.style.cssText.includes('list-style:none'), '<ol> keeps native decimal marker semantics');
        assert.ok(!ol1.children[0].style.cssText.includes('list-style:none'), '<li> inherits list-style from parent list');

        assert.equal(ol3.tagName, 'OL');
        assert.equal(ol3.getAttribute('start'), '3', 'ordered list starting at 3 sets start="3"');
        assert.equal(ol3.children[0].tagName, 'LI');

        assert.equal(ul.tagName, 'UL');
        assert.equal(ul.getAttribute('start'), null);
        assert.ok(ul.style.cssText.includes('list-style:none'), '<ul> keeps list-style:none');

        // Patch start from 3 -> 5 -> cleared (default 1).
        bridge.syncA11y(h, JSON.stringify({ p: 1, u: [{ k: 'group', g: 'ol', p: '#0/ord3', r: [0, 50, 200, 40], st: 5 }] }));
        assert.equal(ol3.getAttribute('start'), '5');
        bridge.syncA11y(h, JSON.stringify({ p: 1, u: [{ k: 'group', g: 'ol', p: '#0/ord3', r: [0, 50, 200, 40] }] }));
        assert.equal(ol3.getAttribute('start'), null);
    } finally {
        bridge.destroy(h);
    }
});

test('same-origin block El::href and relative inline links support native browser link interactions while routing plain clicks through Rune', () => {
    const { win, canvas, events, h } = setup();
    canvas.getBoundingClientRect = () => ({ left: 0, top: 0, right: 400, bottom: 300, width: 400, height: 300 });
    try {
        bridge.syncA11y(h, JSON.stringify([
            { k: 'link', p: '#0/chat', r: [10, 10, 120, 24], t: 'Chat 123', h: '/chat/123' },
            { k: 'link', p: '#0/ext', r: [10, 40, 120, 24], t: 'External', h: 'https://example.com/docs' },
            {
                k: 'text', p: '#0/msg', r: [10, 80, 300, 24],
                u: [
                    { t: 'See ' },
                    { t: 'thread', h: '/chat/456', p: '#0/msg', i: 0 },
                    { t: ' or ' },
                    { t: '@alice', p: '#0/msg', i: 1 },
                    { t: ' or ' },
                    { t: 'bad', h: 'javascript:alert(1)', p: '#0/msg', i: 2 },
                ],
            },
        ]));

        // 1. Block El::href("/chat/123"): pointer-events:auto, real href, no target="_blank".
        const chat = h.a11yNodes.get('a:#0/chat');
        assert.equal(chat.href, '/chat/123');
        assert.equal(chat.target, undefined);
        assert.match(chat.style.cssText, /pointer-events:auto;/);
        chat.getClientRects = () => [{ left: 10, top: 10, right: 130, bottom: 34, width: 120, height: 24 }];

        // Hovering over canvas coords of a block El::href must NOT arm/disarm it via hoverLink.
        canvas.dispatchEvent({ type: 'pointermove', pointerType: 'mouse', buttons: 0, clientX: 50, clientY: 20 });
        assert.equal(h.hoverLink, null, 'block El::href stays permanently armed without entering hoverLink');
        win.dispatchEvent({ type: 'blur' });
        assert.notEqual(chat.style.pointerEvents, 'none', 'disarmLink must never disable block El::href');

        // Plain unmodified primary click prevents browser navigation and routes through a11y_press.
        events.length = 0;
        let prevented = false;
        chat.dispatchEvent({ type: 'click', button: 0, preventDefault() { prevented = true; } });
        assert.equal(prevented, true);
        assert.deepEqual(events, [{ kind: 'a11y_press', path: '#0/chat' }]);

        // Middle-click and Cmd/Ctrl/Shift/Alt+click stay with the browser (not prevented, no a11y_press).
        for (const mod of [
            { button: 1 },
            { button: 0, metaKey: true },
            { button: 0, ctrlKey: true },
            { button: 0, shiftKey: true },
            { button: 0, altKey: true },
        ]) {
            events.length = 0;
            prevented = false;
            chat.dispatchEvent({ type: 'click', ...mod, preventDefault() { prevented = true; } });
            assert.equal(prevented, false, `modified/middle click stays native: ${JSON.stringify(mod)}`);
            assert.deepEqual(events, []);
        }

        // External block El::href keeps target="_blank" and lets plain click navigate natively.
        const ext = h.a11yNodes.get('a:#0/ext');
        assert.equal(ext.target, '_blank');
        events.length = 0;
        prevented = false;
        ext.dispatchEvent({ type: 'click', button: 0, preventDefault() { prevented = true; } });
        assert.equal(prevented, false);
        assert.deepEqual(events, []);

        // Promoted drag (> 4px) starting on same-origin block El::href suppresses trailing click (no a11y_press).
        events.length = 0;
        h.a11y.dispatchEvent({ type: 'pointerdown', target: chat, pointerType: 'mouse', pointerId: 4, button: 0, buttons: 1, clientX: 40, clientY: 20 });
        win.dispatchEvent({ type: 'pointermove', target: chat, pointerType: 'mouse', pointerId: 4, button: -1, buttons: 1, clientX: 80, clientY: 20 });
        win.dispatchEvent({ type: 'pointerup', target: chat, pointerType: 'mouse', pointerId: 4, button: 0, buttons: 0, clientX: 80, clientY: 20 });
        prevented = false;
        chat.dispatchEvent({ type: 'click', button: 0, preventDefault() { prevented = true; } });
        assert.equal(prevented, true, 'promoted drag cancels trailing click');
        assert.deepEqual(events.filter(e => e.kind === 'a11y_press'), [], 'promoted drag must not fire a11y_press');

        // 2. Inline relative link "/chat/456" vs opaque action link "@alice" vs unsafe "javascript:".
        const msg = h.a11yNodes.get('p:#0/msg');
        const anchors = msg.children.filter(c => c.tagName === 'A');
        assert.equal(anchors.length, 2, 'unsafe javascript: link collapses to plain text');
        const [relLink, opaqueLink] = anchors;
        assert.equal(relLink.href, '/chat/456');
        assert.equal(relLink.target, undefined);
        assert.equal(opaqueLink.href, '#');
        assert.equal(opaqueLink._opaqueLink, true);

        relLink.getClientRects = () => [{ left: 40, top: 80, right: 100, bottom: 100, width: 60, height: 20 }];
        opaqueLink.getClientRects = () => [{ left: 120, top: 80, right: 170, bottom: 100, width: 50, height: 20 }];

        // Hovering over the opaque action link does NOT arm it.
        canvas.dispatchEvent({ type: 'pointermove', pointerType: 'mouse', buttons: 0, clientX: 140, clientY: 90 });
        assert.equal(h.hoverLink, null, 'opaque non-URL inline link must not be hover-armed');

        // Hovering over the relative inline link arms pointer-events:auto for status bar / middle / right click.
        canvas.dispatchEvent({ type: 'pointermove', pointerType: 'mouse', buttons: 0, clientX: 60, clientY: 90 });
        assert.equal(h.hoverLink, relLink);
        assert.equal(relLink.style.pointerEvents, 'auto');

        // Unmodified primary click sends a11y_link; Cmd/Ctrl/middle click stays with browser.
        events.length = 0;
        prevented = false;
        relLink.dispatchEvent({ type: 'click', button: 0, preventDefault() { prevented = true; } });
        assert.equal(prevented, true);
        assert.deepEqual(events, [{ kind: 'a11y_link', path: '#0/msg', link: 0 }]);

        for (const mod of [{ button: 1 }, { button: 0, metaKey: true }, { button: 0, ctrlKey: true }]) {
            events.length = 0;
            prevented = false;
            relLink.dispatchEvent({ type: 'click', ...mod, preventDefault() { prevented = true; } });
            assert.equal(prevented, false);
            assert.deepEqual(events, []);
        }

        // Pointer leave disarms the inline relative link back to none.
        relLink.dispatchEvent({ type: 'pointerleave' });
        assert.equal(h.hoverLink, null);
        assert.equal(relLink.style.pointerEvents, 'none');
    } finally {
        bridge.destroy(h);
    }
});

test('macOS right-click armMenu flushes layout synchronously and does not suppress contextmenu if engine still targets canvas', () => {
    const { doc, win, canvas, events, h } = setup({ platform: 'MacIntel' });
    try {
        assert.equal(bridge.isMac(), true);
        bridge.syncA11y(h, JSON.stringify([
            { k: 'text', p: '#0/p', r: [10, 10, 200, 30], t: 'Hello macOS' },
        ]));
        const p = h.a11yNodes.get('p:#0/p');
        let flushCalls = 0;
        p.getBoundingClientRect = () => {
            if (p.style.pointerEvents === 'auto') flushCalls++;
            return { left: 10, top: 10, right: 210, bottom: 40, width: 200, height: 30 };
        };

        // 1. macOS right-click on text arms p, flushes layout synchronously on pointerdown, and sends secondary_browser.
        events.length = 0;
        bridge.syncSecondary(h, 'text');
        canvas.dispatchEvent({ type: 'pointerdown', target: canvas, pointerType: 'mouse', button: 2, clientX: 50, clientY: 20, shiftKey: false, preventDefault() {} });
        assert.equal(h.menu, 'browser');
        assert.equal(p.style.pointerEvents, 'auto');
        assert.ok(flushCalls >= 1, 'armMenu flushes layout synchronously after setting pointerEvents=auto');
        assert.deepEqual(events.filter(e => e.kind !== 'mods'), [{ kind: 'a11y_flush' }, { kind: 'secondary_browser' }]);

        // Safari/Firefox race: contextmenu still resolves event.target === canvas -> MUST NOT preventDefault when h.menu === 'browser'.
        let prevented = false;
        doc.dispatchEvent({ type: 'contextmenu', target: canvas, button: 2, preventDefault() { prevented = true; } });
        assert.equal(prevented, false, 'contextmenu must not be suppressed after armMenu armed browser menu');

        // Window blur disarms the browser menu and restores pointerEvents.
        win.dispatchEvent({ type: 'blur' });
        assert.equal(h.menu, '');
        assert.equal(p.style.pointerEvents, undefined);

        // 2. macOS Ctrl+click (button=0, ctrlKey=true) also triggers armAt on pointerdown.
        events.length = 0;
        bridge.syncSecondary(h, 'text');
        canvas.dispatchEvent({ type: 'pointerdown', target: canvas, pointerType: 'mouse', button: 0, ctrlKey: true, clientX: 50, clientY: 20, shiftKey: false, preventDefault() {} });
        assert.equal(h.menu, 'browser');
        prevented = false;
        doc.dispatchEvent({ type: 'contextmenu', target: canvas, button: 0, ctrlKey: true, preventDefault() { prevented = true; } });
        assert.equal(prevented, false);
        win.dispatchEvent({ type: 'blur' });

        // 3. Rune-owned right-click (secondary === 'app' without Shift) sets h.menu = 'rune' and DOES preventDefault.
        events.length = 0;
        bridge.syncSecondary(h, 'app');
        canvas.dispatchEvent({ type: 'pointerdown', target: canvas, pointerType: 'mouse', button: 2, clientX: 50, clientY: 20, shiftKey: false, preventDefault() {} });
        assert.equal(h.menu, 'rune');
        assert.deepEqual(events.filter(e => e.kind !== 'mods'), []);
        prevented = false;
        doc.dispatchEvent({ type: 'contextmenu', target: canvas, button: 2, preventDefault() { prevented = true; } });
        assert.equal(prevented, true, 'Rune-owned contextmenu stays suppressed for browser');
        assert.equal(h.menu, '');

        // 4. Shift+right-click overrides 'app' claim so browser menu opens even if target === canvas.
        events.length = 0;
        bridge.syncSecondary(h, 'app');
        canvas.dispatchEvent({ type: 'pointerdown', target: canvas, pointerType: 'mouse', button: 2, clientX: 50, clientY: 20, shiftKey: true, preventDefault() {} });
        assert.equal(h.menu, 'browser');
        prevented = false;
        doc.dispatchEvent({ type: 'contextmenu', target: canvas, button: 2, shiftKey: true, preventDefault() { prevented = true; } });
        assert.equal(prevented, false, 'Shift+right-click opens browser menu even when target === canvas');
        win.dispatchEvent({ type: 'blur' });

        // 5. Stray un-armed contextmenu on canvas (h.menu === '') is still suppressed.
        prevented = false;
        doc.dispatchEvent({ type: 'contextmenu', target: canvas, button: 2, preventDefault() { prevented = true; } });
        assert.equal(prevented, true, 'un-armed canvas contextmenu remains prevented');
    } finally {
        bridge.destroy(h);
    }
});
