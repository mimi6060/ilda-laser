// Safety profiles per venue (T-260): the settings in force saved under a
// name, loaded in one go (disarming, the checklist restarted), « Aperçu
// sûr » built in and untouchable, a looser profile only after the
// operator's confirmation, the active profile remembered across a restart
// (the laser still disarmed). Locked in show mode; never from a controller.
// Preview only: the fake output, never a laser.
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();
const BUILTIN = 'Aperçu sûr';
const BAND = [[-1, -1], [1, -1], [1, -0.6], [-1, -0.6]];

const profiles = () => studio.get('/api/safety/profiles');
const caps = async () => {
  const j = await studio.get('/api/outputs/limits');
  return j.outputs.find((o: { id: string }) => o.id === j.active).limits;
};

async function openSafety(page: Page) {
  await openWorkspace(page, 'settings');
  if ((await page.locator('#safetyPanel').getAttribute('open')) === null) await page.locator('#safetyPanel > summary').click();
  await expect(page.locator('#sfpSelect option')).not.toHaveCount(0);
}

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await openUi(page, studio);
});

test('an existing install keeps its settings as « Réglages actuels »; « Aperçu sûr » is built in', async ({ page }) => {
  const v = await profiles();
  expect(v.active).toBe('Réglages actuels');
  expect(v.modified).toBe(false);
  expect(v.profiles[0]).toMatchObject({ name: BUILTIN, builtin: true });
  expect((await caps()).max_power).toBeCloseTo(0.5, 5);
  await expect(page.locator('#profilePill')).toHaveText('Profil : Réglages actuels');
  // « Aperçu sûr » can be neither modified nor deleted.
  expect(await studio.post('/api/safety/profiles', { action: 'save', name: BUILTIN, confirm_loosen: true })).toBe(403);
  expect(await studio.post('/api/safety/profiles', { action: 'delete', name: BUILTIN })).toBe(403);
  expect(await studio.post('/api/safety/profiles', { action: 'verify', name: BUILTIN })).toBe(403);
  await openSafety(page);
  await page.locator('#sfpSelect').selectOption(BUILTIN);
  await expect(page.locator('#sfpDelete')).toBeDisabled();
  await expect(page.locator('#sfpVerify')).toBeDisabled();
  await expect(page.locator('#safetyPanel')).toContainText("l'arrêt réel reste matériel");
});

test('save as, check on site, load a tighter profile at once and a looser one only once confirmed', async ({ page }) => {
  // A zone in force (tighter: applied at once), saved as « Club test ».
  const { settings } = await studio.get('/api/safety');
  settings.zones = [{ id: 0, name: 'Bas', kind: 'blank', level: 0, points: BAND }];
  expect(await studio.post('/api/safety', settings)).toBe(200);
  await openSafety(page);
  await page.locator('#sfpSaveAs').click();
  await page.locator('#sfpName').fill('Club test');
  await page.locator('#sfpVenue').fill('Le Hangar');
  await page.locator('#sfpSaveOk').click();
  await expect.poll(async () => (await profiles()).active).toBe('Club test');
  await expect(page.locator('#profilePill')).toHaveText('Profil : Club test');
  // Never checked on site: the warning, until « vérifié aujourd'hui ».
  await expect(page.locator('#sfpStale')).toBeVisible();
  await page.locator('#sfpSelect').selectOption('Club test');
  await page.locator('#sfpVerify').click();
  await expect(page.locator('#sfpStale')).toBeHidden();
  expect((await profiles()).active_profile.verified_on).toMatch(/^\d{4}-\d{2}-\d{2}$/);

  // Armed, then « Aperçu sûr »: it removes the zone (looser), so it asks.
  expect(await studio.post('/api/arm', { on: true })).toBe(200);
  await page.locator('#sfpSelect').selectOption(BUILTIN);
  await page.locator('#sfpLoad').click();
  await expect(page.locator('#sfpLoosen')).toBeVisible();
  await expect(page.locator('#sfpLoosenText')).toContainText('Zone « Bas » supprimée');
  // Nothing changed yet: still armed, the zone still in force.
  expect((await studio.state()).armed).toBe(true);
  expect((await studio.get('/api/safety')).settings.zones).toHaveLength(1);
  await page.locator('#sfpLoosenOk').click();
  await expect.poll(async () => (await profiles()).active).toBe(BUILTIN);
  const st = await studio.state();
  expect(st.armed).toBe(false);
  expect(st.arm.last_disarm.reason_fr).toBe('Changement de profil');
  expect((await studio.get('/api/safety')).settings.zones).toHaveLength(0);
  expect((await caps()).max_power).toBeCloseTo(0.2, 5);
  await expect(page.locator('#profilePill')).toHaveText(`Profil : ${BUILTIN}`);

  // Back to « Club test »: the cap goes up from 20 % to 50 %, so the API
  // answers 409 with the list; confirmed, the zone is back.
  const r = await fetch(studio.url + '/api/safety/profiles/load', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ name: 'Club test' }) });
  expect(r.status).toBe(409);
  expect((await r.json()).loosen.join(' ')).toContain('Puissance max');
  expect(await studio.post('/api/safety/profiles/load', { name: 'Club test', confirm_loosen: true })).toBe(200);
  expect((await studio.get('/api/safety')).settings.zones).toHaveLength(1);
  // And « Aperçu sûr » is not tighter now (the zone goes): still asks.
  expect(await studio.post('/api/safety/profiles/load', { name: BUILTIN })).toBe(409);
  // The safety log has the loads.
  const log = await studio.get('/api/safety/log');
  expect(log.events.filter((e: { kind: string }) => e.kind === 'safety_profile').length).toBeGreaterThanOrEqual(3);
});

test('only the operator switches profile, never in show mode, never an unknown one', async () => {
  for (const source of ['midi', 'osc', 'dmx', 'network', 'system']) {
    expect(await studio.post('/api/safety/profiles/load', { name: 'Club test', source, confirm_loosen: true })).toBe(403);
  }
  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  expect(await studio.post('/api/safety/profiles/load', { name: 'Club test' })).toBe(423);
  expect(await studio.post('/api/safety/profiles', { action: 'save', name: 'Autre' })).toBe(423);
  expect(await studio.post('/api/lock', { show_mode: false })).toBe(200);
  // Unknown: disarmed, the settings in force kept.
  expect(await studio.post('/api/arm', { on: true })).toBe(200);
  const before = (await studio.get('/api/safety')).settings;
  expect(await studio.post('/api/safety/profiles/load', { name: 'Inconnu' })).toBe(404);
  expect((await studio.state()).armed).toBe(false);
  expect((await studio.get('/api/safety')).settings).toEqual(before);
});

test('the active profile comes back after a restart, the laser disarmed', async ({ page }) => {
  expect((await profiles()).active).toBe('Club test');
  await studio.restart();
  await openUi(page, studio);
  const v = await profiles();
  expect(v.active).toBe('Club test');
  expect(v.modified).toBe(false);
  expect((await studio.state()).armed).toBe(false);
  expect((await studio.get('/api/safety')).settings.zones).toHaveLength(1);
  await expect(page.locator('#profilePill')).toHaveText('Profil : Club test');
});
