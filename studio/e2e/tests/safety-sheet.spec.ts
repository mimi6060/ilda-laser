// Safety sheet (T-263): RÉGLAGES › Fiche sécurité opens a printable,
// script-free summary of the settings in force in a new tab. Generating it
// changes nothing and never arms; the mobile number typed for it is
// written to no file and kept nowhere in the browser.
// Preview only: the studio has no output (no --device).
import { test, expect, useStudio, openUi, reveal } from '../studio';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';

const studio = useStudio();

const PHONE = '+32 470 98 76 54';

function filesUnder(dir: string): string[] {
  return readdirSync(dir).flatMap(name => {
    const p = path.join(dir, name);
    return statSync(p).isDirectory() ? filesUnder(p) : [p];
  });
}

test.beforeEach(async ({ page }) => {
  // Zones once per studio: posting them again would remove the saved ones
  // (a loosening, refused without confirmation).
  const safety = (await studio.get('/api/safety')).settings;
  if (safety.zones.length === 0) expect(await studio.post('/api/safety', {
    ...safety,
    zones: [
      { name: 'Régie', kind: 'blank', points: [[-1, -1], [-0.6, -1], [-0.6, -0.6], [-1, -0.6]] },
      { name: 'Fosse', kind: 'blank', audience: true, points: [[-0.4, -1], [0.4, -1], [0.4, -0.7], [-0.4, -0.7]] },
    ],
  })).toBe(200);
  expect(await studio.post('/api/outputs/limits', { projector: { name: 'Banc RGB', class: '4', power_mw: [300, 400, 300], wavelength_nm: [638, 520, 450], aperture_mm: 5, divergence_mrad: 1 } })).toBe(200);
  await studio.reset();
  await openUi(page, studio);
});

test('the sheet route serves a script-free page with every section and the disclaimer', async () => {
  const r = await fetch(studio.url + '/api/safety/sheet');
  expect(r.status).toBe(200);
  expect(r.headers.get('content-security-policy')).toContain("default-src 'none'");
  const html = await r.text();
  for (const title of ['1. Événement', '2. Projecteurs et sorties', '3. Extérieur', '4. Mesures de sécurité configurées', '5. Estimation d\'exposition']) {
    expect(html).toContain(title);
  }
  expect(html).toContain('Régie');
  expect(html).toContain('Fosse');
  expect(html).toContain('Banc RGB');
  expect(html).toContain('Ne remplace ni une mesure ni l&#39;avis d&#39;un expert.');
  expect(html).toContain('(estimation)');
  expect(html.toLowerCase()).not.toContain('<script');
});

test('« Générer la fiche » opens the sheet in a new tab and changes nothing', async ({ page, context }) => {
  const before = { safety: (await studio.get('/api/safety')).settings, outputs: (await studio.get('/api/outputs/limits')).outputs, armed: (await studio.state()).armed };
  await reveal(page, '#safetySheetPanel');
  await page.locator('#safetySheetPanel > summary').click();
  await expect(page.locator('#ssBanner')).toContainText('ne remplace');
  await page.locator('#ssName').fill('Fête du village');
  await page.locator('#ssVenue').fill('Place communale');
  await page.locator('#ssOperator').fill('A. Opérateur');
  await page.locator('#ssPhone').fill(PHONE);
  await page.locator('#ssDates input').first().fill('10/10/2026');
  const [sheet] = await Promise.all([context.waitForEvent('page'), page.locator('#ssGenerate').click()]);
  await sheet.waitForLoadState();
  await expect(sheet.locator('h1')).toContainText('Fiche sécurité du show');
  await expect(sheet.locator('h2')).toHaveCount(5);
  await expect(sheet.locator('body')).toContainText('Fête du village');
  await expect(sheet.locator('body')).toContainText(PHONE);
  await expect(sheet.locator('svg.zones polygon')).toHaveCount(2);
  await expect(sheet.locator('script')).toHaveCount(0);
  await expect(page.locator('#ssError')).toHaveText('');
  await sheet.close();

  const after = { safety: (await studio.get('/api/safety')).settings, outputs: (await studio.get('/api/outputs/limits')).outputs, armed: (await studio.state()).armed };
  expect(after).toEqual(before);
  expect(after.armed).toBe(false);
  // The number is in no file of the data folder and not kept by the page.
  for (const f of filesUnder(studio.dataDir)) {
    expect(readFileSync(f).includes(PHONE), f).toBe(false);
  }
  const stored = await page.evaluate(() => JSON.stringify({ ...localStorage }));
  expect(stored).not.toContain(PHONE);
  expect(stored).toContain('Fête du village');
});

test('a refused event field shows the reason and opens nothing', async ({ page, context }) => {
  await reveal(page, '#safetySheetPanel');
  await page.locator('#safetySheetPanel > summary').click();
  // Over the limit: the server refuses it (the input's maxlength is bypassed).
  await page.locator('#ssName').evaluate((el: HTMLInputElement) => { el.removeAttribute('maxlength'); el.value = 'x'.repeat(201); });
  const pages = context.pages().length;
  await page.locator('#ssGenerate').click();
  await expect(page.locator('#ssError')).toContainText('200 caractères');
  await expect.poll(() => context.pages().length).toBe(pages);
});
