// Cue editor (T-164): CRÉATION › Cue évolutif. A program is built with the
// mouse (a « Taille » curve and two keys, snapped to the beat unless Alt is
// held), previewed live (pictures only, never the laser), saved on the
// cue, and the relaunched cue plays it: /api/frame at two instants.
// « Annuler » leaves the cue as it was, the server's refusals show in
// French, show mode refuses saving, and « Enregistrer dans une case »
// makes a user cue. Preview only: nothing is ever armed.
import type { Page } from '@playwright/test';
import { test, expect, useStudio, openUi, openWorkspace, extent, type Point } from '../studio';

const studio = useStudio();
const CUE = 'abstraits-001'; // Lissajous 1:2, a steady figure: its size is easy to measure
const SHOT = process.env.PROGRAM_EDITOR_SHOT;

interface Evolving { pos: number; waiting: boolean; cue: string | null }
type ProgFrame = { evolving: Evolving[]; points: Point[] };

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await openUi(page, studio);
});

async function openEditor(page: Page, id: string) {
  await openWorkspace(page, 'creation');
  await page.locator('#creationTabs [data-ctab="program"]').click();
  await expect(page.locator('#pgEditor')).toBeVisible();
  await page.locator('#pgCue').selectOption(id);
  const name = (await studio.presets()).presets.find(p => p.id === id)!.name;
  await expect(page.locator('#pgCueName')).toHaveText(name);
}

// A point of lane `lane` (0 = first) at `beat` and value fraction `v` (0 = min, 1 = max).
async function lanePoint(page: Page, lane: number, beat: number, len: number, v: number) {
  const box = (await page.locator('#pgCanvas').boundingBox())!;
  const x = 10 + beat / len * (box.width - 20), top = 24 + lane * 72;
  return { x: box.x + x, y: box.y + top + 64 - v * 56 };
}

test('a size curve made with the mouse previews, saves and plays on the next launch', async ({ page }) => {
  try {
    await openEditor(page, CUE);
    await page.locator('#pgLen').selectOption('4');
    await page.locator('#pgPlay [data-play="once"]').click();
    await expect(page.locator('#pgPlay [data-play="once"]')).toHaveClass(/active/);
    await page.locator('#pgPlay [data-play="loop"]').click();
    await expect(page.locator('#pgPlay [data-play="once"]')).not.toHaveClass(/active/);
    await expect(page.locator('#pgModified')).toBeVisible();

    await page.locator('#pgAddTarget').selectOption('size');
    let preview = page.waitForResponse(r => r.url().endsWith('/api/cues/program/preview'));
    await page.locator('#pgAddLane').click();
    await expect(page.locator('.pg-head[data-target="size"]')).toBeVisible();
    const before = await (await preview).json();
    expect(before.frames.length).toBeGreaterThan(8);

    // Magnetism: a click at beat 2.4 lands on beat 2; with Alt, where it is.
    let p = await lanePoint(page, 0, 2.4, 4, 0.5);
    await page.mouse.click(p.x, p.y);
    await expect(page.locator('#pgKeyBeat')).toHaveValue('2');
    const snapped = await lanePoint(page, 0, 2, 4, 0.5);
    await page.mouse.click(snapped.x, snapped.y, { button: 'right' });
    await expect(page.locator('.pg-head[data-target="size"] .rng')).toContainText('0 clé');
    await page.keyboard.down('Alt');
    await page.mouse.click(p.x, p.y);
    await page.keyboard.up('Alt');
    const free = +(await page.locator('#pgKeyBeat').inputValue());
    expect(free).toBeGreaterThan(2.2);
    expect(free).toBeLessThan(2.6);
    await page.mouse.click(p.x, p.y, { button: 'right' });

    // Two keys: small at beat 0, full size at beat 4 (exact values typed in).
    p = await lanePoint(page, 0, 0, 4, 0.1);
    await page.mouse.click(p.x + 2, p.y);
    await page.locator('#pgKeyValue').fill('0.25');
    await page.locator('#pgKeyValue').press('Enter');
    p = await lanePoint(page, 0, 4, 4, 0.5);
    await page.mouse.click(p.x - 2, p.y);
    await expect(page.locator('#pgKeyBeat')).toHaveValue('4');
    preview = page.waitForResponse(r => r.url().endsWith('/api/cues/program/preview'));
    await page.locator('#pgKeyValue').fill('1');
    await page.locator('#pgKeyValue').press('Enter');
    await page.locator('#pgKeyEase').selectOption('smooth');
    await expect(page.locator('.pg-head[data-target="size"] .rng')).toContainText('2 clés');
    // The preview changed (its pictures), and nothing was armed or played.
    const after = await (await preview).json();
    expect(after.frames).not.toEqual(before.frames);
    expect((await studio.state()).armed).toBe(false);
    expect((await studio.get('/api/cues')).slots[CUE]?.program ?? null).toBeNull();
    await expect(page.locator('#pgPvInfo')).toContainText('jamais envoyé au laser');

    await page.locator('#pgSave').click();
    await expect(page.locator('#pgMsg')).toContainText('Programme enregistré');
    await expect(page.locator('#pgModified')).toBeHidden();
    const saved = (await studio.get('/api/cues')).slots[CUE].program;
    expect(saved).toMatchObject({ length_beats: 4, play: 'loop' });
    expect(saved.lanes).toEqual([{ target: 'size', keys: [{ beat: 0, value: 0.25, ease: 'linear' }, { beat: 4, value: 1, ease: 'smooth' }] }]);
    if (SHOT) await page.screenshot({ path: SHOT });

    // Relaunched: small at the start of each pass, nearly full size at its end.
    await studio.post('/api/control', { id: 'tempo.bpm', value: 60 });
    expect(await studio.post('/api/cue', { id: CUE, down: true })).toBe(200);
    const at = async (ok: (e: Evolving) => boolean) => {
      const until = Date.now() + 10_000;
      let last: unknown = null;
      while (Date.now() < until) {
        const f = await studio.frame() as unknown as ProgFrame;
        last = f.evolving;
        const e = f.evolving.find(x => x.cue === CUE);
        if (e && !e.waiting && ok(e) && f.points.length) return f;
      }
      throw new Error('instant never reached: ' + JSON.stringify(last));
    };
    const start = await at(e => e.pos < 0.5);
    const end = await at(e => e.pos >= 3.5);
    expect(extent(end.points)).toBeGreaterThan(2.5 * extent(start.points));
    expect((await studio.state()).armed).toBe(false);
  } finally {
    await studio.post('/api/cues/program', { id: CUE, program: null });
  }
});

test('« Annuler » leaves the cue as it was', async ({ page }) => {
  const program = { length_beats: 8, lanes: [{ target: 'size', keys: [{ beat: 0, value: 0.5 }] }] };
  expect(await studio.post('/api/cues/program', { id: CUE, program })).toBe(200);
  try {
    const kept = (await studio.get('/api/cues')).slots[CUE].program;
    await openEditor(page, CUE);
    await expect(page.locator('.pg-head[data-target="size"]')).toBeVisible();
    await page.locator('#pgAddTarget').selectOption('color.hue');
    await page.locator('#pgAddLane').click();
    const p = await lanePoint(page, 1, 4, 8, 0.5);
    await page.mouse.click(p.x, p.y);
    await page.locator('#pgLen').selectOption('16');
    await expect(page.locator('#pgModified')).toBeVisible();
    await page.locator('#pgCancel').click();
    await expect(page.locator('#pgMsg')).toContainText('Modifications annulées');
    await expect(page.locator('#pgModified')).toBeHidden();
    await expect(page.locator('.pg-head[data-target="color.hue"]')).toHaveCount(0);
    await expect(page.locator('#pgLen')).toHaveValue('8');
    expect((await studio.get('/api/cues')).slots[CUE].program).toEqual(kept);
  } finally {
    await studio.post('/api/cues/program', { id: CUE, program: null });
  }
});

test('refusals show in French and show mode refuses saving', async ({ page }) => {
  await openEditor(page, CUE);
  // A program the editor can't make: the server's own words.
  await page.evaluate(() => { (window as any).eval('pg.prog.lanes.push({ target: "mirror", keys: [] })'); });
  await page.locator('#pgSave').click();
  await expect(page.locator('#pgMsg')).toHaveText('Programme refusé : courbe : « mirror » n\'est pas un paramètre du cue');
  await expect(page.locator('#pgMsg')).toHaveClass(/err/);
  await page.locator('#pgCancel').click();

  // Show mode: CRÉATION goes away and the studio refuses the save.
  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  try {
    await expect(page.locator('body')).toHaveClass(/show-mode/);
    await expect(page.locator('#wsTabs [data-ws-tab="creation"]')).toBeHidden();
    expect(await studio.post('/api/cues/program', { id: CUE, program: {} })).toBe(423);
    await page.evaluate(() => document.getElementById('pgSave')!.click());
    await expect(page.locator('#pgMsg')).toHaveText('Verrouillé en mode spectacle');
    expect((await studio.get('/api/cues')).slots[CUE]?.program ?? null).toBeNull();
  } finally {
    await studio.post('/api/lock', { show_mode: false });
  }
});

test('« Modifier le programme… » from a cell, then « Enregistrer dans une case »', async ({ page }) => {
  await page.locator(`#cues .cue[data-id="${CUE}"]`).click({ button: 'right' });
  await page.locator('#cueMenuProgram').click();
  await expect(page.locator('#pgEditor')).toBeVisible();
  await expect(page.locator('#pgCue')).toHaveValue(CUE);
  // An inner layer and a modulator.
  await page.locator('#pgLayerSrc').selectOption('shape:circle');
  await page.locator('#pgLayerAdd').click();
  await expect(page.locator('#pgLayers .pg-item')).toHaveCount(1);
  await expect(page.locator('#pgLayers .pg-item')).toContainText('Forme · Cercle');
  await page.locator('#pgLfoAdd').click();
  await expect(page.locator('#pgLfos .lfo')).toHaveCount(1);
  await page.locator('#pgLfos select[data-f="target"]').selectOption('pos_x');

  await page.locator('#pgStore').click();
  await expect(page.locator('#pgMsg')).toContainText('Enregistré dans la page « Mes cues », case 1');
  const id = await page.locator('#pgCue').inputValue();
  expect(id).toMatch(/^u-\d+$/);
  const info = await studio.get(`/api/cues/program?id=${id}`);
  expect(info.user).toBe(true);
  expect(info.program.layers).toEqual([{ kind: 'shape', shape: 'circle' }]);
  expect(info.program.lfos[0]).toMatchObject({ target: 'pos_x', wave: 'sine' });
  // The built-in cue is untouched.
  expect((await studio.get(`/api/cues/program?id=${CUE}`)).program).toBeNull();
  expect((await studio.state()).armed).toBe(false);
});
