// Microphone permission and input diagnostics (T-242). Preview only and
// --no-audio: no microphone or interface is ever opened. The native input
// is simulated by --test-hooks' POST /api/test/native_audio: a capture
// state, a device list and an analysis snapshot carrying the health the
// studio's analysis gives the samples (exact zeros, background noise,
// clipping, too quiet: unit-tested on the real chain in audio/worker.rs).
// The studio only detects and explains: nothing here touches macOS'
// privacy settings, and an input problem never arms the laser.
import { test, expect, useStudio, openUi, openWorkspace, reveal, type Studio } from '../studio';
import type { Page } from '@playwright/test';

const studio: Studio = useStudio({ testHooks: true });
const MIC = { name: 'Micro test', is_default: true };

/** Posts the simulated native input every 100 ms for `ms`. */
async function feed(ms: number, snapshot: Record<string, unknown>) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    expect(await studio.post('/api/test/native_audio', { state: 'running', device: MIC.name, snapshot })).toBe(200);
    await new Promise(r => setTimeout(r, 100));
  }
}

async function openPanel(page: Page) {
  await openUi(page, studio);
  await openWorkspace(page, 'settings');
  await page.locator('#micDiagPanel > summary').click();
  await expect(page.locator('#micDiagGrid')).toBeVisible();
}

let errors: string[] = [];
test.beforeEach(async ({ page }) => {
  errors = [];
  page.on('pageerror', e => errors.push(String(e)));
  page.on('console', m => { if (m.type() === 'error') errors.push(m.text()); });
  await studio.reset();
  expect(await studio.post('/api/audio/config', { source: 'browser', device: null })).toBe(200);
  expect(await studio.post('/api/test/native_audio', { state: 'off', devices: [] })).toBe(200);
});
test.afterEach(() => { expect(errors).toEqual([]); });

test('the panel is read-only and says when the native capture is off', async ({ page }) => {
  await openPanel(page);
  await expect(page.locator('#micDiagHealth')).toHaveText('Capture native arrêtée');
  await expect(page.locator('#micDiagSource')).toHaveText('Navigateur');
  await expect(page.locator('#micDiagSignal')).toHaveText('non');
  await expect(page.locator('#micDiagSteps')).toContainText('Confidentialité et sécurité › Microphone');
  // No button that would change the privacy settings: only « Actualiser »
  // and the help on the Mac's own sound (T-241).
  await expect(page.locator('#micDiagPanel button')).toHaveText(['Actualiser', 'Comment écouter le son du Mac ?']);
  await expect(page.locator('#micDiagSystem')).toContainText('indisponible');
  await expect(page.locator('#micDiagPanel')).not.toContainText('tccutil');
  // Found by the RÉGLAGES search.
  expect(await page.locator('#micDiagPanel').getAttribute('data-search')).toContain('micro');
});

test('exact zeros: « Autorisation refusée ? » in red, with the help, in LIVE and in RÉGLAGES', async ({ page }) => {
  expect(await studio.post('/api/audio/config', { source: 'native' })).toBe(200);
  expect(await studio.post('/api/test/native_audio', { devices: [MIC, { name: 'Carte son', is_default: false }] })).toBe(200);
  await openPanel(page);
  const feeding = feed(2500, { level_db: -120, peak_db: -120, health: 'permission_suspected' });
  await expect(page.locator('#micDiagHealth')).toHaveText('Autorisation refusée ?');
  await expect(page.locator('#micDiagHealth')).toHaveAttribute('data-tone', 'bad');
  await expect(page.locator('#micDiagHelp')).toContainText('Réglages Système › Confidentialité et sécurité › Microphone');
  await expect(page.locator('#micDiagSignal')).toHaveText('non');
  await expect(page.locator('#micDiagState')).toContainText('en cours');
  await expect(page.locator('#micDiagRate')).toHaveText('48,0 kHz · 1 canal');
  await expect(page.locator('#micDiagLevel')).toHaveText('−∞ dBFS / −∞ dBFS');
  await expect(page.locator('#micDiagDevices li')).toHaveText(['Micro test (par défaut)', 'Carte son']);
  // The badge next to the meter says the same.
  await reveal(page, '#musPanel');
  await expect(page.locator('#musHealth')).toHaveText('Autorisation refusée ?');
  await expect(page.locator('#musSilentHint')).toBeVisible();
  await feeding;
  // Never arms anything.
  expect((await studio.state()).armed).toBe(false);
});

test('silence, too quiet, too loud and a good level each have their badge colour', async ({ page }) => {
  expect(await studio.post('/api/audio/config', { source: 'native' })).toBe(200);
  await openUi(page, studio);
  await reveal(page, '#musPanel');
  const cases: [string, number, number, string, string][] = [
    ['silent', -70, -60, 'Silence', 'warn'],
    ['too_quiet', -50, -40, 'Entrée trop faible', 'warn'],
    ['clipping', -3, 0, 'Entrée trop forte', 'warn'],
    ['ok', -18, -6, 'Signal correct', 'ok'],
  ];
  for (const [health, level_db, peak_db, label, tone] of cases) {
    const feeding = feed(800, { level_db, peak_db, health });
    await expect(page.locator('#musHealth')).toHaveText(label);
    await expect(page.locator('#musHealth')).toHaveAttribute('data-tone', tone);
    // A warning, not the permission help.
    await expect(page.locator('#musSilentHint')).toBeHidden();
    await feeding;
  }
  // The meter still marks the advised zone −25..0 dBFS.
  await expect(page.locator('#musDbMeter .zone')).toBeVisible();
  expect((await studio.state()).armed).toBe(false);
});

test('no device and a lost device: red badge and the last error kept', async ({ page }) => {
  expect(await studio.post('/api/audio/config', { source: 'native' })).toBe(200);
  await openPanel(page);
  expect(await studio.post('/api/test/native_audio', { state: 'no_device', message: 'Pas d\'entrée audio' })).toBe(200);
  await expect(page.locator('#micDiagHealth')).toHaveText('Aucune entrée audio');
  await expect(page.locator('#micDiagHealth')).toHaveAttribute('data-tone', 'bad');
  expect(await studio.post('/api/test/native_audio', { state: 'device_lost', device: MIC.name, message: 'Entrée « Micro test » perdue (débranchée)' })).toBe(200);
  await expect(page.locator('#micDiagHealth')).toHaveText('Entrée perdue');
  await expect(page.locator('#micDiagError')).toContainText('Entrée « Micro test » perdue (débranchée)');
  // Back: the badge recovers, the last error stays readable.
  const feeding = feed(1200, { level_db: -18, peak_db: -6, health: 'ok' });
  await expect(page.locator('#micDiagHealth')).toHaveText('Signal correct');
  await expect(page.locator('#micDiagSignal')).toHaveText('oui');
  await expect(page.locator('#micDiagError')).toContainText('perdue');
  await feeding;
});
