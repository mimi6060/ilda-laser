// Timeline editor (T-162): new show, drag a cue from the library onto a
// track, move / resize with Magnétisme, markers (Entrée while the timeline
// has the focus), loop region, zoom, copy / paste of phrases in whole bars,
// undo / redo, save and reload, editing while the show plays, and 500
// events drawn fast enough. Preview only: nothing here arms the laser.
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { test, expect, useStudio, openUi, openWorkspace, isLit, type Page, type Point } from '../studio';

const studio = useStudio();

interface Ev { id: number; start: number; len: number; source: { kind: string; id: string } }
interface Show { name: string; time_base: string; tracks: { name: string; layer: number; events: Ev[] }[]; markers: { at: number; name: string }[]; loop_region: [number, number] | null }
interface View { t0: number; pps: number; top: number; lane: number; ruler: number; mark: number }
const timeline = () => studio.get<{ state: any; show: Show | null }>('/api/timeline');
const serverShow = async () => (await timeline()).show!;
const events = async (track = 0) => (await serverShow()).tracks[track].events;
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

/** A new show through the API, optionally with events on track 1 (seconds, 120 BPM 4/4: 1 bar = 2 s). */
async function seed(name: string, evs: Partial<Ev>[] = [], extra: Partial<Show> = {}) {
  expect(await studio.post('/api/timeline/new', { name, time_base: 'seconds' })).toBe(200);
  const show = await serverShow();
  show.tracks[0].events = evs.map((e, i) => ({ id: i + 1, start: 0, len: 2, source: { kind: 'cue', id: cues[0] }, ...e })) as Ev[];
  Object.assign(show, extra);
  expect(await studio.post('/api/timeline/edit', { show })).toBe(200);
}

async function openEditor(page: Page, name: string) {
  await openWorkspace(page, 'timeline');
  await expect.poll(() => page.evaluate(() => (window as any).tlDebug.show()?.name)).toBe(name);
}
const view = (page: Page) => page.evaluate(() => (window as any).tlDebug.view()) as Promise<View>;
const localShow = (page: Page) => page.evaluate(() => (window as any).tlDebug.show()) as Promise<Show>;
async function setView(page: Page, t0: number, pps: number) {
  await page.evaluate(([a, b]) => (window as any).tlDebug.setView(a, b), [t0, pps]);
}
/** Page coordinates of show time `u` in track lane `lane` (or in the ruler / marker lane). */
async function at(page: Page, u: number, where: number | 'ruler' | 'marker') {
  const v = await view(page);
  const box = (await page.locator('#tlCanvas').boundingBox())!;
  const y = where === 'ruler' ? v.ruler / 2 : where === 'marker' ? v.ruler + v.mark / 2 : v.top + where * v.lane + v.lane / 2;
  return { x: box.x + (u - v.t0) * v.pps, y: box.y + y };
}
async function drag(page: Page, from: { x: number; y: number }, to: { x: number; y: number }) {
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move((from.x + to.x) / 2, (from.y + to.y) / 2, { steps: 4 });
  await page.mouse.move(to.x, to.y, { steps: 4 });
  await page.mouse.up();
}

test('new show, drag a cue from the library onto track 1 at bar 5', async ({ page }) => {
  await openWorkspace(page, 'timeline');
  await page.locator('#tlNew').click();
  await page.locator('#tlNewName').fill('Glisser e2e');
  await page.locator('#tlNewBase').selectOption('seconds');
  await page.locator('#tlNewOk').click();
  await expect.poll(async () => (await timeline()).state.name).toBe('Glisser e2e');
  await openEditor(page, 'Glisser e2e');
  await expect(page.locator('#tlHeads .tl-head')).toHaveCount(2);
  await expect(page.locator('#tlLibList .tl-lib-item')).not.toHaveCount(0);

  await setView(page, 0, 40); // 1 beat = 0.5 s = 20 px: the strong magnet takes beats
  const item = page.locator('#tlLibList .tl-lib-item').first();
  const id = await item.getAttribute('data-id');
  const v = await view(page);
  // Bar 5 starts at 8 s; drop it 5 px late: the magnet puts it exactly on the bar.
  await item.dragTo(page.locator('#tlCanvas'), { targetPosition: { x: (8 - v.t0) * v.pps + 5, y: v.top + v.lane / 2 } });
  await expect.poll(async () => (await events()).length).toBe(1);
  const [e] = await events();
  expect(e.start).toBe(8);
  expect(e.source).toEqual({ kind: 'cue', id });
  expect(e.len).toBeGreaterThan(0);
  expect((await timeline()).state.modified).toBe(true);
  await expect(page.locator('#tlModified')).toContainText('modifié');
  // Nothing written until Enregistrer.
  const file = JSON.parse(readFileSync(path.join(studio.dataDir, 'shows', 'Glisser e2e.json'), 'utf8'));
  expect(file.tracks[0].events).toEqual([]);
  await page.screenshot({ path: 'test-results/timeline-editor.png' });
});

test('a Temps show counts in beats of the live tempo: bar 3 = beat 8', async ({ page }) => {
  expect(await studio.post('/api/timeline/new', { name: 'Temps e2e', time_base: 'beats' })).toBe(200);
  await openEditor(page, 'Temps e2e');
  await expect(page.locator('#tlBpm')).toBeDisabled();
  await setView(page, 0, 40); // 40 px per beat
  const v = await view(page);
  await page.locator('#tlLibList .tl-lib-item').first().dragTo(page.locator('#tlCanvas'), { targetPosition: { x: 8 * v.pps + 6, y: v.top + v.lane * 1.5 } });
  await expect.poll(async () => (await events(1)).map(e => [e.start, e.len])).toEqual([[8, 16]]);
});

test('move and resize snap to beats, markers and nothing (Off)', async ({ page }) => {
  await seed('Aimant e2e', [{ start: 8, len: 8 }], { markers: [{ at: 13.3, name: 'Voix', color: [255, 0, 0] }] as any });
  await openEditor(page, 'Aimant e2e');
  await setView(page, 0, 40);
  await page.locator('#tlSnap [data-snap="strong"]').click();

  // Move by 2 bars + 7 px, one track down: exactly 12 s, on track 2.
  const from = await at(page, 12, 0), to = await at(page, 16.175, 1);
  await drag(page, from, to);
  await expect.poll(async () => (await events(1)).map(e => e.start)).toEqual([12]);
  expect(await events(0)).toEqual([]);

  // Resize the end (20 s) by 33 px (0.825 s): it lands on the beat, 21 s.
  await drag(page, { ...(await at(page, 20, 1)), x: (await at(page, 20, 1)).x - 2 }, { ...(await at(page, 20.825, 1)), x: (await at(page, 20.825, 1)).x - 2 });
  await expect.poll(async () => (await events(1))[0].len).toBe(9);

  // Moyen: the start is pulled onto the marker when within a few pixels.
  await page.locator('#tlSnap [data-snap="medium"]').click();
  await drag(page, await at(page, 14, 1), await at(page, 14 + 1.3 + 0.1, 1));
  await expect.poll(async () => (await events(1))[0].start).toBe(13.3);

  // Off: wherever the mouse says.
  await page.locator('#tlSnap [data-snap="off"]').click();
  await drag(page, await at(page, 15, 1), await at(page, 15.3, 1));
  await expect.poll(async () => Math.abs((await events(1))[0].start - 13.6)).toBeLessThan(0.05);
  const s = (await events(1))[0].start;
  expect(Math.abs(s * 2 - Math.round(s * 2))).toBeGreaterThan(0.01);
  await page.locator('#tlSnap [data-snap="strong"]').click();
});

test('copy 8 bars and paste them at the next bar after the playhead: whole bars', async ({ page }) => {
  await seed('Phrase e2e', Array.from({ length: 8 }, (_, i) => ({ start: 2 * i, len: 1.5, source: { kind: 'cue', id: cues[i % cues.length] } })));
  await openEditor(page, 'Phrase e2e');
  await page.locator('#tlFit').click();
  // Select all with the timeline focused, then Copier la phrase.
  const lane = await at(page, 17, 1);
  await page.mouse.click(lane.x, lane.y); // empty lane: focus, no selection
  await page.keyboard.press('ControlOrMeta+a');
  await page.locator('#tlCopy').click();
  await expect(page.locator('#tlEdMsg')).toContainText('8 événement(s), 8 mesure(s)');

  // Playhead at 20.3 s = bar 11.15 (0-based 10.15): pasted from bar 12 (22 s).
  expect(await studio.post('/api/timeline/seek', { position: 20.3 })).toBe(200);
  await expect.poll(() => page.evaluate(() => (window as any).tlDebug && document.querySelector('#tlPos')!.textContent)).toContain('mesure 11');
  await page.locator('#tlPaste').click();
  await expect(page.locator('#tlEdMsg')).toContainText('Collé à la mesure 12');
  await expect.poll(async () => (await events()).length).toBe(16);
  const evs = await events();
  const orig = evs.filter(e => e.start < 16), pasted = evs.filter(e => e.start >= 16);
  expect(pasted.map(e => e.start)).toEqual(orig.map(e => e.start + 22));
  for (let i = 0; i < 8; i++) {
    const shiftBars = (pasted[i].start - orig[i].start) / 2;
    expect(shiftBars).toBe(Math.round(shiftBars));
    expect(pasted[i].len).toBeCloseTo(1.5, 9);
    expect(pasted[i].source).toEqual(orig[i].source);
  }
  expect(new Set(evs.map(e => e.id)).size).toBe(16);

  // Annuler / Rétablir (keyboard and buttons).
  await page.keyboard.press('ControlOrMeta+z');
  await expect.poll(async () => (await events()).length).toBe(8);
  await page.keyboard.press('ControlOrMeta+Shift+z');
  await expect.poll(async () => (await events()).length).toBe(16);
  await page.locator('#tlUndo').click();
  await expect.poll(async () => (await events()).length).toBe(8);
  expect((await localShow(page)).tracks[0].events.length).toBe(8);
  await page.locator('#tlRedo').click();
  await expect.poll(async () => (await events()).length).toBe(16);

  // Dupliquer (Cmd+D): the selection again right after itself, in whole
  // bars (0 → 18.75 bars: 19 bars later).
  await page.locator('#tlCanvas').focus();
  await page.keyboard.press('ControlOrMeta+a');
  await page.keyboard.press('ControlOrMeta+d');
  await expect.poll(async () => (await events()).length).toBe(32);
  expect(Math.max(...(await events()).map(e => e.start))).toBe(22 + 14 + 38);
  // Delete, then undo it.
  await page.keyboard.press('Delete');
  await expect.poll(async () => (await events()).length).toBe(16);
  await page.keyboard.press('ControlOrMeta+z');
  await expect.poll(async () => (await events()).length).toBe(32);
});

test('markers: Entrée with the timeline focused, and a moved marker takes its events along', async ({ page }) => {
  await seed('Marqueurs e2e', [{ start: 4, len: 2 }, { start: 5, len: 2 }], { markers: [{ at: 4, name: 'Couplet', color: [0, 255, 0] }] as any });
  await openEditor(page, 'Marqueurs e2e');
  await setView(page, 0, 40);
  const bpm = (await studio.frame()).tempo.bpm;

  // Entrée elsewhere: tap tempo as before, no marker.
  await page.locator('h1').click();
  await page.keyboard.press('Enter');
  await page.waitForTimeout(150);
  expect((await serverShow()).markers.length).toBe(1);

  // Entrée on the focused timeline: a marker at the playhead.
  expect(await studio.post('/api/timeline/seek', { position: 9 })).toBe(200);
  await expect.poll(() => page.evaluate(() => document.querySelector('#tlPos')!.textContent)).toContain('0:09.0');
  await page.locator('#tlCanvas').focus();
  await page.keyboard.press('Enter');
  await expect.poll(async () => (await serverShow()).markers.map(m => m.at)).toEqual([4, 9]);
  expect((await studio.frame()).tempo.bpm).toBe(bpm);

  // Drag the marker at 4 s to 6 s: the event that starts on it follows, the other doesn't.
  const m = await at(page, 4, 'marker');
  const to = await at(page, 6, 'marker');
  await drag(page, { x: m.x + 4, y: m.y }, { x: to.x + 4, y: to.y });
  await expect.poll(async () => (await serverShow()).markers.map(x => x.at)).toEqual([6, 9]);
  expect((await events()).map(e => e.start).sort((a, b) => a - b)).toEqual([5, 6]);
  // Name it.
  await page.locator('#tlMarkName').fill('Refrain');
  await page.locator('#tlMarkName').press('Tab');
  await expect.poll(async () => (await serverShow()).markers[0].name).toBe('Refrain');

  // Ajouter un marqueur (button) at the playhead too.
  await page.locator('#tlAddMarker').click();
  await expect.poll(async () => (await serverShow()).markers.length).toBe(3);
});

test('loop region from the ruler, wheel zoom around the cursor, Tout afficher', async ({ page }) => {
  await seed('Boucle e2e', [{ start: 0, len: 40 }]);
  await openEditor(page, 'Boucle e2e');
  await setView(page, 0, 40);
  await drag(page, await at(page, 4.1, 'ruler'), await at(page, 7.9, 'ruler'));
  await expect.poll(async () => (await timeline()).state.loop_region).toEqual([4, 8]);
  // A click in the ruler seeks.
  const c = await at(page, 10, 'ruler');
  await page.mouse.click(c.x, c.y);
  await expect.poll(async () => (await timeline()).state.position).toBeCloseTo(10, 1);
  // Double-click: no more region.
  await page.mouse.dblclick(c.x, c.y);
  await expect.poll(async () => (await timeline()).state.loop_region).toBeNull();

  const p = await at(page, 10, 0);
  await page.mouse.move(p.x, p.y);
  await page.mouse.wheel(0, -400);
  await expect.poll(async () => (await view(page)).pps).toBeGreaterThan(60);
  const v = await view(page);
  const box = (await page.locator('#tlCanvas').boundingBox())!;
  expect(v.t0 + (p.x - box.x) / v.pps).toBeCloseTo(10, 3);
  await page.locator('#tlFit').click();
  const all = await view(page);
  expect(all.t0).toBe(0);
  expect(40 * all.pps).toBeLessThanOrEqual(box.width);
});

test('save, reload the page and open the show again', async ({ page }) => {
  await seed('Sauver e2e', [{ start: 2, len: 4 }], { markers: [{ at: 2, name: 'Début', color: [1, 2, 3] }] as any });
  await openEditor(page, 'Sauver e2e');
  await expect(page.locator('#tlModified')).toContainText('modifié');
  await page.locator('#tlSave').click();
  await expect(page.locator('#tlEdMsg')).toContainText('enregistré');
  await expect.poll(async () => (await timeline()).state.modified).toBe(false);
  await expect(page.locator('#tlModified')).toHaveText('');
  const file = JSON.parse(readFileSync(path.join(studio.dataDir, 'shows', 'Sauver e2e.json'), 'utf8'));
  expect(file.tracks[0].events.map((e: Ev) => [e.start, e.len])).toEqual([[2, 4]]);
  expect(file.markers[0].name).toBe('Début');

  // Something else loaded, then reload the page and open it from the list.
  await seed('Autre e2e');
  await page.reload();
  await openWorkspace(page, 'timeline');
  await expect.poll(() => page.evaluate(() => (window as any).tlDebug.show()?.name)).toBe('Autre e2e');
  // « Autre e2e » has unsaved edits: the page asks before dropping them.
  page.once('dialog', d => d.accept());
  await page.locator('#tlShow').focus();
  await page.locator('#tlShow').selectOption('Sauver e2e');
  await expect.poll(() => page.evaluate(() => (window as any).tlDebug.show()?.tracks[0].events.length)).toBe(1);
  expect((await localShow(page)).markers[0].name).toBe('Début');
  await expect(page.locator('#tlUndo')).toBeDisabled();
});

test('editing while the show plays keeps it playing, in range and disarmed', async ({ page }) => {
  await seed('Direct e2e', [{ start: 0, len: 30 }]);
  await openEditor(page, 'Direct e2e');
  await setView(page, 0, 40);
  await page.locator('#tlPlay').click();
  await expect.poll(async () => (await timeline()).state.active).toEqual([1]);
  // Resize while it plays, then move it.
  await page.locator('#tlFollow').uncheck();
  await setView(page, 0, 20);
  await drag(page, { ...(await at(page, 30, 0)), x: (await at(page, 30, 0)).x - 2 }, { ...(await at(page, 34, 0)), x: (await at(page, 34, 0)).x - 2 });
  await expect.poll(async () => (await events())[0].len).toBe(34);
  const st = (await timeline()).state;
  expect(st.playing).toBe(true);
  expect(st.active).toEqual([1]);
  for (let i = 0; i < 5; i++) {
    const pts = (await studio.get<{ points: Point[] }>('/api/frame')).points;
    expect(pts.filter(isLit).length).toBeGreaterThan(0);
    for (const p of pts) { expect(Math.abs(p[0])).toBeLessThanOrEqual(1); expect(Math.abs(p[1])).toBeLessThanOrEqual(1); }
  }
  // An unknown cue sent by hand is refused, the show keeps playing.
  const bad = await serverShow();
  bad.tracks[0].events[0].source.id = 'nope';
  expect(await studio.post('/api/timeline/edit', { show: bad })).toBe(400);
  expect((await timeline()).state.playing).toBe(true);
  await studio.post('/api/timeline/stop');
});

test('500 events stay fluid (draw well under 33 ms)', async ({ page }) => {
  const many = Array.from({ length: 500 }, (_, i) => ({ start: i * 0.5, len: 0.45, source: { kind: 'cue', id: cues[i % cues.length] } }));
  expect(await studio.post('/api/timeline/new', { name: 'Charge e2e' })).toBe(200);
  const show = await serverShow();
  show.tracks = [0, 1, 2, 3].map(t => ({ name: `P${t + 1}`, layer: t + 1, events: many.filter((_, i) => i % 4 === t).map((e, i) => ({ ...e, id: t * 1000 + i + 1 })) })) as any;
  expect(await studio.post('/api/timeline/edit', { show })).toBe(200);
  await openEditor(page, 'Charge e2e');
  await page.locator('#tlFit').click();
  await page.locator('#tlPlay').click();
  await expect.poll(async () => (await timeline()).state.playing).toBe(true);
  const d0 = (await page.evaluate(() => (window as any).tlDebug.stats())).draws;
  await page.waitForTimeout(1000);
  const s = await page.evaluate(() => (window as any).tlDebug.stats());
  expect(s.draws - d0).toBeGreaterThanOrEqual(30);
  expect(s.drawMs).toBeLessThan(33);
  await studio.post('/api/timeline/stop');
});
