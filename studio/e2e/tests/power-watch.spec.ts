// Battery and overheating warnings (T-495). The reading is simulated
// (--test-hooks, POST /api/test/power): this Mac's battery and thermal
// state are never read. Preview only; the warnings never arm, disarm or dim.
import { test, expect, useStudio, openUi } from '../studio';

const studio = useStudio({ testHooks: true });
const power = (source: string, battery_pct: number | null, thermal: string) => studio.post('/api/test/power', { source, battery_pct, thermal });
const toast = (page: import('@playwright/test').Page, text: string) => page.locator('#toasts .toast-text', { hasText: text });

test.afterEach(async () => {
  await power('ac', 100, 'nominal');
});

test('an injected battery and thermal state shows the pills and the notices', async ({ page }) => {
  await openUi(page, studio);
  await expect(page.locator('#powerPill')).toBeHidden();
  await expect(page.locator('#thermalPill')).toBeHidden();

  expect(await power('battery', 64, 'nominal')).toBe(200);
  await expect(page.locator('#powerPill')).toHaveText('Sur batterie (64 %)');
  await expect(page.locator('#powerPill')).not.toHaveClass(/crit/);
  await expect(toast(page, 'Sur batterie (64 %)')).toBeVisible();
  expect((await studio.state()).power_watch).toEqual({ source: 'battery', battery_pct: 64, thermal: 'nominal' });

  expect(await power('battery', 9, 'serious')).toBe(200);
  await expect(page.locator('#powerPill')).toHaveText('Batterie faible (9 %) : branchez le secteur');
  await expect(page.locator('#powerPill')).toHaveClass(/crit/);
  await expect(page.locator('#thermalPill')).toHaveText('Le Mac chauffe : images en retard possibles');
  await expect(toast(page, 'le Mac va se mettre en veille')).toBeVisible();

  // Information only: the laser stays as it was.
  expect((await studio.state()).armed).toBe(false);

  // Back to normal: pills gone, announced.
  expect(await power('ac', 9, 'nominal')).toBe(200);
  await expect(page.locator('#powerPill')).toBeHidden();
  await expect(page.locator('#thermalPill')).toBeHidden();
  await page.locator('#noticeBtn').click();
  await expect(page.locator('#noticePanel .n-line', { hasText: 'Mac de nouveau sur secteur' })).toHaveCount(1);
  await expect(page.locator('#noticePanel .n-line', { hasText: 'Température du Mac revenue à la normale' })).toHaveCount(1);
  const log = await studio.get('/api/safety/log');
  expect(log.events.filter((e: { kind: string }) => e.kind === 'mac_power').length).toBeGreaterThanOrEqual(4);
});

test('the checklist ticks « Mac sur secteur » by itself on mains power', async () => {
  expect(await power('battery', 50, 'nominal')).toBe(200);
  const item = async () => (await studio.state(), await studio.get('/api/checklist')).items.find((i: { id: string }) => i.id === 'mac_ac');
  expect((await item()).done).toBe(false);
  expect(await power('ac', 50, 'nominal')).toBe(200);
  await expect.poll(async () => (await item()).done).toBe(true);
});
