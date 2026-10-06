// Macros (T-425): command lines with waits, edited in CRÉATION › Macros,
// played from LIVE › Macros, Alt+letter, « En cours »; waits in beats land
// on the beat; refused lines can't be saved; Échap ends every macro.
// Preview only: nothing is armed.
import { test, expect, useStudio, openUi, focusPage, reveal } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();
type Run = { id: string; step: number; go: boolean };
const frame = async () => (await studio.frame()) as unknown as { live: { size: number }; tempo: { beat: number }; armed: boolean; estop: boolean; macros: { running: Run[] } };

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  await studio.post('/api/macros/stop', {});
  await studio.post('/api/lock', { show_mode: false });
  expect(await studio.post('/api/macros', { macros: [] })).toBe(200);
  await studio.post('/api/control', { id: 'tempo.bpm', value: 120 });
  await openUi(page, studio);
});

/** CRÉATION › Macros: a new macro with these steps (command, wait kind, value). */
async function createMacro(page: Page, name: string, steps: [string, string, number?][]) {
  await reveal(page, '#macAdd');
  await page.locator('#macAdd').click();
  await page.locator('#macName').fill(name);
  for (let i = 0; i < steps.length; i++) {
    if (i > 0) await page.locator('#macAddStep').click();
    const [cmd, wait, v] = steps[i];
    await page.locator(`#macCmd${i}`).fill(cmd);
    await page.locator(`#macWait${i}`).selectOption(wait);
    if (v !== undefined) {
      await page.locator(`#macVal${i}`).fill(String(v));
      await page.locator(`#macVal${i}`).dispatchEvent('change');
    }
  }
}

test('taille 50, attendre 1 temps, taille 100: the second step lands on the beat', async ({ page }) => {
  await createMacro(page, 'Montée', [['master.size 50', 'beats', 1], ['master.size 100', 'none']]);
  await page.locator('#macSave').click();
  await expect(page.locator('#macState')).toHaveText('Macros enregistrées');
  await reveal(page, '#macButtons');
  const pad = page.locator('#macButtons .mac-btn', { hasText: 'Montée' });
  await expect(pad).toBeVisible();
  await pad.click();
  const seen: { size: number; beat: number }[] = [];
  const t0 = Date.now();
  while (Date.now() - t0 < 3000) {
    const f = await frame();
    seen.push({ size: f.live.size, beat: f.tempo.beat });
    if (seen.some(s => Math.abs(s.size - 0.5) < 1e-6) && Math.abs(f.live.size - 1) < 1e-6) break;
  }
  const first = seen.findIndex(s => Math.abs(s.size - 0.5) < 1e-6);
  expect(first).toBeGreaterThanOrEqual(0);
  const back = seen.findIndex((s, i) => i > first && Math.abs(s.size - 1) < 1e-6);
  expect(back).toBeGreaterThan(first);
  // Launched at beat b, the step is due on round(b + 1): never before it,
  // and the last frame at 50 % was before it (within one frame, 0.04 beat at 120 BPM).
  const target = Math.round(seen[first].beat + 1);
  expect(seen[back].beat).toBeGreaterThanOrEqual(target - 0.04);
  expect(seen[back - 1].beat).toBeLessThan(target + 0.04);
  expect((await frame()).armed).toBe(false);
});

test('a running macro shows in « En cours » and stops there; Go moves it on', async ({ page }) => {
  await createMacro(page, 'Attente', [['master.size 40', 'go'], ['master.size 70', 'none']]);
  await page.locator('#macSave').click();
  await expect(page.locator('#macState')).toHaveText('Macros enregistrées');
  await reveal(page, '#macButtons');
  await page.locator('#macButtons .mac-btn', { hasText: 'Attente' }).click();
  await expect(page.locator('#macGo')).toBeVisible();
  await expect.poll(async () => (await studio.live()).size).toBeCloseTo(0.4, 5);
  await page.locator('#liveTabs [data-panel-tab="running"]').click();
  const row = page.locator('#runMacros .run-row', { hasText: 'Attente' });
  await expect(row).toContainText('attend Go');
  await row.getByRole('button', { name: 'Arrêter' }).click();
  await expect(page.locator('#runMacros .run-row')).toHaveCount(0);
  expect((await frame()).macros.running).toEqual([]);
  // Again, then Go.
  await page.locator('#liveTabs [data-panel-tab="macros"]').click();
  await page.locator('#macButtons .mac-btn', { hasText: 'Attente' }).click();
  await expect(page.locator('#macGo')).toBeVisible();
  await page.locator('#macGo').click();
  await expect.poll(async () => (await studio.live()).size).toBeCloseTo(0.7, 5);
  await expect(page.locator('#macGo')).toBeHidden();
});

test('a step « armer » is refused under its line, nothing is saved, nothing arms', async ({ page }) => {
  await createMacro(page, 'Interdite', [['tempo 120', 'none'], ['armer', 'none']]);
  await page.locator('#macSave').click();
  const err = page.locator('.mac-step[data-step="2"] .mac-err');
  await expect(err).toBeVisible();
  await expect(err).toContainText('Étape 2 : Commande refusée');
  await expect(page.locator('#macState')).toHaveText('Non enregistré');
  expect((await studio.get('/api/macros')).macros).toEqual([]);
  expect((await frame()).armed).toBe(false);
});

test('Échap during a macro: the stop latches and no step runs after it', async ({ page }) => {
  // T-480: the stop must land inside the wait; 3 s leaves room for the
  // poll and the key press on a loaded machine.
  await createMacro(page, 'Longue', [['master.size 30', 'seconds', 3], ['master.size 90', 'none']]);
  await page.locator('#macSave').click();
  await expect(page.locator('#macState')).toHaveText('Macros enregistrées');
  await reveal(page, '#macButtons');
  await page.locator('#macButtons .mac-btn', { hasText: 'Longue' }).click();
  await expect.poll(async () => (await studio.live()).size).toBeCloseTo(0.3, 5);
  await focusPage(page);
  await page.keyboard.press('Escape');
  await expect.poll(async () => (await frame()).estop).toBe(true);
  expect((await frame()).macros.running).toEqual([]);
  // Past the wait: the second step never runs.
  await page.waitForTimeout(3500);
  expect((await studio.live()).size).toBeCloseTo(0.3, 5);
  expect((await frame()).armed).toBe(false);
  // Launching is refused while latched.
  expect(await studio.post('/api/macros/run', { id: (await studio.get('/api/macros')).macros[0].id })).toBe(409);
});

test('Alt+M launches the macro bound to M without playing the cue of the M key', async ({ page }) => {
  await createMacro(page, 'Touche', [['master.size 60', 'go']]);
  await page.locator('#macKey').selectOption('M');
  await page.locator('#macSave').click();
  await expect(page.locator('#macState')).toHaveText('Macros enregistrées');
  await page.locator('#wsTabs [data-ws-tab="live"]').click();
  await focusPage(page);
  const before = (await studio.frame() as unknown as { cues: { active: unknown[] } }).cues.active.length;
  await page.keyboard.press('Alt+KeyM');
  await expect.poll(async () => (await frame()).macros.running.length).toBe(1);
  await expect.poll(async () => (await studio.live()).size).toBeCloseTo(0.6, 5);
  expect((await studio.frame() as unknown as { cues: { active: unknown[] } }).cues.active.length).toBe(before);
  expect((await frame()).armed).toBe(false);
});
