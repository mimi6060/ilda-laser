// Audience scanning mode (T-255), locked by default. « Au-dessus du
// public » blanks every Public zone; with a real output (simulated here by
// --test-checklist, preview only) arming waits until a Public zone or a
// full horizon exists. « Balayage public » needs every condition, the
// typed phrase, a disarmed laser, and ends on an e-stop and on a restart.
// All declarations below are fictitious: no laser is ever driven.
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';

const PHRASE = 'JE PRENDS LA RESPONSABILITÉ';
const FULL = [[-1, -1], [1, -1], [1, 1], [-1, 1]];

const studio = useStudio({ testChecklist: true });

const mode = async () => (await studio.state()).audience.mode as string;

/** Every lit point of the preview frame: [x, y, r, g, b]. */
async function lit() {
  const f = await studio.frame();
  return f.points.filter(p => Math.max(p[2], p[3], p[4]) > 1e-3);
}

async function openSafety(page: import('@playwright/test').Page) {
  await openWorkspace(page, 'settings');
  if (!(await page.locator('#safetyPanel').getAttribute('open') !== null)) await page.locator('#safetyPanel > summary').click();
}

test('a fresh install starts above the audience and refuses arming until the audience is located', async ({ page }) => {
  expect(await mode()).toBe('above');
  await studio.completeChecklist();
  await openUi(page, studio);
  const arm = await fetch(studio.url + '/api/arm', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ on: true }) });
  expect(arm.status).toBe(409);
  expect((await arm.json()).blocking.join(' ')).toContain('Aucune zone Public');

  // A zone drawn in the Sécurité panel and marked « Public ».
  await openSafety(page);
  await expect(page.locator('#audInfo')).toContainText("l'armement est refusé");
  await page.locator('#sfZoneAdd').click();
  const row = page.locator('#sfZones .sfzone').last();
  await row.locator('.zaud').check();
  await expect.poll(async () => (await studio.get('/api/safety')).settings.zones.some((z: { audience: boolean }) => z.audience)).toBe(true);
  await expect(page.locator('#audInfo')).not.toContainText("l'armement est refusé");
  expect(await studio.post('/api/arm', { on: true })).toBe(200);
  expect(await studio.post('/api/arm', { on: false })).toBe(200);
});

test('the wizard shows every condition, unlocks with the phrase, and the e-stop ends it', async ({ page }) => {
  // A Public zone over the whole field, dimmed to 10 %: blanked while above.
  const { settings } = await studio.get('/api/safety');
  settings.zones = [{ id: 0, name: 'Public', kind: 'dim', level: 0.1, points: FULL, audience: true }];
  settings.dwell.profile = 'beams';
  // Replacing the zones loosens: the operator's confirmation.
  expect(await studio.post('/api/safety', { ...settings, confirm_loosen: true })).toBe(200);
  await openUi(page, studio);
  await expect.poll(async () => (await lit()).length).toBe(0);

  await openSafety(page);
  await page.locator('#audSeg [data-aud="scanning"]').click();
  const wizard = page.locator('#audWizard');
  await expect(wizard).toBeVisible();
  await expect(wizard).toContainText('Le logiciel ne détecte pas une panne des galvos');
  await expect(wizard).toContainText('personne qualifiée');
  const steps = page.locator('#audSteps li');
  await expect(steps).toHaveCount(5);
  await expect(page.locator('#audSteps li[data-cond="dim_zone"]')).toHaveClass('ok');
  for (const id of ['projector', 'checklist', 'hold_to_run', 'dwell_strict']) {
    await expect(page.locator(`#audSteps li[data-cond="${id}"]`)).toHaveClass('bad');
  }
  await expect(page.locator('#audUnlock')).toBeDisabled();
  // The API refuses too, with the missing labels.
  let r = await fetch(studio.url + '/api/audience', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ mode: 'scanning', phrase: PHRASE }) });
  expect(r.status).toBe(409);
  expect((await r.json()).missing).toHaveLength(4);

  // « Régler » opens the setting: the static beam guard, set to strict.
  await page.locator('#audSteps li[data-cond="dwell_strict"] button').click();
  await expect(page.locator('#sfDwellProfile')).toBeFocused();
  await page.locator('#sfDwellProfile').selectOption('strict');
  await expect(page.locator('#audSteps li[data-cond="dwell_strict"]')).toHaveClass('ok');

  // Fictitious declarations: the projector sheet, the checklist with its
  // audience items, hold-to-run.
  expect(await studio.post('/api/outputs/limits', { projector: { name: 'Projecteur test', power_mw: [300, 500, 400], divergence_mrad: 1.5, hw_scan_fail: true } })).toBe(200);
  await studio.post('/api/checklist/restart');
  const cl = await studio.get('/api/checklist');
  expect(await studio.post('/api/checklist/config', { items: cl.items, outdoor: false, audience_scan: true })).toBe(200);
  const items = (await studio.get('/api/checklist')).items.filter((i: { visible: boolean }) => i.visible).map((i: { id: string }) => i.id);
  expect(items).toEqual(expect.arrayContaining(['audience_irradiance', 'audience_attenuation']));
  await studio.completeChecklist();
  const presence = (await studio.get('/api/presence')).settings;
  expect(await studio.post('/api/presence', { ...presence, hold_to_run: true })).toBe(200);
  for (const id of ['projector', 'checklist', 'hold_to_run']) {
    await expect(page.locator(`#audSteps li[data-cond="${id}"]`)).toHaveClass('ok');
  }
  await expect(page.locator('#audUnlock')).toBeEnabled();

  // A wrong phrase is refused.
  await page.locator('#audPhrase').fill('je prends');
  await page.locator('#audUnlock').click();
  await expect(page.locator('#audError')).toContainText('Saisissez');
  expect(await mode()).toBe('above');

  // The phrase: unlocked, orange border and status label.
  await page.locator('#audPhrase').fill(PHRASE);
  await page.locator('#audUnlock').click();
  await expect.poll(mode).toBe('scanning');
  await expect(page.locator('#audPill')).toBeVisible();
  await expect(page.locator('#audPill')).toHaveText('BALAYAGE PUBLIC');
  await expect(page.locator('#stage')).toHaveClass(/aud-scan/);
  await expect(wizard).toBeHidden();
  // The Public zone now gets light, at most its 10 % (under the 50 % cap).
  await expect.poll(async () => (await lit()).length).toBeGreaterThan(0);
  const peak = Math.max(...(await lit()).map(p => Math.max(p[2], p[3], p[4])));
  expect(peak).toBeLessThanOrEqual(0.1 + 1e-3);

  // The safety log has the unlock with its conditions.
  const log = await studio.get('/api/safety/log');
  const unlock = log.events.find((e: { kind: string; detail: { mode: string } }) => e.kind === 'audience_mode' && e.detail.mode === 'scanning');
  expect(unlock.detail.conditions).toHaveLength(5);
  expect(unlock.detail.operator).toBe('Test e2e');

  // Emergency stop: back above the audience, and still so after the reset.
  await page.locator('#estopBtn').click();
  await expect.poll(mode).toBe('above');
  await expect(page.locator('#audPill')).toBeHidden();
  await expect(page.locator('#stage')).not.toHaveClass(/aud-scan/);
  expect(await studio.post('/api/estop/reset')).toBe(200);
  expect(await mode()).toBe('above');
  await expect.poll(async () => (await lit()).length).toBe(0);
  const ended = (await studio.get('/api/safety/log')).events.find((e: { kind: string; detail: { reason: string } }) => e.kind === 'audience_mode' && e.detail.reason === 'estop');
  expect(ended).toBeTruthy();

  // Unlocking is refused while armed.
  expect(await studio.post('/api/arm', { on: true })).toBe(200);
  r = await fetch(studio.url + '/api/audience', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ mode: 'scanning', phrase: PHRASE }) });
  expect(r.status).toBe(409);
  expect((await r.json()).armed).toBe(true);
  expect(await studio.post('/api/arm', { on: false })).toBe(200);

  // Unlocked again, then a condition dropped: hold-to-run off ends it.
  expect(await studio.post('/api/audience', { mode: 'scanning', phrase: PHRASE })).toBe(200);
  expect(await mode()).toBe('scanning');
  expect(await studio.post('/api/presence', { ...presence, hold_to_run: false })).toBe(200);
  expect(await mode()).toBe('above');

  // Back on, then « Au-dessus du public » by hand.
  expect(await studio.post('/api/presence', { ...presence, hold_to_run: true })).toBe(200);
  expect(await studio.post('/api/audience', { mode: 'scanning', phrase: PHRASE })).toBe(200);
  await page.locator('#audSeg [data-aud="above"]').click();
  await expect.poll(mode).toBe('above');

  // A restart never restores the mode.
  expect(await studio.post('/api/audience', { mode: 'scanning', phrase: PHRASE })).toBe(200);
  await studio.restart();
  expect(await mode()).toBe('above');
  expect(await studio.post('/api/presence', { ...presence, hold_to_run: false })).toBe(200);
});
