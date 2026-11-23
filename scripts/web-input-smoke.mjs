import assert from 'node:assert/strict';

export async function inputSmoke(page, url) {
    await page.goto(`${url}/?input-proof`);
    await page.waitForFunction(() => window.__inputProof);
    assert.equal(await page.title(), 'Input proof');
    assert.equal(await page.locator('pre').count(), 0, 'window requests must not replace the app');
    const snapshot = () => page.evaluate(() => window.__inputProof);
    // The fixture reports from poll, before that frame drains queued keys. Wait for the
    // host's next presented frame rather than guessing how long shader startup takes.
    const settle = () => page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    await page.touchscreen.tap(100, 36);
    assert.equal(await page.evaluate(() => document.activeElement.tagName), 'TEXTAREA');
    await page.keyboard.insertText('hello ');
    await settle();
    assert.equal((await snapshot()).plain, 'hello ');

    // A touch inside the same editor must never pass focus through the canvas/body.
    await page.evaluate(() => {
        window.__editorBlurs = 0;
        document.querySelector('textarea').addEventListener('blur', () => window.__editorBlurs++);
    });
    await page.touchscreen.tap(100, 36);
    assert.equal(await page.evaluate(() => window.__editorBlurs), 0);

    // WebKit on iOS 27 still dispatches a tap's compatibility mousedown to the canvas after a
    // prevented pointerdown; bridge.js must prevent it so the canvas does not steal focus back.
    const compatMousedownPrevented = await page.evaluate(() => {
        const event = new MouseEvent('mousedown', { bubbles: true, cancelable: true, clientX: 100, clientY: 36 });
        document.querySelector('canvas').dispatchEvent(event);
        return event.defaultPrevented;
    });
    assert.equal(compatMousedownPrevented, true, 'compatibility mousedown on canvas must be prevented');
    assert.equal(await page.evaluate(() => document.activeElement.tagName), 'TEXTAREA');
    assert.equal(await page.evaluate(() => window.__editorBlurs), 0);

    // Gboard reopens an existing word: include its unchanged prefix in the preedit range.
    await page.locator('textarea').evaluate(field => {
        field.dispatchEvent(new CompositionEvent('compositionstart'));
        field.value = 'hello'; field.setSelectionRange(5, 5);
        field.dispatchEvent(new CompositionEvent('compositionupdate', { data: 'hello' }));
        field.dispatchEvent(new InputEvent('input', { data: 'hello', inputType: 'insertCompositionText', isComposing: true }));
    });
    await settle();
    assert.equal((await snapshot()).display, 'hello');
    assert.equal((await snapshot()).plain, 'hello ');
    await page.locator('textarea').evaluate(field => field.dispatchEvent(new CompositionEvent('compositionend', { data: 'hello' })));
    await settle();
    assert.equal((await snapshot()).plain, 'hello');

    // Enter with keyCode 229 may have no usable keydown; beforeinput is authoritative.
    const lineBreak = async (inputType, shift = false) => page.locator('textarea').evaluate((field, { inputType, shift }) => {
        field.dispatchEvent(new KeyboardEvent('keydown', { key: 'Unidentified', keyCode: 229, shiftKey: shift, bubbles: true }));
        const event = new InputEvent('beforeinput', { inputType, cancelable: true, bubbles: true });
        field.dispatchEvent(event);
        if (!event.defaultPrevented) { field.setRangeText('\n', field.selectionStart, field.selectionEnd, 'end'); field.dispatchEvent(new InputEvent('input')); }
        field.dispatchEvent(new KeyboardEvent('keyup', { shiftKey: false }));
        return event.defaultPrevented;
    }, { inputType, shift });
    assert.equal(await lineBreak('insertParagraph'), true);
    await settle();
    assert.equal((await snapshot()).submits, 1);
    assert.equal(await lineBreak('insertLineBreak', true), true);
    await settle();
    assert.equal((await snapshot()).plain, 'hello\n');
    assert.equal((await snapshot()).submits, 1);

    await page.keyboard.press('Tab'); // Single-line editor.
    await page.keyboard.insertText('one line');
    assert.equal(await lineBreak('insertLineBreak', true), true);
    await settle();
    assert.equal((await snapshot()).single, 'one line');
    assert.equal((await snapshot()).submits, 2);

    await page.keyboard.press('Tab'); // Secure editor must activate a password sink.
    assert.equal(await page.evaluate(() => document.activeElement.type), 'password');
    const password = page.locator('input[type=password]');
    assert.equal(await password.getAttribute('autocomplete'), 'current-password');
    assert.equal(await password.getAttribute('autocorrect'), 'off');
    assert.equal(await password.evaluate(p => p.spellcheck), false);
    for (const ch of 'sëcret') await page.keyboard.insertText(ch);
    assert.equal(await password.inputValue(), '', 'the secret must not remain in the DOM');
    await settle();
    assert.equal((await snapshot()).secretLength, 6, 'secure input reaches the model');
    // Browser/password-manager autofill replaces the typed value instead of appending to it.
    await password.evaluate(field => {
        field.value = 'autofilled-13';
        field.dispatchEvent(new Event('input', { bubbles: true }));
    });
    assert.equal(await password.inputValue(), '', 'filled password must not remain in the DOM');
    await settle();
    assert.equal((await snapshot()).secretLength, 13, 'autofill replaces typed secret');
    // Multi-grapheme insertReplacementText also replaces the whole password.
    await password.evaluate(field => {
        field.value = 'replaced';
        field.dispatchEvent(new InputEvent('input', { inputType: 'insertReplacementText', data: 'replaced', bubbles: true }));
    });
    await settle();
    assert.equal((await snapshot()).secretLength, 8, 'insertReplacementText replaces secret');
    // Paste still goes through the paste listener and inserts at the caret.
    await password.evaluate(field => {
        const dt = new DataTransfer();
        dt.setData('text/plain', '!!');
        field.dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, cancelable: true, bubbles: true }));
    });
    await settle();
    assert.equal((await snapshot()).secretLength, 10, 'paste inserts into secret');
    await password.evaluate(field => field.dispatchEvent(new InputEvent('beforeinput', {
        inputType: 'deleteContentBackward', cancelable: true, bubbles: true,
    })));
    await settle();
    assert.equal((await snapshot()).secretLength, 9);
    await page.keyboard.press('Enter');
    await settle();
    assert.equal((await snapshot()).submits, 3);

    await page.keyboard.press('Tab'); // A non-editor control must return DOM focus to canvas.
    assert.equal(await page.evaluate(() => document.activeElement.tagName), 'CANVAS');
    await page.keyboard.press('Meta+k');
    await settle();
    assert.equal((await snapshot()).shortcuts, 1, 'canvas shortcuts survive the editor-to-canvas focus handoff');
    await page.keyboard.press('Shift+Tab');
    assert.equal(await page.evaluate(() => document.activeElement.type), 'password');
    await page.keyboard.press('Escape');
    assert.equal(await page.evaluate(() => document.activeElement.tagName), 'CANVAS');
    await page.keyboard.press('Meta+k');
    await settle();
    assert.equal((await snapshot()).shortcuts, 2);

    await page.touchscreen.tap(100, 36);
    await page.touchscreen.tap(300, 500); // Background tap dismisses the editor.
    assert.equal(await page.evaluate(() => document.activeElement.tagName), 'CANVAS');
    // Dispatch a burst in one JS turn: movement/wheels must wait for rAF, not run frames.
    const polls = await page.evaluate(() => {
        const before = window.__inputProof.polls;
        const canvas = document.querySelector('canvas');
        for (let i = 0; i < 20; i++) {
            canvas.dispatchEvent(new PointerEvent('pointermove', { pointerType: 'mouse', pointerId: 1,
                button: -1, buttons: 0, clientX: 100 + i, clientY: 500 }));
            canvas.dispatchEvent(new WheelEvent('wheel', { deltaY: 1, clientX: 100, clientY: 500 }));
        }
        return window.__inputProof.polls - before;
    });
    assert.equal(polls, 0, 'continuous pointer events must not run synchronous frames');
    // Each change reaches `poll`, which reads it with `Tx::window_visible`.
    const hiddenPolls = (await snapshot()).polls;
    await page.evaluate(() => {
        Object.defineProperty(document, 'hidden', { configurable: true, value: true });
        document.dispatchEvent(new Event('visibilitychange'));
    });
    await settle();
    let seen = await snapshot();
    assert.equal(seen.visible, false);
    assert.equal(seen.windowVisible, false);
    assert.ok(seen.polls > hiddenPolls, 'hiding runs poll');
    await page.evaluate(() => {
        delete document.hidden;
        document.dispatchEvent(new Event('visibilitychange'));
    });
    await settle();
    seen = await snapshot();
    assert.equal(seen.visible, true);
    assert.equal(seen.windowVisible, true);
    console.log(JSON.stringify({ inputRegressions: 'passed', checks: ['stable editor focus', 'recomposition range',
        'beforeinput Enter', 'multiline vs single-line', 'password sink and deletion', 'Tab to canvas',
        'Escape shortcuts', 'outside dismiss', 'coalesced pointer frames', 'visibility', 'app title', 'nonfatal window requests'] }));
}
