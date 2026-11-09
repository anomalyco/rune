// A page's fake clock drives Rune's frames, even one installed after the app started
// (Playwright's `clock.install`): with the clock paused, a touch fling's momentum waits for
// `runFor` instead of finishing in real time. Chromium only (touch comes from CDP).
import assert from 'node:assert/strict';

export async function clockSmoke(browser, url) {
    // Its own context: Playwright's clock belongs to a context, and would pause the other pages.
    const context = await browser.newContext({ viewport: { width: 390, height: 760 }, deviceScaleFactor: 3, hasTouch: true, isMobile: true });
    const page = await context.newPage();
    try {
        await page.addInitScript(() => {
            window.__submits = 0;
            const submit = GPUQueue.prototype.submit;
            GPUQueue.prototype.submit = function (...args) { window.__submits++; return submit.apply(this, args); };
        });
        await page.goto(url);
        await page.waitForFunction(() => window.__submits > 0, { timeout: 30000 });
        await page.waitForTimeout(1000);
        await page.clock.install();
        await page.clock.pauseAt(await page.evaluate(() => Date.now() + 1000));
        const cdp = await context.newCDPSession(page);
        const touch = (type, y) => cdp.send('Input.dispatchTouchEvent', { type, touchPoints: type === 'touchEnd' ? [] : [{ x: 180, y }] });
        await touch('touchStart', 180);
        for (let i = 1; i <= 10; i++) {
            await touch('touchMove', 180 + 30 * i);
            await page.clock.runFor(16);
        }
        await touch('touchEnd', 480);
        await page.clock.runFor(16);
        // Two seconds of real time with the page clock paused: the fling must not move on.
        await page.waitForTimeout(2000);
        await page.clock.runFor(16);
        const a = await page.screenshot();
        await page.clock.runFor(32);
        assert.ok(!a.equals(await page.screenshot()), 'momentum follows the paused page clock, not real time');
        await page.clock.runFor(5000);
        const settled = await page.screenshot();
        await page.clock.runFor(100);
        assert.ok(settled.equals(await page.screenshot()), 'momentum settles as the page clock runs');
    } finally {
        await context.close();
    }
}
