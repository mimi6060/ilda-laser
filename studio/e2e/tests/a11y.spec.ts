// Accessibility and keyboard navigation (T-436): an axe audit of every
// workspace, a full Tab walk of LIVE with a visible focus ring, the cue
// grid on the arrows with `0` as its play key (never Espace, the arming,
// nor Entrée, the tap), tab lists on ← / →, and dialogs that keep the
// focus while Échap stays the emergency stop. Preview only: "armed" is a
// flag on the fake output, never a laser.
// axe-core / @axe-core/playwright: MPL-2.0, a test-only dev dependency.
import AxeBuilder from '@axe-core/playwright';
import { test, expect, useStudio, openUi, openWorkspace, focusPage } from '../studio';

const studio = useStudio();
const armed = async () => (await studio.state()).armed as boolean;
const activeCue = async () => (await studio.controlValues()).active_cue;
const focused = (page: import('@playwright/test').Page) => page.evaluate(() => {
  const el = document.activeElement as HTMLElement | null;
  return el ? { id: el.id, slot: el.dataset.slot ?? null, control: el.dataset.control ?? null, tab: el.getAttribute('role') === 'tab' ? el.textContent : null } : null;
});

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  await studio.post('/api/control', { id: 'cue.mode', value: 0 }); // Basculer
  await studio.post('/api/grid', { cols: 8, rows: 5, pages: [] });
  await studio.post('/api/control', { id: 'page.1' });
  await openUi(page, studio);
});

/** Serious or critical axe findings (WCAG 2.1 A/AA: names, roles, values, contrast). */
async function audit(page: import('@playwright/test').Page, where: string) {
  await page.waitForTimeout(300); // built lists get their names on the next slider pass (200 ms)
  const r = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
  const bad = r.violations.filter(v => v.impact === 'serious' || v.impact === 'critical')
    .map(v => `${where}: ${v.id} (${v.impact}) ${v.nodes.slice(0, 5).map(n => n.target.join(' ')).join(', ')}`);
  expect(bad, bad.join('\n')).toEqual([]);
  return r;
}

test('axe finds nothing serious in any workspace', async ({ page }) => {
  // Something on show, so the active states are audited too.
  await studio.post('/api/cue', { id: (await studio.presets()).presets[0].id, down: true });
  for (const tab of ['direct', 'lfo', 'fx', 'music', 'scenes', 'cuelist', 'running']) {
    await page.locator(`#liveTabs [data-panel-tab="${tab}"]`).click();
    await page.evaluate(() => document.querySelectorAll('#livePanel details').forEach(d => { (d as HTMLDetailsElement).open = true; }));
    const r = await audit(page, 'LIVE › ' + tab);
    if (tab === 'direct') {
      // The rules that matter here did run (not skipped as inapplicable).
      const ran = new Set([...r.passes, ...r.violations].map(v => v.id));
      for (const id of ['color-contrast', 'button-name', 'label', 'aria-required-children', 'aria-allowed-attr']) expect(ran, id).toContain(id);
    }
  }
  await page.locator('#liveTabs [data-panel-tab="direct"]').click();
  await openWorkspace(page, 'timeline');
  await audit(page, 'TIMELINE');
  await openWorkspace(page, 'creation');
  for (const tab of ['look', 'figures', 'media', 'program']) {
    await page.locator(`#creationTabs [data-ctab="${tab}"]`).click();
    await audit(page, 'CRÉATION › ' + tab);
  }
  await page.locator('#creationTabs [data-ctab="look"]').click();
  await openWorkspace(page, 'settings');
  await page.evaluate(() => document.querySelectorAll('#settingsPanel details').forEach(d => { (d as HTMLDetailsElement).open = true; }));
  await audit(page, 'RÉGLAGES');
  // PUPITRE (T-427): played and edited, with one of each component.
  expect(await studio.post('/api/desks', { tabs: [{ name: 'A11y', items: [
    { id: 1, kind: 'button', x: 0, y: 0, w: 2, h: 1, binding: { type: 'control', id: 'tempo.tap' } },
    { id: 2, kind: 'fader', x: 2, y: 0, w: 1, h: 4, binding: { type: 'control', id: 'master.size' } },
    { id: 3, kind: 'xy_pad', x: 3, y: 0, w: 3, h: 3, binding: { type: 'control', id: 'master.pos_x' }, binding_y: { type: 'control', id: 'master.pos_y' } },
    { id: 4, kind: 'label', x: 6, y: 0, w: 3, h: 1, label: 'Intro' },
    { id: 5, kind: 'preview', x: 6, y: 1, w: 3, h: 3 },
    { id: 6, kind: 'button', x: 0, y: 1, w: 2, h: 1, binding: { type: 'control', id: 'grid.9.9.9' } },
  ] }] })).toBe(200);
  await openWorkspace(page, 'desk');
  await audit(page, 'PUPITRE');
  await page.locator('#deskMode').click();
  await page.locator('#deskGrid .k-fader').click();
  await audit(page, 'PUPITRE › Modifier');
  await page.locator('#deskMode').click();
  expect(await studio.post('/api/desks', { tabs: [] })).toBe(200);
  await openWorkspace(page, 'live');
});

test('nothing has the focus at start, and never the laser button', async ({ page }) => {
  await page.goto(studio.url + '/');
  await expect(page.locator('#cues .cue').first()).toBeVisible();
  expect(await page.evaluate(() => document.activeElement === document.body)).toBe(true);
  await expect(page.locator('#armBtn')).toHaveAttribute('aria-pressed', 'false');
  await expect(page.locator('#estopBanner')).toHaveAttribute('role', 'alert');
  await expect(page.locator('#armRefused')).toHaveAttribute('role', 'alert');
  await expect(page.locator('#presenceLed')).toHaveAttribute('role', 'status');
  await expect(page.locator('#engineLed')).toHaveAttribute('role', 'status');
  await focusPage(page);
  await page.keyboard.press('Space');
  await expect.poll(armed).toBe(true);
  await expect(page.locator('#armBtn')).toHaveAttribute('aria-pressed', 'true');
  await page.keyboard.press('Space');
  await expect.poll(armed).toBe(false);
});

test('Tab reaches every LIVE control, each with a visible focus ring', async ({ page }) => {
  // Every visible, enabled control of LIVE › Direct (the grid counts once: one tab stop).
  const wanted: string[] = await page.evaluate(() => [...document.querySelectorAll<HTMLElement>('[data-control]')]
    .filter(el => el.matches('button, input, select, textarea') && !(el as HTMLButtonElement).disabled && el.getClientRects().length && !el.closest('#cues'))
    .map(el => el.dataset.control!));
  expect(wanted.length).toBeGreaterThan(40);
  await focusPage(page);
  const seen = new Set<string>();
  const order: string[] = [];
  let gridStops = 0;
  for (let i = 0; i < 700; i++) {
    await page.keyboard.press('Tab');
    const f = await page.evaluate(() => {
      const el = document.activeElement as HTMLElement | null;
      if (!el || el === document.body) return null;
      const cs = getComputedStyle(el);
      return { control: el.dataset.control ?? null, grid: !!el.closest('#cues'), where: el.closest('#top') ? 'top' : el.closest('#cues') ? 'grid' : el.closest('#layers') ? 'layers' : 'other',
        ring: cs.outlineStyle !== 'none' && parseFloat(cs.outlineWidth) >= 2, tag: el.tagName + (el.id ? '#' + el.id : '') };
    });
    if (!f) { if (seen.size) break; continue; }
    expect(f.ring, `focus ring on ${f.tag}`).toBe(true);
    if (f.grid) gridStops++;
    if (order[order.length - 1] !== f.where) order.push(f.where);
    if (f.control) {
      if (seen.has(f.control) && f.control === wanted[0]) break; // looped
      seen.add(f.control);
    }
  }
  expect(wanted.filter(c => !seen.has(c))).toEqual([]);
  // The grid is one stop; the top bar comes first, the layers after the grid.
  expect(gridStops).toBe(1);
  expect(order[0]).toBe('top');
  expect(order.indexOf('grid')).toBeLessThan(order.indexOf('layers'));
});

test('workspace tabs: one tab stop, ← / → move and select', async ({ page }) => {
  const stops = await page.locator('#wsTabs [role=tab][tabindex="0"]').count();
  expect(stops).toBe(1);
  await page.locator('#wsTabs [data-ws-tab="live"]').focus();
  await page.keyboard.press('ArrowRight');
  await expect(page.locator('#wsTabs [data-ws-tab="timeline"]')).toHaveAttribute('aria-selected', 'true');
  await expect(page.locator('#wsTabs [data-ws-tab="timeline"]')).toBeFocused();
  await expect(page.locator('#wsTabs [data-ws-tab="timeline"]')).toHaveAttribute('tabindex', '0');
  await expect(page.locator('#wsTabs [data-ws-tab="live"]')).toHaveAttribute('tabindex', '-1');
  await page.keyboard.press('ArrowLeft');
  await page.keyboard.press('ArrowLeft'); // wraps round to PUPITRE (T-427)
  await expect(page.locator('#wsTabs [data-ws-tab="desk"]')).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('ArrowLeft');
  await expect(page.locator('#wsTabs [data-ws-tab="settings"]')).toHaveAttribute('aria-selected', 'true');
  await openWorkspace(page, 'live');
  // The LIVE panel tabs too.
  await page.locator('#liveTabs [data-panel-tab="direct"]').click();
  await page.keyboard.press('ArrowRight');
  await expect(page.locator('#liveTabs [data-panel-tab="lfo"]')).toHaveAttribute('aria-selected', 'true');
  await page.locator('#liveTabs [data-panel-tab="direct"]').click();
});

test('the grid: arrows move, Espace arms and Entrée taps without playing, 0 plays', async ({ page }) => {
  const cells = page.locator('#cues [data-slot]');
  await expect(page.locator('#cues')).toHaveAttribute('role', 'grid');
  await expect(page.locator('#cues [role=row]')).toHaveCount(5);
  await expect(page.locator('#cues [role=gridcell]')).toHaveCount(40);
  await expect(page.locator('#cues [tabindex="0"]')).toHaveCount(1);
  const first = page.locator('#cues .cue').first();
  await expect(first).toHaveAttribute('aria-label', (await first.locator('.n').textContent())!);
  await first.focus();
  expect(await page.evaluate(() => (window as unknown as { shortcutsDebug: { zone: () => string } }).shortcutsDebug.zone())).toBe('grid');
  await page.keyboard.press('ArrowRight');
  expect((await focused(page))!.slot).toBe('1');
  await page.keyboard.press('ArrowDown');
  expect((await focused(page))!.slot).toBe('9');
  await page.keyboard.press('End');
  expect((await focused(page))!.slot).toBe('15');
  await page.keyboard.press('Home');
  expect((await focused(page))!.slot).toBe('8');
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('ArrowLeft'); // stays on the first cell
  expect((await focused(page))!.slot).toBe('0');
  await expect(cells.nth(0)).toHaveAttribute('tabindex', '0');
  await expect(cells.nth(1)).toHaveAttribute('tabindex', '-1');

  // Espace on a focused cell: arms (preview), plays nothing.
  await page.keyboard.press('Space');
  await expect.poll(armed).toBe(true);
  await page.waitForTimeout(300);
  expect(await activeCue()).toBeNull();
  await page.keyboard.press('Space');
  await expect.poll(armed).toBe(false);
  // Entrée: a tap, never the cue.
  const tap = page.waitForRequest(r => r.url().endsWith('/api/control') && (r.postData() ?? '').includes('"tempo.tap"'));
  await page.keyboard.press('Enter');
  await tap;
  await page.waitForTimeout(300);
  expect(await activeCue()).toBeNull();
  await expect(cells.nth(0)).toBeFocused();

  // `0` plays the focused cell (Basculer: a second press stops it).
  const id = await cells.nth(0).getAttribute('data-id');
  await page.keyboard.press('Digit0');
  await expect.poll(activeCue).toBe(id);
  await expect(cells.nth(0)).toHaveAttribute('aria-selected', 'true');
  await page.keyboard.press('Numpad0');
  await expect.poll(activeCue).toBeNull();
  await expect(cells.nth(0)).toHaveAttribute('aria-selected', 'false');

  // The key is in the « ? » help, in the single table.
  await page.locator('#keysBtn').click();
  await expect(page.locator('#keysList [data-shortcut="grid.activate"]')).toContainText('Déclencher la case sélectionnée');
  await expect(page.locator('#keysList [data-shortcut="grid.move"]')).toBeVisible();
  await page.locator('#keysClose').click();
  const conflicts = await page.evaluate(() => (window as unknown as { shortcutsDebug: { conflicts: () => unknown[] } }).shortcutsDebug.conflicts());
  expect(conflicts).toEqual([]);
});

test('0 held on a Flash cell plays it until released', async ({ page }) => {
  await studio.post('/api/control', { id: 'cue.mode', value: 1 }); // Flash
  await expect(page.locator('#cueModes [data-mode="flash"]')).toHaveClass(/active/);
  const cell = page.locator('#cues .cue').nth(2);
  const id = await cell.getAttribute('data-id');
  await cell.focus();
  await page.keyboard.down('Digit0');
  await expect.poll(activeCue).toBe(id);
  await page.waitForTimeout(300);
  expect(await activeCue()).toBe(id); // still held
  await page.keyboard.up('Digit0');
  await expect.poll(activeCue).toBeNull();
  // Outside the grid, 0 does nothing.
  await focusPage(page);
  await page.keyboard.press('Digit0');
  await page.waitForTimeout(300);
  expect(await activeCue()).toBeNull();
});

test('the Projet dialog keeps the focus, gives it back, and Échap still stops', async ({ page }) => {
  await page.locator('#projBtn').focus();
  await page.keyboard.press('Space'); // Espace arms, it never opens the menu
  await expect.poll(armed).toBe(true);
  await page.locator('#projBtn').click();
  await page.locator('#projMenu [data-proj="open"]').click();
  const dlg = page.locator('#projDialog');
  await expect(dlg).toBeVisible();
  await expect(dlg).toHaveAttribute('role', 'dialog');
  await expect(page.locator('#projPath')).toBeFocused();
  // Tab and Maj+Tab loop inside.
  for (let i = 0; i < 12; i++) {
    await page.keyboard.press('Tab');
    expect(await page.evaluate(() => !!document.activeElement?.closest('#projDialog'))).toBe(true);
  }
  await page.locator('#projPath').focus();
  await page.keyboard.press('Shift+Tab');
  await expect(page.locator('#projCancel')).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(page.locator('#projPath')).toBeFocused();
  // Closed by its own button: the focus goes back to « Projet » (the menu has closed).
  await page.locator('#projCancel').focus();
  await page.keyboard.press('Tab'); // still inside
  await page.locator('#projCancel').click();
  await expect(dlg).toBeHidden();
  await expect(page.locator('#projBtn')).toBeFocused();

  // Échap in an open dialog: the emergency stop.
  await page.locator('#projBtn').click();
  await page.locator('#projMenu [data-proj="open"]').click();
  await expect(page.locator('#projPath')).toBeFocused();
  await page.keyboard.press('Escape');
  await expect.poll(armed).toBe(false);
  await expect(page.locator('#estopBanner')).toBeVisible();
  await page.locator('#projCancel').click();
  await expect(dlg).toBeHidden();
  await studio.post('/api/estop/reset', {});
});
