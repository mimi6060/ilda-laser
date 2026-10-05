// One slider behaviour everywhere (T-431): double-click to type a value
// (comma or point, unit tolerated, clamped to the registry's bounds), the
// name's right click / double-click for the default, Maj + drag for fine
// moves, arrows and the wheel only with the focus, the right click on the
// slider still the MIDI menu, a loosening safety reset still confirmed,
// Space and Escape still working afterwards. Preview only (fake output).
import { test, expect, useStudio, openUi, openWorkspace, type Page, type Locator } from '../studio';

const studio = useStudio();
type Kind = { type: string; min: number; max: number; default: number; unit: string };
let kinds: Record<string, Kind> = {};
const values = async () => (await studio.controlValues()).values as Record<string, number>;
const armed = async () => (await studio.state()).armed as boolean;

test.beforeAll(async () => {
  const list = await studio.get<{ id: string; kind: Kind }[]>('/api/controls');
  kinds = Object.fromEntries((Array.isArray(list) ? list : (list as any).controls).map((c: { id: string; kind: Kind }) => [c.id, c.kind]));
});

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  await openUi(page, studio);
});

test.afterEach(async () => {
  await studio.post('/api/arm', { on: false });
});

/** Show the workspace / LIVE tab holding the element, like a user would. */
async function show(page: Page, loc: Locator) {
  const where = await loc.evaluate(el => ({
    ws: (el.closest('[data-ws]') as HTMLElement | null)?.dataset.ws ?? null,
    panel: (el.closest('[data-panel]') as HTMLElement | null)?.dataset.panel ?? null,
  }));
  if (where.ws) await openWorkspace(page, where.ws as any);
  if (where.panel) await page.locator(`#liveTabs [data-panel-tab="${where.panel}"]`).click();
  await loc.scrollIntoViewIfNeeded();
  await expect(loc).toBeVisible();
}
/** The slider's name (what the page itself resets from). */
async function nameOf(page: Page, slider: Locator) {
  const n = await slider.evaluate(el => {
    const l = (window as any).eval('sliderLabel')(el) as HTMLElement | null;
    if (!l) return 0;
    const tag = String(Math.random()).slice(2);
    l.dataset.testName = tag;
    return tag;
  });
  expect(n, 'the slider has a name').toBeTruthy();
  return page.locator(`[data-test-name="${n}"]`);
}
async function typeInto(page: Page, slider: Locator, text: string) {
  await slider.dblclick();
  const entry = page.locator('.slider-entry');
  await expect(entry).toBeVisible();
  await entry.fill(text);
  await entry.press('Enter');
  await expect(entry).toHaveCount(0);
}

test('every [data-control] slider: typed value, clamped bounds, default from its name', async ({ page }) => {
  const sliders = page.locator('input[type=range][data-control]');
  const n = await sliders.count();
  expect(n).toBeGreaterThan(20);
  for (let i = 0; i < n; i++) {
    const s = sliders.nth(i);
    const a = await s.evaluate((el: HTMLInputElement) => ({ id: el.dataset.control!, min: +el.min, max: +el.max, step: el.getAttribute('step'), k: el.dataset.k, dk: el.dataset.displayK, live: !!el.dataset.live }));
    const kind = kinds[a.id];
    expect(kind?.type, a.id).toBe('continuous');
    const k = a.k ? +a.k : Math.round((kind.max ? a.max / kind.max : a.min / kind.min) * 1000) / 1000;
    const dk = a.dk ? +a.dk : 1;
    const span = kind.max - kind.min;
    await show(page, s);
    // A mid value, typed with a comma. A master slider takes it exactly;
    // the others round it to their step, like a drag: whole numbers there.
    const whole = !a.live && (a.step === null || +a.step >= 1);
    const shown = (a.min + (a.max - a.min) * 0.37) * dk;
    const typed = whole ? Math.round(shown) : Math.round(shown * 10) / 10;
    await typeInto(page, s, String(typed).replace('.', ','));
    const tol = a.live ? span * 0.002 : +(a.step ?? 1) / k / 2 + 1e-6;
    await expect.poll(async () => Math.abs((await values())[a.id] - typed / dk / k) <= tol, `${a.id} typed ${typed}`).toBe(true);
    // Out of range: the bound, with a unit after it.
    await typeInto(page, s, `${(a.max * dk + 1000).toFixed(0)} %`);
    await expect.poll(async () => (await values())[a.id], `${a.id} clamped`).toBeCloseTo(kind.max, 3);
    await expect(page.locator('#lockToast')).toContainText('Limité à');
    // Back to the registry's default from its name.
    await (await nameOf(page, s)).click({ button: 'right' });
    await expect.poll(async () => (await values())[a.id], `${a.id} default`).toBeCloseTo(kind.default, 3);
    await expect(page.locator('#midiMenu')).toBeHidden();
  }
});

test('typing: comma, sign, unit; invalid text changes nothing; Échap cancels and still stops', async ({ page }) => {
  const s = page.locator('#mTiltX');
  await show(page, s);
  await typeInto(page, s, '-45,5°');
  await expect.poll(async () => (await values())['master.rot_x.angle']).toBeCloseTo(-45.5, 3);
  await typeInto(page, s, 'abc');
  await page.waitForTimeout(200);
  expect((await values())['master.rot_x.angle']).toBeCloseTo(-45.5, 3);
  await s.dblclick();
  await page.locator('.slider-entry').fill('90');
  await page.keyboard.press('Escape');
  await expect(page.locator('.slider-entry')).toHaveCount(0);
  await expect(page.locator('#estopBanner')).toBeVisible();
  expect((await values())['master.rot_x.angle']).toBeCloseTo(-45.5, 3);
  await studio.post('/api/estop/reset', {});
  // Typed in the shown unit: « Étalement » shows cycles (0,00 … 4,00).
  const sp = page.locator('#colorSpread');
  await show(page, sp);
  await typeInto(page, sp, '2,25');
  await expect.poll(async () => (await values())['master.color.spread']).toBeCloseTo(2.25, 3);
  // A clamped value says so.
  await show(page, page.locator('#mSize'));
  await typeInto(page, page.locator('#mSize'), '500');
  await expect(page.locator('#lockToast')).toHaveText('Limité à 200 %');
});

test('the wheel only moves a focused slider; Maj + drag is ten times finer', async ({ page }) => {
  const s = page.locator('#mSize');
  await show(page, s);
  const box = (await s.boundingBox())!;
  const cx = box.x + box.width / 2, cy = box.y + box.height / 2;
  await page.locator('h1').click();
  await page.mouse.move(cx, cy);
  await page.mouse.wheel(0, -400);
  await page.waitForTimeout(300);
  expect((await values())['master.size']).toBeCloseTo(1, 3);
  await s.focus();
  await page.mouse.wheel(0, -100);
  await expect.poll(async () => (await values())['master.size']).toBeGreaterThan(1);
  await studio.post('/api/control', { id: 'master.size', value: 1 });

  const dragBy = async (shift: boolean) => {
    await studio.post('/api/control', { id: 'master.size', value: 1 });
    await page.locator('h1').click();
    await expect.poll(async () => s.inputValue()).toBe('100');
    if (shift) await page.keyboard.down('Shift');
    await page.mouse.move(cx, cy);
    await page.mouse.down();
    await page.waitForTimeout(100);
    const start = (await values())['master.size'];
    await page.mouse.move(cx + 100, cy, { steps: 10 });
    await page.mouse.up();
    if (shift) await page.keyboard.up('Shift');
    await page.waitForTimeout(300);
    return (await values())['master.size'] - start;
  };
  const plain = await dragBy(false);
  const fine = await dragBy(true);
  expect(plain).toBeGreaterThan(0.3);
  expect(fine / plain).toBeGreaterThan(0.07);
  expect(fine / plain).toBeLessThan(0.13);
});

test('arrows with the focus: a step, Maj a tenth, Page ten steps, Début / Fin the bounds', async ({ page }) => {
  const s = page.locator('#mSize');
  await show(page, s);
  await s.focus();
  await page.keyboard.press('ArrowRight');
  await expect.poll(async () => (await values())['master.size']).toBeCloseTo(1.01, 4);
  await page.keyboard.press('Shift+ArrowRight');
  await expect.poll(async () => (await values())['master.size']).toBeCloseTo(1.011, 4);
  await page.keyboard.press('PageDown');
  await expect.poll(async () => (await values())['master.size']).toBeCloseTo(0.911, 4);
  await page.keyboard.press('End');
  await expect.poll(async () => (await values())['master.size']).toBeCloseTo(2, 4);
  await page.keyboard.press('Home');
  await expect.poll(async () => (await values())['master.size']).toBeCloseTo(0, 4);
});

test('the right click on a slider is still « Apprendre MIDI »; double-click on its name resets', async ({ page }) => {
  const s = page.locator('#mSpeed');
  await show(page, s);
  await s.click({ button: 'right' });
  await expect(page.locator('#midiMenu')).toBeVisible();
  await expect(page.locator('#midiMenuLearn')).toBeVisible();
  await page.locator('h1').click();
  await expect(page.locator('#midiMenu')).toBeHidden();
  await typeInto(page, s, '250');
  await expect.poll(async () => (await values())['master.speed']).toBeCloseTo(2.5, 3);
  await (await nameOf(page, s)).dblclick();
  await expect.poll(async () => (await values())['master.speed']).toBeCloseTo(1, 3);
  await expect(page.locator('.slider-entry')).toHaveCount(0);
});

test('a modulator added later behaves the same', async ({ page }) => {
  await show(page, page.locator('#lfoAdd'));
  await page.locator('#lfoAdd').click();
  const depth = page.locator('#lfoList .lfo').last().locator('input[data-f="depth"]');
  await expect(depth).toBeVisible();
  await typeInto(page, depth, '30 %');
  const lfos = async () => (await studio.get('/api/lfos')).lfos ?? (await studio.get('/api/lfos'));
  await expect.poll(async () => { const l = await lfos(); return l[l.length - 1].depth; }).toBeCloseTo(0.3, 3);
  await (await nameOf(page, depth)).click({ button: 'right' });
  await expect.poll(async () => { const l = await lfos(); return l[l.length - 1].depth; }).toBeCloseTo(0.5, 3);
  await page.locator('#lfoList .lfo').last().locator('.del').click();
});

test('resetting a safety slider that loosens asks first', async ({ page }) => {
  const { settings } = await studio.get('/api/safety');
  settings.color_gain = [0.5, 1, 1];
  expect(await studio.post('/api/safety', settings)).toBe(200);
  await page.reload();
  await openWorkspace(page, 'settings');
  if (!await page.locator('#sfGainR').isVisible()) await page.locator('#safetyPanel summary').click();
  const s = page.locator('#sfGainR');
  await show(page, s);
  await expect(page.locator('#sfGainRV')).toHaveText('50 %');
  await (await nameOf(page, s)).click({ button: 'right' });
  await expect(page.locator('#sfLoosen')).toBeVisible();
  await page.locator('#sfLoosenCancel').click();
  expect((await studio.get('/api/safety')).settings.color_gain[0]).toBe(0.5);
  // Typed, tighter: applied at once, in percent as shown.
  await typeInto(page, s, '40 %');
  await expect.poll(async () => (await studio.get('/api/safety')).settings.color_gain[0]).toBeCloseTo(0.4, 3);
  settings.color_gain = [1, 1, 1];
  expect(await studio.post('/api/safety', { ...settings, confirm_loosen: true })).toBe(200);
});

test('after a slider, Space still arms and Escape still stops (T-291)', async ({ page }) => {
  const s = page.locator('#mBright');
  await show(page, s);
  await typeInto(page, s, '80');
  await expect.poll(async () => (await values())['master.brightness']).toBeCloseTo(0.8, 3);
  // The focus is back on the slider, not in a text field.
  expect(await page.evaluate(() => document.activeElement?.id)).toBe('mBright');
  await page.keyboard.press('Space');
  await expect.poll(armed).toBe(true);
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('Escape');
  await expect.poll(armed).toBe(false);
  await expect(page.locator('#estopBanner')).toBeVisible();
  await studio.post('/api/estop/reset', {});
});
