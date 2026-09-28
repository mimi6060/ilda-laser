// Figure editor (T-296): CRÉATION › Figures. Draw on the canvas, save to
// the library, reopen identical, play it as a cue (page Figures) through
// the layers and the normal output path, animate several images on the
// tempo, undo / redo. Preview only: the laser stays disarmed.
import { test, expect, useStudio, openUi, openWorkspace, isLit, extent, type Point } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();

interface Stroke { points: [number, number][]; color: [number, number, number]; lit: boolean }
interface Figure { name: string; frames: { strokes: Stroke[] }[]; rate: number; per: string; loop_mode: string }
const load = async (name: string) => {
  const r = await fetch(studio.url + '/api/figures/load', { method: 'POST', body: JSON.stringify({ name }) });
  return r.ok ? (await r.json()) as Figure : null;
};
const lit = async () => (await studio.frame()).points.filter(isLit);

/** Page coordinates of laser point (x, y) on the editor canvas. */
async function xy(page: Page, x: number, y: number): Promise<[number, number]> {
  const b = (await page.locator('#figCanvas').boundingBox())!;
  return [b.x + (x + 1) / 2 * b.width, b.y + (1 - y) / 2 * b.height];
}
async function click(page: Page, x: number, y: number) { await page.mouse.click(...await xy(page, x, y)); }
async function drag(page: Page, from: [number, number], to: [number, number]) {
  await page.mouse.move(...await xy(page, ...from));
  await page.mouse.down();
  await page.mouse.move(...await xy(page, ...to), { steps: 5 });
  await page.mouse.up();
}
const tool = (page: Page, t: string) => page.locator(`#figTools [data-tool="${t}"]`).click();

async function openEditor(page: Page) {
  await openUi(page, studio);
  await openWorkspace(page, 'creation');
  await page.locator('#creationTabs [data-ctab="figures"]').click();
  await expect(page.locator('#figEditor')).toBeVisible();
  await expect(page.locator('#figProps')).toBeVisible();
  // The look panel is on the other sub-tab.
  await expect(page.locator('#shapes')).toBeHidden();
}

test.beforeEach(async () => {
  await studio.reset();
  await studio.post('/api/control', { id: 'cue.stop_all' });
});

test('draw, save, reopen identical, play as a cue: a non-empty frame, laser still off', async ({ page }) => {
  await openEditor(page);
  // A red rectangle, then a green polyline (double-click ends it).
  await page.locator('#figColor').fill('#ff0000');
  await tool(page, 'rect');
  await drag(page, [-0.5, -0.5], [0.5, 0.5]);
  await page.locator('#figColor').fill('#00ff00');
  await tool(page, 'line');
  await click(page, -0.8, 0.8);
  await click(page, 0, 0.9);
  await page.mouse.dblclick(...await xy(page, 0.8, 0.8));
  // Then an explicit blanked move.
  await tool(page, 'move');
  await click(page, 0.8, 0.8);
  await page.mouse.dblclick(...await xy(page, 0.8, -0.8));
  await expect(page.locator('#figStats')).toContainText('3 tracé(s)');

  // Names are file names: refused before anything is sent.
  await page.locator('#figName').fill('../evil');
  await page.locator('#figSave').click();
  await expect(page.locator('#figMsg')).toHaveClass(/err/);
  expect(await studio.post('/api/figures', { name: '../evil' })).toBe(400);

  await page.locator('#figName').fill('Ma figure');
  await page.locator('#figSave').click();
  await expect(page.locator('#figMsg')).toContainText('enregistrée');
  const saved = (await load('Ma figure'))!;
  const [rect, line, move] = saved.frames[0].strokes;
  expect(rect.color).toEqual([255, 0, 0]);
  expect(rect.lit).toBe(true);
  expect(rect.points.length).toBe(5);
  for (const [x, y] of rect.points) expect(Math.max(Math.abs(Math.abs(x) - 0.5), Math.abs(Math.abs(y) - 0.5))).toBeLessThan(0.02);
  expect(line.color).toEqual([0, 255, 0]);
  expect(line.points.length).toBe(3);
  expect(move.lit).toBe(false);
  await expect(page.locator('#figLib [data-fig="Ma figure"]')).toBeVisible();

  // After a restart, the library still has it, identical; the editor reopens it.
  await studio.restart();
  expect(await load('Ma figure')).toEqual(saved);
  await openEditor(page);
  await page.locator('#figNew').click();
  await expect(page.locator('#figStats')).toContainText('0 tracé(s)');
  await page.locator('#figLib [data-fig="Ma figure"]').click();
  await expect(page.locator('#figName')).toHaveValue('Ma figure');
  await expect(page.locator('#figStats')).toContainText('3 tracé(s)');
  await page.locator('#figSave').click();
  await expect(page.locator('#figMsg')).toContainText('enregistrée');
  expect(await load('Ma figure')).toEqual(saved);

  // Jouer: its cue plays, the frame draws it (red and green, the rectangle's size).
  await page.locator('#figPlay').click();
  await expect.poll(async () => (await studio.frame()).active_cue).toBe('figure:Ma figure');
  await expect.poll(async () => (await lit()).length).toBeGreaterThan(50);
  const pts = await lit();
  expect(pts.some(p => p[2] > 0 && p[3] === 0)).toBe(true);
  expect(pts.some(p => p[3] > 0 && p[2] === 0)).toBe(true);
  expect(extent(pts)).toBeGreaterThan(0.45);
  // No lit point on the blanked move (x = 0.8, y from 0.8 to -0.8, away from the rest).
  expect(pts.some(p => Math.abs(p[0] - 0.8) < 0.01 && p[1] < 0.5 && p[1] > -0.5)).toBe(false);
  const f = await studio.frame() as unknown as { armed: boolean; output_lit: number };
  expect(f.armed).toBe(false);
  expect(f.output_lit).toBe(0);
  await expect(page.locator('#figNote')).toBeHidden(); // on the Look sub-tab only

  // Through the layers: muting layer 1 takes it off.
  expect(await studio.post('/api/control', { id: 'layer.1.mute', value: 1 })).toBe(200);
  await expect.poll(async () => (await lit()).length).toBe(0);
  expect(await studio.post('/api/control', { id: 'layer.1.mute', value: 0 })).toBe(200);
  await expect.poll(async () => (await lit()).length).toBeGreaterThan(50);

  // It is a cue of the Figures page in LIVE.
  await openWorkspace(page, 'live');
  await page.locator('#cueTabs button', { hasText: 'Figures (1)' }).click();
  await expect(page.locator('#cues .cue[data-id="figure:Ma figure"]')).toBeVisible();
  await expect(page.locator('#cues .cue[data-id="figure:Ma figure"]')).toHaveClass(/active/);
  // A second click stops it (Basculer).
  await page.locator('#cues .cue[data-id="figure:Ma figure"]').click();
  await expect.poll(async () => (await studio.frame()).active_cue).toBeNull();
});

test('an animation of several images plays in a loop on the tempo', async ({ page }) => {
  await studio.post('/api/control', { id: 'tempo.bpm', value: 240 });
  await openEditor(page);
  await page.locator('#figNew').click();
  // Image 1: a line at the top. Image 2: a line at the bottom.
  await tool(page, 'line');
  await click(page, -0.5, 0.6);
  await page.mouse.dblclick(...await xy(page, 0.5, 0.6));
  await page.locator('#figAddFrame').click();
  await expect(page.locator('#figFrames button')).toHaveCount(2);
  await expect(page.locator('#figFrameInfo')).toHaveText('Image 2 / 2');
  await click(page, -0.5, -0.6);
  await page.mouse.dblclick(...await xy(page, 0.5, -0.6));
  // Image 3: a copy of image 2, moved up to the middle.
  await page.locator('#figDupFrame').click();
  await expect(page.locator('#figFrames button')).toHaveCount(3);
  await tool(page, 'select');
  await drag(page, [0, -0.6], [0, 0]);
  await page.locator('#figRate').fill('1');
  await page.locator('#figPer').selectOption('beat');
  await page.locator('#figLoop').selectOption('ping_pong');
  await page.locator('#figName').fill('Anim');
  await page.screenshot({ path: 'test-results/figures-editor.png' });
  await page.locator('#figPlay').click();
  await expect.poll(async () => (await studio.frame()).active_cue).toBe('figure:Anim');

  const fig = (await load('Anim'))!;
  expect(fig.frames.length).toBe(3);
  expect(fig.loop_mode).toBe('ping_pong');
  expect(Math.abs(fig.frames[2].strokes[0].points[0][1])).toBeLessThan(0.03);
  expect(fig.frames[1].strokes[0].points[0][1]).toBeCloseTo(-0.6, 1);

  // Height of the line on show, and the beat it was drawn on.
  const sample = async () => {
    const f = await studio.frame();
    const pts = f.points.filter(isLit);
    const y = pts.reduce((a: number, p: Point) => a + p[1], 0) / (pts.length || 1);
    return { beat: f.tempo.beat, y: Math.round(y * 10) / 10 + 0, n: pts.length };
  };
  // At 240 BPM, one image per beat: all three heights show within ~2 s.
  const seen = new Set<number>();
  await expect.poll(async () => {
    const s = await sample();
    if (s.n) seen.add(s.y);
    return [...seen].sort((a, b) => a - b);
  }, { timeout: 5_000, intervals: [40] }).toEqual([-0.6, 0, 0.6]);
  // Stepped on the beat, from the bar's one, back and forth: images
  // 1 2 3 2 on beats 1 2 3 4 of every bar (4 beats = one ping-pong period).
  const expected = [0.6, -0.6, 0, -0.6];
  let checked = 0;
  for (let i = 0; i < 40 && checked < 6; i++) {
    await new Promise(r => setTimeout(r, 70));
    const { beat, y, n } = await sample();
    const frac = beat - Math.floor(beat);
    if (!n || frac < 0.2 || frac > 0.9) continue; // too close to a step
    expect(y, `beat ${beat.toFixed(2)}`).toBe(expected[Math.floor(beat) % 4]);
    checked++;
  }
  expect(checked).toBeGreaterThan(2);
});

test('undo and redo, over more than 50 actions', async ({ page }) => {
  await openEditor(page);
  await page.locator('#figNew').click();
  await tool(page, 'point');
  for (let i = 0; i < 55; i++) await click(page, -0.9 + (i % 10) * 0.2, 0.9 - Math.floor(i / 10) * 0.2);
  await expect(page.locator('#figStats')).toContainText('55 tracé(s)');
  for (let i = 0; i < 55; i++) await page.locator('#figUndo').click();
  await expect(page.locator('#figStats')).toContainText('0 tracé(s)');
  for (let i = 0; i < 55; i++) await page.locator('#figRedo').click();
  await expect(page.locator('#figStats')).toContainText('55 tracé(s)');

  // Keyboard: Cmd/Ctrl+Z and Cmd/Ctrl+Shift+Z (focus off the text fields).
  await page.locator('#figCanvas').focus();
  await page.keyboard.press('ControlOrMeta+z');
  await page.keyboard.press('ControlOrMeta+z');
  await expect(page.locator('#figStats')).toContainText('53 tracé(s)');
  await page.keyboard.press('ControlOrMeta+Shift+z');
  await expect(page.locator('#figStats')).toContainText('54 tracé(s)');
  // A new action clears the redo list.
  await tool(page, 'erase');
  await click(page, -0.9, 0.9);
  await expect(page.locator('#figStats')).toContainText('53 tracé(s)');
  await expect(page.locator('#figRedo')).toBeDisabled();
  await page.locator('#figName').fill('Points');
  await page.locator('#figSave').click();
  await expect(page.locator('#figMsg')).toContainText('enregistrée');
  expect((await load('Points'))!.frames[0].strokes.length).toBe(53);
  // Undo also works on transforms and the order of strokes.
  await tool(page, 'select');
  await click(page, 0.9, 0.9);
  await page.locator('#figUp').click();
  await page.locator('#figMirX').click();
  await page.locator('#figUndo').click();
  await page.locator('#figUndo').click();
  await page.locator('#figSave').click();
  await expect(page.locator('#figMsg')).toContainText('enregistrée');
  expect((await load('Points'))!.frames[0].strokes.length).toBe(53);
});

test('delete from the library removes the cue; the Look sub-tab still works', async ({ page }) => {
  await openEditor(page);
  await page.locator('#figNew').click();
  await tool(page, 'ellipse');
  await drag(page, [-0.3, -0.3], [0.3, 0.3]);
  await page.locator('#figName').fill('A effacer');
  await page.locator('#figSave').click();
  await expect(page.locator('#figLib [data-fig="A effacer"]')).toBeVisible();
  expect((await studio.presets()).presets.some(p => p.id === 'figure:A effacer')).toBe(true);
  page.once('dialog', d => d.accept());
  await page.locator('#figLib .figitem', { hasText: 'A effacer' }).locator('.del').click();
  await expect(page.locator('#figLib [data-fig="A effacer"]')).toHaveCount(0);
  expect((await studio.presets()).presets.some(p => p.id === 'figure:A effacer')).toBe(false);
  expect(await load('A effacer')).toBeNull();
  await page.locator('#creationTabs [data-ctab="look"]').click();
  await expect(page.locator('#figEditor')).toBeHidden();
  await expect(page.locator('#shapes')).toBeVisible();
});
