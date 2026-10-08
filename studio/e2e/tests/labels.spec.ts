// Table unique des libellés (T-461): the page's texts come from labels.js,
// filled in at start-up. Preview only: no laser, nothing armed.
import { test, expect, useStudio, openUi, openWorkspace, focusPage, type Workspace } from '../studio';

const studio = useStudio();

test.beforeEach(async () => {
  await studio.post('/api/estop/reset', {});
  await studio.post('/api/arm', { on: false });
});

test.afterEach(async () => {
  await studio.post('/api/estop/reset', {});
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
        for (const s of [own, el.title, el.getAttribute('aria-label') ?? '', el.getAttribute('placeholder') ?? '']) {
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

test('t() fills parameters and flags unknown keys', async ({ page }) => {
  await openUi(page, studio);
  const r = await page.evaluate(() => {
    const w = window as unknown as { t: (k: string, p?: Record<string, unknown>) => string };
    return [w.t('securite.maintien.demande', { touche: 'Maj' }), w.t('nulle.part')];
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

test('without labels.js, the safety texts and Escape still work', async ({ page }) => {
  await page.route('**/labels.js', r => r.abort());
  await page.goto(studio.url + '/');
  await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
  await expect(page.locator('#estopBtn')).toHaveText('ARRÊT');
  await expect(page.locator('#blackoutBtn')).toHaveText('Noir');
  await focusPage(page);
  await page.keyboard.press('Escape');
  await expect(page.locator('#estopBanner')).toBeVisible();
  await expect(page.locator('#estopBanner')).toContainText("ARRÊT D'URGENCE — réinitialiser pour réarmer");
  expect((await studio.state()).armed).toBe(false);
});
