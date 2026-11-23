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
    removeAttribute(name) { this._attrs.delete(name); }
    getAttribute(name) { return this._attrs.get(name) ?? null; }

    get firstElementChild() { return this.children[0] ?? null; }
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
        const i = this.parentElement.children.indexOf(this);
        if (i >= 0) this.parentElement.children.splice(i, 1);
        this.parentElement = null;
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

function installDom() {
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

    const win = new FakeNode('window');
    Object.defineProperty(globalThis, 'Node', { value: FakeNode, configurable: true });
    Object.defineProperty(globalThis, 'document', { value: doc, configurable: true });
    Object.defineProperty(globalThis, 'window', { value: win, configurable: true });
    Object.defineProperty(globalThis, 'navigator', {
        value: { platform: 'Linux x86_64', standalone: false },
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
    return { doc, win, body };
}

installDom();
const bridge = await import('./bridge.js');

function setup({ embedded = false, fitHeight = false, page = false, coarse = false } = {}) {
    coarsePointer = coarse;
    const { doc, body } = installDom();
    const canvas = doc.createElement('canvas');
    body.append(canvas);
    const events = [];
    const h = bridge.mount(canvas, msg => events.push(JSON.parse(msg)), embedded, fitHeight, page);
    return { doc, body, canvas, events, h };
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

test('sync keeps inputMode none on coarse pointers when reclaiming focus from canvas', async () => {
    const { doc, canvas, h } = setup({ coarse: true });
    await Promise.resolve();
    assert.equal(doc.activeElement, canvas);
    assert.equal(h.textarea.inputMode, 'none');

    bridge.sync(h, 'field', 'hello', 0, 5, 5, false);
    assert.equal(doc.activeElement, h.textarea);
    assert.equal(h.textarea.inputMode, 'none');
    assert.equal(h.textarea.selectionStart, 5);

    // An explicit tap activates the soft keyboard.
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
