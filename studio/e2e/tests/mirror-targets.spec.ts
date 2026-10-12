// Mirror targets (T-109): RÉGLAGES › Cibles miroir lists the machine's
// calibrated points. Adding or moving one sends a static beam somewhere
// new, so it waits for « Confirmer l'assouplissement »; « Viser » takes the
// next click on the 2D preview; a target below the horizon is refused with
// a message; one in a Blank zone stays dark and the panel says so; the
// list is still there after a reload and a restart. The « Cibles miroir »
// effect lights the checked targets. Preview only: the fake output, never
// a laser.
import { test, expect, useStudio, openUi, openWorkspace, reveal, isLit, centroid } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();
const targets = async () => (await studio.get('/api/targets')).items as { name: string; x: number; y: number }[];
const safety = async () => (await studio.get('/api/safety')).settings;

async function openTargets(page: Page) {
  await openWorkspace(page, 'settings');
  if ((await page.locator('#mtPanel').getAttribute('open')) === null) await page.locator('#mtPanel > summary').click();
  await expect(page.locator('#mtAdd')).toBeVisible();
}

test.beforeEach(async () => {
  await studio.reset();
  expect(await studio.post('/api/targets', { items: [] })).toBe(200);
  const s = await safety();
  expect(await studio.post('/api/safety', { ...s, zones: [], confirm_loosen: true })).toBe(200);
});

test('add a target with the confirmation, aim it in the preview, find it after a reload and a restart', async ({ page }) => {
  await openUi(page, studio);
  await openTargets(page);
  await expect(page.locator('#mtList')).toContainText('Aucune cible');

  // Adding sends light somewhere new: nothing stored before the confirmation.
  await page.locator('#mtAdd').click();
  await expect(page.locator('#mtLoosen')).toBeVisible();
  await expect(page.locator('#mtLoosenText')).toContainText('Cible 1 « Cible 1 » ajoutée');
  expect(await targets()).toEqual([]);
  await page.locator('#mtLoosenOk').click();
  await expect.poll(targets).toEqual([{ name: 'Cible 1', x: 0, y: 0.5 }]);
  await expect(page.locator('#mtLoosen')).toBeHidden();
  await expect(page.locator('#mtName0')).toHaveValue('Cible 1');

  // « Viser »: the next click on the preview moves it (identity calibration).
  await page.getByRole('button', { name: 'Viser la cible 1 dans l’aperçu' }).click();
  await expect(page.locator('#mtAimHint')).toBeVisible();
  const box = (await page.locator('#preview').boundingBox())!;
  await page.locator('#preview').click({ position: { x: box.width * 0.75, y: box.height * 0.25 } });
  await expect(page.locator('#mtLoosenText')).toContainText('déplacée');
  await page.locator('#mtLoosenOk').click();
  await expect.poll(async () => (await targets())[0].x).toBeCloseTo(0.5, 1);
  expect((await targets())[0].y).toBeCloseTo(0.5, 1);
  await expect(page.locator('#mtAimHint')).toBeHidden();

  // Renaming changes no light: applied at once.
  await page.locator('#mtName0').fill('Boule à facettes');
  await page.locator('#mtName0').press('Tab');
  await expect.poll(async () => (await targets())[0].name).toBe('Boule à facettes');
  await expect(page.locator('#mtLoosen')).toBeHidden();

  await page.reload();
  await openTargets(page);
  await expect(page.locator('#mtName0')).toHaveValue('Boule à facettes');

  await studio.restart();
  expect((await targets()).map(t => t.name)).toEqual(['Boule à facettes']);
});

test('a target below the horizon is refused, one in a Blank zone is shown cut', async ({ page }) => {
  expect(await studio.post('/api/targets', { items: [{ name: 'Mur', x: 0.3, y: 0.5 }], confirm_loosen: true })).toBe(200);
  await openUi(page, studio);
  await openTargets(page);
  const y = page.locator('#mtY0');
  await y.fill('-0.5');
  await y.press('Tab');
  await expect(page.locator('#mtError')).toContainText('sous l\'horizon');
  await expect(y).toHaveValue('0.500');
  expect((await targets())[0].y).toBe(0.5);

  const s = await safety();
  s.zones = [{ id: 1, name: 'Régie', kind: 'blank', level: 0, points: [[0, 0], [1, 0], [1, 1], [0, 1]] }];
  expect(await studio.post('/api/safety', s)).toBe(200);
  await page.locator('#mtPanel > summary').click();
  await page.locator('#mtPanel > summary').click();
  await expect(page.locator('.mtstatus')).toContainText('Coupée : Dans la zone sans laser « Régie »');
});

test('the « Cibles miroir » effect lights the checked targets, through the safety stage', async ({ page }) => {
  const items = [{ name: 'Gauche', x: -0.5, y: 0.6 }, { name: 'Droite', x: 0.5, y: 0.6 }];
  expect(await studio.post('/api/targets', { items, confirm_loosen: true })).toBe(200);
  const settings = (await studio.state()).settings;
  const look = { ...settings, content: { kind: 'generator', generator: 'mirror_targets', params: { a: 1, b: 0 } } };
  expect(await studio.post('/api/settings', look)).toBe(200);
  await expect.poll(async () => centroid((await studio.frame()).points).x).toBeCloseTo(-0.5, 1);

  await openUi(page, studio);
  await reveal(page, '#gTargets');
  const left = page.getByRole('checkbox', { name: '1. Gauche' }), right = page.getByRole('checkbox', { name: '2. Droite' });
  await expect(left).toBeChecked();
  await expect(right).not.toBeChecked();
  await right.check();
  await expect.poll(async () => (await studio.state()).settings.content.params.a).toBe(3);
  await left.uncheck();
  await expect.poll(async () => (await studio.state()).settings.content.params.a).toBe(2);
  await expect.poll(async () => centroid((await studio.frame()).points).x).toBeCloseTo(0.5, 1);

  // A Blank zone over the right one: the safety stage cuts its beam.
  const s = await safety();
  s.zones = [{ id: 1, name: 'Bar', kind: 'blank', level: 0, points: [[0.2, 0.2], [0.8, 0.2], [0.8, 0.9], [0.2, 0.9]] }];
  expect(await studio.post('/api/safety', s)).toBe(200);
  await expect.poll(async () => (await studio.frame()).points.filter(isLit).length).toBe(0);
  expect((await studio.state()).armed).toBe(false);
});

test('adding or moving is refused armed and in show mode; removing always runs', async ({ page }) => {
  expect(await studio.post('/api/targets', { items: [{ name: 'Boule', x: 0.2, y: 0.5 }], confirm_loosen: true })).toBe(200);
  await openUi(page, studio);
  await openTargets(page);
  expect(await studio.post('/api/arm', { on: true })).toBe(200);
  const x = page.locator('#mtX0');
  await x.fill('-0.4');
  await x.press('Tab');
  await expect(page.locator('#mtError')).toContainText('Désarmez le laser');
  expect((await targets())[0].x).toBeCloseTo(0.2, 5);
  expect(await studio.post('/api/arm', { on: false })).toBe(200);
  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  // Show mode (T-283) keeps only what plays on screen: RÉGLAGES is left for LIVE.
  await expect(page.locator('body')).toHaveClass(/show-mode/);
  await expect(page.locator('#mtAdd')).toBeHidden();
  expect(await studio.post('/api/targets', { items: [] })).toBe(423);
  expect(await studio.post('/api/lock', { show_mode: false })).toBe(200);
  await expect(page.locator('body')).not.toHaveClass(/show-mode/);
  await openTargets(page);
  await page.getByRole('button', { name: 'Supprimer la cible 1' }).click();
  await expect.poll(targets).toEqual([]);
  expect((await studio.state()).armed).toBe(false);
});
