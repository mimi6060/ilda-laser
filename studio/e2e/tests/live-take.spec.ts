// Enregistrer le jeu en direct dans la timeline (T-169): ● Enregistrer
// waits for the playback, cues played and a master moved become tracks
// « Prise n » kept as one undoable edit (locked tracks untouched); the
// emergency stop ends a recording and keeps it; recording is allowed in
// show mode, keeping it is not. Preview only: nothing here arms the laser.
import AxeBuilder from '@axe-core/playwright';
import { test, expect, useStudio, openUi, openWorkspace, type Page } from '../studio';

const studio = useStudio();
test.describe.configure({ mode: 'serial' });

interface Track { name: string; kind: string; layer: number; locked?: boolean; events: { start: number; source: { id: string } }[] }
interface Take { state: string; events: number; take: null | { events: number; envelopes: string[]; reason: string } }
const serverShow = async () => (await studio.get<{ show: { name: string; tracks: Track[] } }>('/api/timeline')).show;
const take = async () => studio.get<Take>('/api/timeline/record');
let cues: string[] = [];

test.beforeAll(async () => {
  cues = (await studio.presets()).presets.map(p => p.id);
});

test.beforeEach(async ({ page }) => {
  await studio.post('/api/timeline/record/stop');
  await studio.post('/api/timeline/record/discard');
  await studio.post('/api/timeline/stop');
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  await openUi(page, studio);
});

test.afterEach(async () => {
  expect((await studio.state()).armed, 'recording never arms').toBe(false);
  await studio.post('/api/lock', { show_mode: false });
  await studio.post('/api/timeline/record/stop');
  await studio.post('/api/timeline/record/discard');
  await studio.post('/api/timeline/stop');
  await studio.post('/api/running/stop', {});
});

/** A Secondes show with one long locked event on « Piste 1 », so it plays on. */
async function seed(name: string) {
  expect(await studio.post('/api/timeline/new', { name, time_base: 'seconds' })).toBe(200);
  const show = await serverShow();
  show.tracks[0].events = [{ id: 1, start: 0, len: 60, source: { kind: 'cue', id: cues[0] } } as any];
  show.tracks[0].locked = true;
  expect(await studio.post('/api/timeline/edit', { show })).toBe(200);
}

async function openEditor(page: Page, name: string) {
  await openWorkspace(page, 'timeline');
  await expect.poll(() => page.evaluate(() => (window as any).tlDebug.show()?.name)).toBe(name);
}

test('record cues and a master move, keep the take as tracks « Prise 1 », undo it', async ({ page }) => {
  await seed('Prise e2e');
  await openEditor(page, 'Prise e2e');
  const rec = page.locator('#tlTakeRec');
  await expect(rec).toHaveText('● Enregistrer');
  await rec.click();
  await expect(rec).toHaveAttribute('aria-pressed', 'true');
  await expect(rec).toContainText('En attente de la lecture');
  expect((await take()).state).toBe('waiting');
  expect(await studio.post('/api/timeline/play')).toBe(200);
  await expect.poll(async () => (await take()).state).toBe('recording');
  await expect(rec).toContainText('Arrêter l\'enregistrement');
  // Played by hand: a cue on top of the show, then a master move.
  expect(await studio.post('/api/cue', { id: cues[1], mode: 'restart' })).toBe(200);
  await expect.poll(async () => (await take()).events).toBe(1);
  expect(await studio.post('/api/control', { id: 'master.size', value: 0.6 })).toBe(200);
  await page.waitForTimeout(300);
  await rec.click();
  await expect.poll(async () => (await take()).state).toBe('idle');
  const t = (await take()).take!;
  expect(t.events).toBe(1);
  expect(t.envelopes).toEqual(['master.size']);
  expect(t.reason).toBe('user');
  await expect(page.locator('#tlTakeRow')).toBeVisible();
  await expect(page.locator('#tlTakeInfo')).toContainText('1 cue(s)');
  await expect(rec).toHaveText('● Enregistrer');
  const axe = await new AxeBuilder({ page }).include('#tlTakeRow').include('#tlTakeRec').withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
  expect(axe.violations.filter(v => v.impact === 'serious' || v.impact === 'critical').map(v => v.id), 'axe on the take row').toEqual([]);

  // Keep it, quantised to the beat: new tracks only, the locked one untouched.
  const before = await serverShow();
  await page.locator('#tlTakeQ').selectOption('beat');
  await page.locator('#tlTakeKeep').click();
  await expect.poll(async () => (await serverShow()).tracks.length).toBeGreaterThan(before.tracks.length);
  const after = await serverShow();
  expect(after.tracks[0]).toEqual(before.tracks[0]);
  const added = after.tracks.slice(before.tracks.length);
  expect(added.map(x => x.name)).toEqual(['Prise 1 · C1', 'Prise 1 · réglages C1']);
  expect(added[0].events.map(e => e.source.id)).toEqual([cues[1]]);
  expect(Number.isInteger(added[0].events[0].start * 2), 'on a beat (0.5 s at 120 BPM)').toBe(true);
  expect(added[1].kind).toBe('bus');
  await expect(page.locator('#tlTakeRow')).toBeHidden();
  expect((await take()).take).toBeNull();
  await expect(page.locator('#tlEdMsg')).toContainText('Prise 1 gardée');

  // One edit: Annuler takes the whole take back.
  await page.locator('#tlUndo').click();
  await expect.poll(async () => (await serverShow()).tracks.length).toBe(before.tracks.length);
});

test('the emergency stop ends a recording and keeps what it captured', async ({ page }) => {
  await seed('Prise arret');
  await openEditor(page, 'Prise arret');
  expect(await studio.post('/api/timeline/play')).toBe(200);
  await page.locator('#tlTakeRec').click();
  await expect.poll(async () => (await take()).state).toBe('recording');
  expect(await studio.post('/api/cue', { id: cues[2], mode: 'restart' })).toBe(200);
  await expect.poll(async () => (await take()).events).toBe(1);
  expect(await studio.post('/api/estop')).toBe(200);
  await expect.poll(async () => (await take()).state).toBe('idle');
  const t = (await take()).take!;
  expect(t.reason).toBe('e_stop');
  expect(t.events).toBe(1);
  await expect(page.locator('#tlTakeInfo')).toContainText('arrêt d\'urgence');
  // A second recording waits for the take to be kept or dropped.
  expect(await studio.post('/api/estop/reset', {})).toBe(200);
  expect(await studio.post('/api/timeline/record/start')).toBe(409);
  page.once('dialog', d => d.accept());
  await page.locator('#tlTakeDrop').click();
  await expect(page.locator('#tlTakeRow')).toBeHidden();
  expect(await studio.post('/api/timeline/record/start')).toBe(200);
});

test('show mode: recording is allowed, keeping the take is not', async ({ page }) => {
  await seed('Prise spectacle');
  await openEditor(page, 'Prise spectacle');
  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  expect(await studio.post('/api/timeline/play')).toBe(200);
  await page.locator('#tlTakeRec').click();
  await expect.poll(async () => (await take()).state).toBe('recording');
  expect(await studio.post('/api/cue', { id: cues[1], mode: 'restart' })).toBe(200);
  await expect.poll(async () => (await take()).events).toBe(1);
  await page.locator('#tlTakeRec').click();
  await expect.poll(async () => (await take()).take?.events).toBe(1);
  expect(await studio.post('/api/timeline/record/take', {})).toBe(423);
  const tracks = (await serverShow()).tracks.length;
  expect(await studio.post('/api/lock', { show_mode: false })).toBe(200);
  expect(await studio.post('/api/timeline/record/take', {})).toBe(200);
  expect((await serverShow()).tracks.length, 'the layout alone changes nothing').toBe(tracks);
});
