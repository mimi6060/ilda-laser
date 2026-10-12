// Table unique des libellés (T-461): the page's texts come from labels.js,
// filled in at start-up. Preview only: no laser, nothing armed.
import { test, expect, useStudio, openUi, openWorkspace, focusPage, type Workspace } from '../studio';

const studio = useStudio();

test.beforeEach(async () => {
  await studio.post('/api/estop/reset', {});
  await studio.post('/api/arm', { on: false });
});

test.afterEach(async () => {
  await studio.post('/api/estop/reset', {}).catch(() => 0); // the last test stops the studio
});

test('no visible element shows a missing key, in any workspace', async ({ page }) => {
  const warnings: string[] = [];
  page.on('console', m => { if (m.text().startsWith('libellé inconnu')) warnings.push(m.text()); });
  await openUi(page, studio);
  for (const ws of ['live', 'timeline', 'creation', 'settings'] as Workspace[]) {
    await openWorkspace(page, ws);
    const missing = await page.evaluate(() => {
      const out: string[] = [];
      for (const el of document.querySelectorAll<HTMLElement>('body *')) {
        if (!el.checkVisibility()) continue;
        const own = [...el.childNodes].filter(n => n.nodeType === 3).map(n => n.textContent).join('');
        for (const s of [own, el.getAttribute('title') ?? '', el.getAttribute('aria-label') ?? '', el.getAttribute('placeholder') ?? '']) {
          if (s.includes('⟦')) out.push(s);
        }
      }
      return out;
    });
    expect(missing, ws).toEqual([]);
  }
  expect(warnings).toEqual([]);
});

test('known labels read as before', async ({ page }) => {
  await openUi(page, studio);
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
  await expect(page.locator('#wsTabs [data-ws-tab="creation"]')).toHaveText('CRÉATION F3');
  await expect(page.locator('#wsTabs [data-ws-tab="settings"]')).toHaveAttribute('title', 'Calibration, sécurité, MIDI, OSC/Art-Net (F4)');
  await expect(page.locator('#lockBtn')).toHaveAttribute('aria-label', 'Mode spectacle');
  await expect(page.locator('#armInfo')).toHaveText(/^Désarmé/);
});

test('libelle() fills parameters and flags unknown keys', async ({ page }) => {
  await openUi(page, studio);
  const r = await page.evaluate(() => {
    const w = window as unknown as { libelle: (k: string, p?: Record<string, unknown>) => string };
    return [w.libelle('securite.maintien.demande', { touche: 'Maj' }), w.libelle('nulle.part')];
  });
  expect(r).toEqual(['MAINTENIR « Maj » POUR ÉMETTRE', '⟦nulle.part⟧']);
});

test('the emergency stop banner shows after Escape', async ({ page }) => {
  await openUi(page, studio);
  await focusPage(page);
  await page.keyboard.press('Escape');
  await expect(page.locator('#estopBanner')).toBeVisible();
  await expect(page.locator('#estopBanner')).toContainText("ARRÊT D'URGENCE — réinitialiser pour réarmer");
  await expect(page.locator('#estopReset')).toHaveText("Réinitialiser l'arrêt d'urgence");
  expect((await studio.state()).armed).toBe(false);
});

// The alerts of the top bar and its banners: never empty, even without labels.js.
const ALERTS = ['#linkLost', '#sdPill', '#wifiPill', '#midiLost', '#dmxLost', '#audPill', '#estopBanner'];

/** Texts of `#top` (own text, title, aria-label) that show a missing key. */
async function missingKeys(page: import('@playwright/test').Page) {
  return page.evaluate(() => {
    const out: string[] = [];
    for (const el of document.querySelectorAll<HTMLElement>('#top, #top *')) {
      const own = [...el.childNodes].filter(n => n.nodeType === 3).map(n => n.textContent).join('');
      for (const s of [own, el.getAttribute('aria-label') ?? '']) if (s.includes('⟦')) out.push(el.id + ': ' + s);
    }
    return out;
  });
}

test('without labels.js, the safety texts, arming and Escape still work', async ({ page }) => {
  await page.route('**/labels.js', r => r.abort());
  await page.goto(studio.url + '/');
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
  await expect(page.locator('#estopBtn')).toHaveText('ARRÊT');
  await expect(page.locator('#blackoutBtn')).toHaveText('Noir');
  await expect(page.locator('#armInfo')).toHaveText(/^Désarmé/);
  await expect(page.locator('#presenceLed')).toHaveText(/^Opérateur (présent|absent)$/);
  await expect(page.locator('#engineLed')).toHaveText('Moteur');
  for (const sel of ALERTS) {
    expect((await page.locator(sel).textContent())?.trim(), sel).toBeTruthy();
  }
  // Arm from the UI (preview: the fake output, no laser).
  await page.locator('#armBtn').click();
  await expect.poll(async () => (await studio.state()).armed).toBe(true);
  await expect(page.locator('#armBtn')).toHaveText('LASER ON');
  await expect(page.locator('#armInfo')).toHaveText(/^Armé depuis .+ \(.+\)$/);
  expect(await missingKeys(page)).toEqual([]);
  // Escape: the e-stop latches and its banner reads in full.
  await focusPage(page);
  await page.keyboard.press('Escape');
  await expect(page.locator('#estopBanner')).toBeVisible();
  await expect(page.locator('#estopBanner')).toContainText("ARRÊT D'URGENCE — réinitialiser pour réarmer");
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
  await expect(page.locator('#armInfo')).toHaveText(/^Désarmé — raison : /);
  expect((await studio.state()).armed).toBe(false);
  expect(await missingKeys(page)).toEqual([]);
});

// Last: stops this file's studio.
test('without labels.js, a lost engine still reads « Connexion au moteur perdue »', async ({ page }) => {
  await page.route('**/labels.js', r => r.abort());
  await page.goto(studio.url + '/');
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
  await expect(page.locator('#linkLost')).toBeHidden();
  await studio.stop();
  await expect(page.locator('#linkLost')).toBeVisible({ timeout: 4000 });
  await expect(page.locator('#linkLost')).toHaveText('Connexion au moteur perdue');
  expect(await missingKeys(page)).toEqual([]);
});
