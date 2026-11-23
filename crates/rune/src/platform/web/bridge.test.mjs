import assert from 'node:assert/strict';
import test from 'node:test';

class FakeNode {
    constructor(tagName) {
        this.tagName = tagName.toUpperCase();
        this.style = { cssText: '', setProperty() {} };
        this.attributes = new Map();
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
        this._value = String(next);
        this.selectionStart = this._value.length;
        this.selectionEnd = this._value.length;
        this.selectionDirection = 'forward';
    }

    setAttribute(name, value) { this.attributes.set(name, String(value)); }
    removeAttribute(name) { this.attributes.delete(name); }
    getAttribute(name) { return this.attributes.get(name) ?? null; }

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
