// Transitions between cues (T-158) and quantised launch (T-159), from the
// grid bar: « Transition » / « Durée », « Quantif. : Off / Temps / Mesure »,
// « Mode beat », a cell's own transition in « Propriétés du cue », a cell
// « en attente » that blinks, and Escape cutting everything at once.
// Preview only: the laser is never armed here.
import { test, expect, useStudio, openUi, openCueMore, reveal, focusPage, isLit } from '../studio';

const studio = useStudio();
let catalog: Awaited<ReturnType<typeof studio.presets>>;
type Deck = { active: { cue: string; phase: { phase: string } }[]; ending: { cue: string }[]; pending: { cue: string; at_beat: number }[]; transition: { kind: string; length: Record<string, number> }; quantize: string; beat_every: number };
const deck = async () => (await studio.get<{ cues: Deck }>('/api/frame')).cues;
const cell = (page: import('@playwright/test').Page, id: string) => page.locator(`#cues .cue[data-id="${id}"]`);
const firstCues = () => catalog.presets.filter(p => p.category === catalog.categories[0]);

test.beforeAll(async () => { catalog = await studio.presets(); });

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await studio.post('/api/estop/reset', {});
  await studio.post('/api/grid', { cols: 8, rows: 5, pages: [] });
  await studio.post('/api/control', { id: 'page.1' });
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.post('/api/control', { id: 'cue.transition', value: 0 });
  await studio.post('/api/control', { id: 'cue.transition_length', value: 2 });
  await studio.post('/api/control', { id: 'cue.quantize', value: 0 });
  await studio.post('/api/control', { id: 'cue.beat_mode', value: 0 });
  await studio.post('/api/control', { id: 'tempo.bpm', value: 120 });
  await openUi(page, studio);
  await reveal(page, '#cues');
});

test('the grid bar sets the transition, its length and the quantisation', async ({ page }) => {
  await page.locator('#cueTransition').selectOption({ label: 'Fondu au noir' });
  await page.locator('#cueTransLen').selectOption({ label: '2 s' });
  await page.locator('#cueQuant button', { hasText: 'Mesure' }).click();
  await openCueMore(page);
  await page.locator('#cueBeatMode').selectOption({ label: '4 temps' });
  await expect.poll(async () => (await deck()).transition).toEqual({ kind: 'fade_through_black', length: { seconds: 2 }, in_flash: false });
  const d = await deck();
  expect([d.quantize, d.beat_every]).toEqual(['bar', 4]);
  await expect(page.locator('#cueQuant button', { hasText: 'Mesure' })).toHaveClass(/active/);
  // Saved with the grid.
  const saved = await studio.get<{ transition: { kind: string }; quantize: string; beat_every: number }>('/api/cues');
  expect([saved.transition.kind, saved.quantize, saved.beat_every]).toEqual(['fade_through_black', 'bar', 4]);
});

test('with a fade, the replaced cue keeps drawing while it fades out', async ({ page }) => {
  const [a, b] = firstCues();
  await page.locator('#cueTransition').selectOption({ label: 'Fondu' });
  await page.locator('#cueTransLen').selectOption({ label: '4 s' });
  await expect.poll(async () => (await deck()).transition.kind).toBe('fade');
  await cell(page, a.id).click();
  await expect.poll(async () => (await deck()).active.map(c => c.cue)).toEqual([a.id]);
  await cell(page, b.id).click();
  await expect.poll(async () => (await deck()).active.map(c => c.cue)).toEqual([b.id]);
  const d = await deck();
  expect(d.ending.map(e => e.cue)).toEqual([a.id]);
  expect(d.active[0].phase.phase).toBe('starting');
  await expect(cell(page, a.id)).toHaveClass(/ending/);
  await expect(cell(page, b.id)).toHaveClass(/active/);
  expect((await studio.frame()).points.some(isLit)).toBe(true);
  // « Tout arrêter » cuts at once, fades included.
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await expect.poll(async () => (await deck()).ending.length).toBe(0);
});

test('a cue launched with « Mesure » waits, blinking, then starts on the bar', async ({ page }) => {
  const [a] = firstCues();
  await studio.post('/api/control', { id: 'tempo.bpm', value: 40 }); // a bar = 6 s
  await page.locator('#cueQuant button', { hasText: 'Mesure' }).click();
  await expect.poll(async () => (await deck()).quantize).toBe('bar');
  // Just after a bar's start (within ⅛ beat) it would start at once: wait for mid-bar.
  await expect.poll(async () => (await studio.frame()).tempo.beat_in_bar, { timeout: 10_000 }).toBe(1);
  await cell(page, a.id).click();
  await expect.poll(async () => (await deck()).pending.map(p => p.cue)).toEqual([a.id]);
  await expect(cell(page, a.id)).toHaveClass(/pending/);
  const d = await deck();
  expect(d.active).toEqual([]);
  expect(d.pending[0].at_beat % 4).toBe(0);
  // A second press takes it back.
  await cell(page, a.id).click();
  await expect.poll(async () => (await deck()).pending.length).toBe(0);
  await expect(cell(page, a.id)).not.toHaveClass(/pending/);
  // Pressed again, then « Off »: what waited starts now.
  await cell(page, a.id).click();
  await expect.poll(async () => (await deck()).pending.length).toBe(1);
  await page.locator('#cueQuant button', { hasText: 'Off' }).click();
  await expect.poll(async () => (await deck()).active.map(c => c.cue)).toEqual([a.id]);
  await expect(cell(page, a.id)).toHaveClass(/active/);
});

test('Escape cuts every fade and drops waiting cues', async ({ page }) => {
  const [a, b, c] = firstCues();
  await studio.post('/api/control', { id: 'tempo.bpm', value: 40 });
  await page.locator('#cueTransition').selectOption({ label: 'Fondu' });
  await page.locator('#cueTransLen').selectOption({ label: '4 s' });
  await cell(page, a.id).click();
  await cell(page, b.id).click();
  await expect.poll(async () => (await deck()).ending.length).toBe(1);
  await page.locator('#cueQuant button', { hasText: 'Mesure' }).click();
  await expect.poll(async () => (await deck()).quantize).toBe('bar');
  await expect.poll(async () => (await studio.frame()).tempo.beat_in_bar, { timeout: 10_000 }).toBe(1);
  await cell(page, c.id).click();
  await expect.poll(async () => (await deck()).pending.length).toBe(1);
  await focusPage(page);
  await page.keyboard.press('Escape');
  await expect.poll(async () => { const d = await deck(); return [d.ending.length, d.pending.length]; }).toEqual([0, 0]);
  await studio.post('/api/estop/reset', {});
});

test('a cue can have its own transition', async ({ page }) => {
  const [a] = firstCues();
  await cell(page, a.id).click({ button: 'right' });
  await expect(page.locator('#cueMenu')).toBeVisible();
  await expect(page.locator('#cueMenuTransLen')).toBeDisabled();
  await page.locator('#cueMenuTransition').selectOption({ label: 'Morph' });
  await expect(page.locator('#cueMenuTransLen')).toBeEnabled();
  await page.locator('#cueMenuTransLen').selectOption({ label: '½ temps' });
  await expect.poll(async () => (await studio.get<{ slots: Record<string, { transition?: unknown }> }>('/api/cues')).slots[a.id]?.transition)
    .toEqual({ kind: 'morph', length: { beats: 0.5 }, in_flash: false });
  await page.locator('#cueMenuClose').click();
  await expect(cell(page, a.id).locator('.tag')).toContainText('T');
  // « Comme la grille » removes it.
  await cell(page, a.id).click({ button: 'right' });
  await page.locator('#cueMenuTransition').selectOption({ label: 'Comme la grille' });
  await expect.poll(async () => (await studio.get<{ slots: Record<string, { transition?: unknown }> }>('/api/cues')).slots[a.id]?.transition).toBeUndefined();
});
