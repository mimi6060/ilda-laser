// Timeline editor (T-445): Verrou de piste and Seuil anti-glissement. A
// shaky click selects without moving; a locked track ignores drag, resize,
// delete, paste and duplicate (its events stay selectable, Muet works);
// Tout verrouiller is one undo step; the lock is saved with the show and
// the server refuses a hand-made edit of a locked track. Preview only:
// nothing here arms the laser.
import { test, expect, useStudio, openUi, openWorkspace, type Page } from '../studio';

const studio = useStudio();

interface Ev { id: number; start: number; len: number; source: { kind: string; id: string } }
interface Track { name: string; layer: number; locked: boolean; mute: boolean; events: Ev[] }
interface Show { name: string; tracks: Track[] }
interface View { t0: number; pps: number; top: number; lane: number; ruler: number; mark: number }
const timeline = () => studio.get<{ state: any; show: Show | null }>('/api/timeline');
const serverShow = async () => (await timeline()).show!;
const starts = async (track: number) => (await serverShow()).tracks[track].events.map(e => e.start);
let cues: string[] = [];

test.beforeAll(async () => {
  cues = (await studio.presets()).presets.map(p => p.id);
});

test.beforeEach(async ({ page }) => {
  await studio.post('/api/timeline/stop');
  await studio.post('/api/estop/reset');
  await studio.reset();
  await openUi(page, studio);
});

test.afterEach(async () => {
  expect((await studio.state()).armed, 'the editor never arms').toBe(false);
});

/** A new Secondes show (120 BPM, 1 bar = 2 s) with events on tracks 1 and 2. */
async function seed(name: string, t0: Partial<Ev>[], t1: Partial<Ev>[] = []) {
  expect(await studio.post('/api/timeline/new', { name, time_base: 'seconds' })).toBe(200);
  const show = await serverShow();
  let id = 1;
  const evs = (list: Partial<Ev>[]) => list.map(e => ({ id: id++, start: 0, len: 2, source: { kind: 'cue', id: cues[0] }, ...e }));
  show.tracks[0].events = evs(t0) as Ev[];
  show.tracks[1].events = evs(t1) as Ev[];
  expect(await studio.post('/api/timeline/edit', { show })).toBe(200);
}

async function openEditor(page: Page, name: string) {
  await openWorkspace(page, 'timeline');
  await expect.poll(() => page.evaluate(() => (window as any).tlDebug.show()?.name)).toBe(name);
  await page.evaluate(() => (window as any).tlDebug.setView(0, 40)); // 40 px per second
  await page.locator('#tlSnap [data-snap="strong"]').click();
}
const view = (page: Page) => page.evaluate(() => (window as any).tlDebug.view()) as Promise<View>;
/** Page coordinates of show time `u` in track lane `lane`. */
async function at(page: Page, u: number, lane: number) {
  const v = await view(page);
  const box = (await page.locator('#tlCanvas').boundingBox())!;
  return { x: box.x + (u - v.t0) * v.pps, y: box.y + v.top + lane * v.lane + v.lane / 2 };
}
/** Press, move by `dx` px in small steps, release. */
async function nudge(page: Page, from: { x: number; y: number }, dx: number) {
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move(from.x + dx, from.y, { steps: Math.max(2, Math.abs(dx) / 2) });
  await page.mouse.up();
}
const lockButton = (page: Page, ti: number) => page.locator(`#tlHeads .tl-head[data-ti="${ti}"]:not(.tl-envhead) [data-f="lock"]`);

test('a press that moves less than 4 px selects and moves nothing; 20 px moves', async ({ page }) => {
  await seed('Seuil e2e', [{ start: 4, len: 2 }]);
  await openEditor(page, 'Seuil e2e');
  expect(await page.evaluate(() => (window as any).tlDebug.dragPx)).toBe(4);
  const before = JSON.stringify(await serverShow());
  const rev = (await timeline()).state.rev;

  // 3 px (the strong magnet would have pulled it onto a beat): selected, not moved.
  await nudge(page, await at(page, 5, 0), 3);
  await expect.poll(() => page.evaluate(() => [...(window as any).tlDebug.show().tracks[0].events.map((e: Ev) => e.start)])).toEqual([4]);
  await page.waitForTimeout(300);
  expect((await timeline()).state.rev, 'nothing sent').toBe(rev);
  expect(JSON.stringify(await serverShow())).toBe(before);
  await expect(page.locator('#tlUndo')).toBeDisabled();

  // 20 px = 0.5 s = one beat: moved, from the press position (no jump).
  await nudge(page, await at(page, 5, 0), 20);
  await expect.poll(() => starts(0)).toEqual([4.5]);
});

test('a locked track ignores drag, resize and delete; Muet still works; the server refuses a hand-made edit', async ({ page }) => {
  await seed('Verrou e2e', [{ start: 4, len: 2 }]);
  await openEditor(page, 'Verrou e2e');
  await lockButton(page, 0).click();
  await expect.poll(async () => (await serverShow()).tracks[0].locked).toBe(true);
  await expect(lockButton(page, 0)).toHaveClass(/active/);
  await expect(lockButton(page, 0)).toHaveAttribute('title', /Déverrouiller la piste/);
  await expect(page.locator('#tlHeads .tl-head[data-ti="0"] input[data-f="name"]')).toBeDisabled();
  const before = JSON.stringify((await serverShow()).tracks[0]);

  // Drag 40 px, then to track 2: nothing moves, the message says why.
  await nudge(page, await at(page, 5, 0), 40);
  await expect(page.locator('#tlEdMsg')).toHaveText('Piste verrouillée');
  // Resize the end edge.
  await nudge(page, { ...(await at(page, 6, 0)), x: (await at(page, 6, 0)).x - 2 }, 40);
  // Still selectable: Suppr refused.
  await page.mouse.click((await at(page, 5, 0)).x, (await at(page, 5, 0)).y);
  expect(await page.evaluate(() => [...(window as any).tlDebug.show().tracks[0].events.map((e: Ev) => e.id)].length)).toBe(1);
  await page.keyboard.press('Delete');
  await expect(page.locator('#tlEdMsg')).toHaveText('Piste verrouillée');
  // Deleting the track is refused too.
  await page.locator('#tlHeads .tl-head[data-ti="0"] [data-f="del"]').click();
  await expect(page.locator('#tlEdMsg')).toHaveText('Piste verrouillée');
  await page.waitForTimeout(300);
  expect(JSON.stringify((await serverShow()).tracks[0])).toBe(before);
  expect((await serverShow()).tracks.length).toBe(2);

  // Muet stays usable on a locked track.
  await page.locator('#tlHeads .tl-head[data-ti="0"] [data-f="mute"]').click();
  await expect.poll(async () => (await serverShow()).tracks[0].mute).toBe(true);

  // By hand: the server refuses the change, in French.
  const show = await serverShow();
  show.tracks[0].events[0].start = 4.004;
  const r = await fetch(studio.url + '/api/timeline/edit', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ show }) });
  expect(r.status).toBe(400);
  expect(await r.text()).toContain('piste verrouillée');
  expect(await starts(0)).toEqual([4]);

  // Alt+L (timeline focused, event selected) unlocks: it moves again.
  await page.mouse.click((await at(page, 5, 0)).x, (await at(page, 5, 0)).y);
  await page.keyboard.press('Alt+KeyL');
  await expect.poll(async () => (await serverShow()).tracks[0].locked).toBe(false);
  await nudge(page, await at(page, 5, 0), 40);
  await expect.poll(() => starts(0)).toEqual([5]);
});

test('a selection across locked and free tracks: only the free events move', async ({ page }) => {
  await seed('Mixte e2e', [{ start: 4, len: 2 }], [{ start: 4, len: 2 }, { start: 8, len: 2 }]);
  await openEditor(page, 'Mixte e2e');
  await lockButton(page, 1).click();
  await expect.poll(async () => (await serverShow()).tracks[1].locked).toBe(true);
  await page.locator('#tlCanvas').focus();
  await page.keyboard.press('ControlOrMeta+a');
  await nudge(page, await at(page, 5, 0), 40); // + 1 s
  await expect.poll(() => starts(0)).toEqual([5]);
  await expect(page.locator('#tlEdMsg')).toHaveText('2 événements verrouillés ignorés');
  expect(await starts(1)).toEqual([4, 8]);

  // Paste and duplicate onto the locked track: nothing, « Piste verrouillée ».
  await page.mouse.click((await at(page, 14, 1)).x, (await at(page, 14, 1)).y); // empty: no selection
  await page.mouse.click((await at(page, 5, 1)).x, (await at(page, 5, 1)).y);
  await page.keyboard.press('ControlOrMeta+c');
  await page.keyboard.press('ControlOrMeta+v');
  await expect(page.locator('#tlEdMsg')).toHaveText('Piste verrouillée');
  await page.locator('#tlEdMsg').evaluate(el => { el.textContent = ''; });
  await page.locator('#tlDup').click();
  await expect(page.locator('#tlEdMsg')).toHaveText('Piste verrouillée');
  await page.waitForTimeout(300);
  expect(await starts(1)).toEqual([4, 8]);
  expect(await starts(0)).toEqual([5]);
});

test('Tout verrouiller is one undo step; the lock is saved and comes back', async ({ page }) => {
  await seed('Tout e2e', [{ start: 4, len: 2 }]);
  await openEditor(page, 'Tout e2e');
  await expect(page.locator('#tlLockAll')).toHaveText('Tout verrouiller');
  await page.locator('#tlLockAll').click();
  await expect.poll(async () => (await serverShow()).tracks.map(t => t.locked)).toEqual([true, true]);
  await expect(page.locator('#tlLockAll')).toHaveText('Tout déverrouiller');
  await page.locator('#tlCanvas').focus();
  await page.keyboard.press('ControlOrMeta+z');
  await expect.poll(async () => (await serverShow()).tracks.map(t => t.locked)).toEqual([false, false]);
  await expect(page.locator('#tlLockAll')).toHaveText('Tout verrouiller');
  // Maj+Alt+L: all again.
  await page.keyboard.press('Shift+Alt+KeyL');
  await expect.poll(async () => (await serverShow()).tracks.map(t => t.locked)).toEqual([true, true]);

  // Save, open another show, open this one again: still locked.
  await page.locator('#tlSave').click();
  await expect.poll(async () => (await timeline()).state.modified).toBe(false);
  expect(await studio.post('/api/timeline/new', { name: 'Autre e2e', time_base: 'seconds' })).toBe(200);
  expect(await studio.post('/api/timeline/load', { name: 'Tout e2e' })).toBe(200);
  await page.reload();
  await openEditor(page, 'Tout e2e');
  await expect(lockButton(page, 0)).toHaveClass(/active/);
  await expect(lockButton(page, 1)).toHaveClass(/active/);
  expect((await serverShow()).tracks.map(t => t.locked)).toEqual([true, true]);
});
