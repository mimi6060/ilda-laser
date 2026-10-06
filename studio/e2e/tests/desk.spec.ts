// Custom desk (T-427): PUPITRE. Components are added, moved, resized and
// bound in « Modifier », and act in « Jouer » where nothing moves; an XY
// pad drives master.pos_x / pos_y and returns to the centre only when
// asked; a fader follows a value changed elsewhere; a cue button in Flash
// plays while held; a missing binding greys its component; arming can't
// be placed on a desk (UI and POST /api/desks); the desk is saved in the
// project; Échap stops and Espace arms in both modes. Preview only:
// "armed" is a flag on the fake output, never a laser.
import { test, expect, useStudio, openUi, openWorkspace, focusPage } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();

interface Item { id: number; kind: string; x: number; y: number; w: number; h: number; label?: string; binding?: unknown; binding_y?: unknown; spring?: boolean; vertical?: boolean }
const desks = async () => (await studio.get<{ tabs: { name: string; items: Item[] }[] }>('/api/desks')).tabs;
const live = async () => (await studio.frame()).live as unknown as { pos_x: number; pos_y: number; size: number };
const ctl = (id: string) => ({ type: 'control', id });
const XY: Item = { id: 1, kind: 'xy_pad', x: 0, y: 0, w: 4, h: 4, binding: ctl('master.pos_x'), binding_y: ctl('master.pos_y') };

async function setDesk(items: Item[]) {
  expect(await studio.post('/api/desks', { tabs: [{ name: 'Test', cols: 12, rows: 8, items }] })).toBe(200);
}
async function openDesk(page: Page) {
  await openWorkspace(page, 'desk');
  await expect(page.locator('#deskRegion')).toBeVisible();
}
async function setMode(page: Page, edit: boolean) {
  const btn = page.locator('#deskMode');
  if ((await btn.getAttribute('aria-pressed')) !== String(edit)) await btn.click();
  await expect(btn).toHaveAttribute('aria-pressed', String(edit));
}
/** Press at (fx, fy) of a component (0..1 from its top left), optionally drag, release. */
async function press(page: Page, sel: string, fx: number, fy: number, to?: [number, number]) {
  const box = (await page.locator(sel).boundingBox())!;
  await page.mouse.move(box.x + box.width * fx, box.y + box.height * fy);
  await page.mouse.down();
  if (to) await page.mouse.move(box.x + box.width * to[0], box.y + box.height * to[1], { steps: 6 });
  return async () => { await page.mouse.up(); };
}

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.post('/api/control', { id: 'cue.mode', value: 0 });
  expect(await studio.post('/api/desks', { tabs: [] })).toBe(200);
  await openUi(page, studio);
});

test('an empty desk says how to start; arming and safety are refused by the server', async ({ page }) => {
  await openDesk(page);
  await expect(page.locator('#deskEmpty')).toBeVisible();
  await expect(page.locator('#deskEmpty')).toContainText('Passez en mode Modifier');
  for (const id of ['transport.arm', 'safety.hold']) {
    expect(await studio.post('/api/desks', [{ items: [{ id: 1, kind: 'button', binding: ctl(id) }] }]), id).toBe(400);
  }
  expect(await studio.post('/api/desks', [{ items: [{ id: 1, kind: 'button', binding: ctl('safety.estop') }] }])).toBe(200);
  expect(await studio.post('/api/desks', [{ items: [{ id: 1, w: 2 }, { id: 2, x: 1 }] }]), 'overlap').toBe(400);
  // The laser button and ARRÊT stay in the top bar, outside the desk.
  for (const sel of ['#armBtn', '#estopBtn']) {
    await expect(page.locator(sel)).toBeVisible();
    expect(await page.locator(sel).evaluate(el => !el.closest('#deskRegion'))).toBe(true);
  }
});

test('Modifier: add an XY pad, bind it (arming refused), then Jouer drives the look', async ({ page }) => {
  await openDesk(page);
  await setMode(page, true);
  await expect(page.locator('#deskPalette')).toBeVisible();
  await page.locator('#deskPalette [data-add="xy_pad"]').click();
  await expect.poll(async () => (await desks())[0]?.items.length ?? 0).toBe(1);
  await expect(page.locator('#deskInsp')).toBeVisible();
  // Arming can't be bound (the page says so, nothing is saved).
  await page.locator('#deskBind').fill('transport.arm');
  await page.locator('#deskBind').press('Tab');
  await expect(page.locator('#deskMsg')).toContainText('ne peut pas être placé');
  expect((await desks())[0].items[0].binding).toEqual({ type: 'none' });
  await page.locator('#deskBind').fill('master.pos_x');
  await page.locator('#deskBind').press('Tab');
  await page.locator('#deskBindY').fill('master.pos_y');
  await page.locator('#deskBindY').press('Tab');
  await expect.poll(async () => (await desks())[0].items[0].binding_y).toEqual(ctl('master.pos_y'));
  expect((await desks())[0].items[0].binding).toEqual(ctl('master.pos_x'));

  await setMode(page, false);
  const pad = '#deskGrid .desk-item.k-xy_pad';
  await expect(page.locator(pad)).toHaveAttribute('data-control', 'master.pos_x');
  // Right edge, top: x = 1, y = 1; dragging to 3/4, 1/4: x = 0.5, y = 0.5.
  const up = await press(page, pad, 0.5, 0.5, [0.75, 0.25]);
  await expect.poll(async () => (await live()).pos_x).toBeCloseTo(0.5, 1);
  await expect.poll(async () => (await live()).pos_y).toBeCloseTo(0.5, 1);
  await up();
  await page.waitForTimeout(300);
  // Released without « Retour au centre »: it stays.
  expect((await live()).pos_x).toBeCloseTo(0.5, 1);
  expect((await live()).pos_y).toBeCloseTo(0.5, 1);
  expect((await studio.state()).armed).toBe(false);
});

test('« Retour au centre » puts both controls back to their default on release', async ({ page }) => {
  await setDesk([{ ...XY, spring: true }]);
  await openDesk(page);
  await setMode(page, false);
  const up = await press(page, '#deskGrid .k-xy_pad', 0.5, 0.5, [0.1, 0.9]);
  await expect.poll(async () => (await live()).pos_x).toBeLessThan(-0.5);
  await expect.poll(async () => (await live()).pos_y).toBeLessThan(-0.5);
  await up();
  await expect.poll(async () => (await live()).pos_x).toBeCloseTo(0, 3);
  await expect.poll(async () => (await live()).pos_y).toBeCloseTo(0, 3);
});

test('Jouer moves nothing; Modifier moves, resizes and drops from the palette', async ({ page }) => {
  await setDesk([XY]);
  await openDesk(page);
  await setMode(page, false);
  const pad = '#deskGrid .k-xy_pad';
  const up = await press(page, pad, 0.5, 0.5, [2.5, 1.5]);
  await up();
  await page.waitForTimeout(200);
  expect((await desks())[0].items[0]).toMatchObject({ x: 0, y: 0, w: 4, h: 4 });

  await setMode(page, true);
  const grid = (await page.locator('#deskGrid').boundingBox())!;
  const cw = grid.width / 12, ch = grid.height / 8;
  // Move by +3 columns, +1 row.
  await page.mouse.move(grid.x + cw * 0.5, grid.y + ch * 0.5);
  await page.mouse.down();
  await page.mouse.move(grid.x + cw * 3.5, grid.y + ch * 1.5, { steps: 6 });
  await page.mouse.up();
  await expect.poll(async () => (await desks())[0].items[0]).toMatchObject({ x: 3, y: 1, w: 4, h: 4 });
  // Resize with the handle: one column narrower, one row taller.
  // The desk is drawn again after the move is saved: wait for its handle.
  await expect(page.locator(`${pad}.sel .desk-resize`)).toBeVisible();
  const handle = (await page.locator(`${pad} .desk-resize`).boundingBox())!;
  await page.mouse.move(handle.x + handle.width / 2, handle.y + handle.height / 2);
  await page.mouse.down();
  await page.mouse.move(handle.x + handle.width / 2 - cw, handle.y + handle.height / 2 + ch, { steps: 6 });
  await page.mouse.up();
  await expect.poll(async () => (await desks())[0].items[0]).toMatchObject({ x: 3, y: 1, w: 3, h: 5 });
  // A fader dragged from the palette onto column 10, row 2.
  const src = (await page.locator('#deskPalette [data-add="fader"]').boundingBox())!;
  await page.mouse.move(src.x + src.width / 2, src.y + src.height / 2);
  await page.mouse.down();
  await page.mouse.move(grid.x + cw * 9.5, grid.y + ch * 1.5, { steps: 8 });
  await page.mouse.up();
  await expect.poll(async () => (await desks())[0].items.find(i => i.kind === 'fader')).toMatchObject({ x: 9, y: 1, w: 1, h: 4 });
  // Suppr deletes the selection (the fader, just added).
  await focusPage(page);
  await page.locator('#deskGrid .k-fader').click();
  await page.keyboard.press('Delete');
  await expect.poll(async () => (await desks())[0].items.length).toBe(1);
  // The layout is an edit: in the history.
  const h = await studio.get('/api/history');
  expect(h.undo[0].label).toMatch(/pupitre/i);
});

test('a fader shows a value changed elsewhere; a Flash cue plays while held', async ({ page }) => {
  const cue = (await studio.presets()).presets[0].id;
  await setDesk([
    { id: 1, kind: 'fader', x: 0, y: 0, w: 1, h: 4, binding: ctl('master.size') },
    { id: 2, kind: 'button', x: 2, y: 0, w: 2, h: 1, binding: { type: 'cue', id: cue } },
  ]);
  await openDesk(page);
  await setMode(page, false);
  const fader = page.locator('#deskGrid .k-fader');
  await expect(fader).toHaveAttribute('role', 'slider');
  // master.size 0..2: 1.5 is 75 %.
  expect(await studio.post('/api/control', { id: 'master.size', value: 1.5 })).toBe(200);
  await expect(fader).toHaveAttribute('aria-valuenow', '75');
  // Dragging the fader writes the control, clamped by the registry.
  const up = await press(page, '#deskGrid .k-fader', 0.5, 0.5, [0.5, -2]);
  await expect.poll(async () => (await live()).size).toBeCloseTo(2, 2);
  await up();

  expect(await studio.post('/api/control', { id: 'cue.mode', value: 1 })).toBe(200); // Flash
  const release = await press(page, '#deskGrid .k-button', 0.5, 0.5);
  await expect.poll(async () => (await studio.frame() as any).cues.active.map((a: { cue: string }) => a.cue)).toContain(cue);
  await expect(page.locator('#deskGrid .k-button')).toHaveClass(/on/);
  await release();
  await expect.poll(async () => (await studio.frame() as any).cues.active.length).toBe(0);
});

test('a missing binding greys its component without a console error', async ({ page }) => {
  const errors: string[] = [];
  page.on('console', m => { if (m.type() === 'error') errors.push(m.text()); });
  page.on('pageerror', e => errors.push(String(e)));
  await setDesk([
    { id: 1, kind: 'button', x: 0, y: 0, w: 2, h: 1, binding: ctl('grid.9.9.9') },
    { id: 2, kind: 'button', x: 2, y: 0, w: 2, h: 1, binding: { type: 'cue', id: 'plus-la' } },
    { id: 3, kind: 'fader', x: 4, y: 0, w: 1, h: 3, binding: ctl('master.brightness') },
  ]);
  await openDesk(page);
  await setMode(page, false);
  const items = page.locator('#deskGrid .desk-item');
  await expect(items.nth(0)).toHaveClass(/missing/);
  await expect(items.nth(1)).toHaveClass(/missing/);
  await expect(items.nth(0)).toContainText('Liaison manquante');
  await expect(items.nth(2)).not.toHaveClass(/missing/);
  await page.screenshot({ path: 'test-results/desk-play.png' });
  await setMode(page, true);
  await items.nth(2).click();
  await page.screenshot({ path: 'test-results/desk-edit.png' });
  await setMode(page, false);
  await items.nth(0).click({ force: true });
  await items.nth(1).click({ force: true });
  await page.waitForTimeout(300);
  expect(errors).toEqual([]);
});

test('Échap stops and Espace arms in Jouer and in Modifier', async ({ page }) => {
  await setDesk([XY]);
  await openDesk(page);
  for (const edit of [false, true]) {
    await setMode(page, edit);
    await focusPage(page);
    await page.keyboard.press('Space');
    await expect.poll(async () => (await studio.state()).armed, `Space arms (edit ${edit})`).toBe(true);
    await page.keyboard.press('Space');
    await expect.poll(async () => (await studio.state()).armed).toBe(false);
    await page.keyboard.press('Space');
    await expect.poll(async () => (await studio.state()).armed).toBe(true);
    await page.keyboard.press('Escape');
    await expect.poll(async () => (await studio.state()).estop, `Escape stops (edit ${edit})`).toBe(true);
    expect((await studio.state()).armed).toBe(false);
    await page.locator('#estopReset').click();
    await expect.poll(async () => (await studio.state()).estop).toBe(false);
  }
  // Alt+F5 opens the desk from another workspace.
  await openWorkspace(page, 'live');
  await focusPage(page);
  await page.keyboard.press('Alt+F5');
  await expect(page.locator('#wsTabs [data-ws-tab="desk"]')).toHaveClass(/active/);
});

test('show mode: the desk plays, its layout is locked', async ({ page }) => {
  await setDesk([XY]);
  await openDesk(page);
  await setMode(page, true);
  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  await expect(page.locator('#deskMode')).toBeHidden();
  await expect(page.locator('#deskPalette')).toBeHidden();
  expect(await studio.post('/api/desks', { tabs: [] })).toBe(423);
  const up = await press(page, '#deskGrid .k-xy_pad', 0.5, 0.5, [0.75, 0.5]);
  await expect.poll(async () => (await live()).pos_x).toBeCloseTo(0.5, 1);
  await up();
  expect(await studio.post('/api/lock', { show_mode: false })).toBe(200);
});

test('the desk is saved in the project and comes back when it is opened', async ({ page }) => {
  await setDesk([{ ...XY, label: 'Position' }]);
  expect(await studio.post('/api/project/save-as', { path: 'Pupitre e2e' })).toBe(200);
  expect(await studio.post('/api/desks', { tabs: [] })).toBe(200);
  expect(await studio.post('/api/project/open', { path: 'Pupitre e2e' })).toBe(200);
  const tabs = await desks();
  expect(tabs[0].name).toBe('Test');
  expect(tabs[0].items[0]).toMatchObject({ label: 'Position', binding_y: ctl('master.pos_y') });
  await openDesk(page);
  await expect(page.locator('#deskGrid .k-xy_pad')).toContainText('Position');
});
