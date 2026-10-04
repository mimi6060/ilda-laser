// Festival palettes and perceived balance (T-130): the row of palette
// pills above the cue grid, quantised to the next bar (or phrase, or at
// once), « Couleurs des cues » to give the looks their colours back,
// « Équilibre perçu » with its preset, and the two new colour modes of
// the Effet tab. Preview only: the laser is never armed here.
import { test, expect, useStudio, openUi, openWorkspace, reveal, isLit, type Point } from '../studio';

const studio = useStudio();
type Festival = {
  palettes: { name: string; c1: number[]; c2: number[] }[];
  active: number | null;
  pending: { palette: number | null; at_beat: number } | null;
  quantize: string;
  balance: { enabled: boolean; gains: number[] };
};
const festival = async () => (await studio.get<{ festival: Festival }>('/api/state')).festival;
const lit = async (): Promise<Point[]> => (await studio.frame()).points.filter(isLit);
const pill = (page: import('@playwright/test').Page, name: string) => page.locator('#festPals .fest-pal', { hasText: name });

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.post('/api/control', { id: 'tempo.bpm', value: 120 });
  await studio.post('/api/festival', { quantize: 'now', balance: { enabled: false, gains: [1, 0.6, 1] } });
  await studio.post('/api/festival/palette', { palette: null });
  // A red beam fan on show.
  const s = (await studio.state()).settings;
  await studio.post('/api/settings', {
    ...s, color: [255, 0, 0], brightness: 1,
    content: { kind: 'generator', generator: 'beam_fan', params: { count: 6, color_mode: 'solid', color2: [0, 0, 255] } },
  });
  await openUi(page, studio);
  await reveal(page, '#festRow');
});

test('eight palettes in French, one click recolours what plays', async ({ page }) => {
  await expect(page.locator('#festPals .fest-pal')).toHaveText([
    'Vert pur', 'Blanc', 'Cyan glacier', 'Rouge hardstyle', 'Afterlife', 'Vert/bleu', 'Coucher de soleil', 'Pop',
  ]);
  await expect.poll(async () => (await lit()).length).toBeGreaterThan(0);
  expect((await lit()).every(p => p[2] > 0 && p[3] === 0)).toBe(true);
  await page.locator('#festQuant button', { hasText: 'Immédiat' }).click();
  await pill(page, 'Vert pur').click();
  await expect.poll(async () => (await festival()).active).toBe(0);
  await expect(pill(page, 'Vert pur')).toHaveClass(/on/);
  await expect.poll(async () => (await lit()).every(p => p[2] === 0 && p[3] > 0 && p[4] === 0)).toBe(true);
  // Every palette applies.
  for (let i = 1; i < 8; i++) {
    await page.locator('#festPals .fest-pal').nth(i).click();
    await expect.poll(async () => (await festival()).active).toBe(i);
  }
  // « Couleurs des cues »: the look's red again.
  await page.locator('#festOwn').click();
  await expect.poll(async () => (await festival()).active).toBe(null);
  await expect.poll(async () => (await lit()).every(p => p[2] > 0 && p[3] === 0)).toBe(true);
  // The look itself was never changed.
  expect((await studio.state()).settings.color).toEqual([255, 0, 0]);
});

test('with « Mesure », a palette picked mid-bar lands on the next one', async ({ page }) => {
  await studio.post('/api/control', { id: 'tempo.bpm', value: 40 }); // a bar = 6 s
  await page.locator('#festQuant button', { hasText: 'Mesure' }).click();
  await expect.poll(async () => (await festival()).quantize).toBe('bar');
  await expect(page.locator('#festQuant button', { hasText: 'Mesure' })).toHaveClass(/active/);
  // Mid-bar, so it can't land at once.
  await expect.poll(async () => (await studio.frame()).tempo.beat_in_bar, { timeout: 10_000 }).toBe(1);
  await pill(page, 'Rouge hardstyle').click();
  await expect.poll(async () => (await festival()).pending?.palette).toBe(3);
  const at = (await festival()).pending!.at_beat;
  expect(at % 4).toBe(0);
  await expect(pill(page, 'Rouge hardstyle')).toHaveClass(/pending/);
  await expect(page.locator('#festWait')).toHaveText('attend la mesure…');
  // Watch the change: never before the « one », and within a frame or so
  // of it (40 BPM: a 60 fps frame is 1/90 beat; polling adds a little).
  let landed = -1;
  for (let i = 0; i < 400 && landed < 0; i++) {
    const st = await studio.get<{ festival: Festival; tempo: { beat: number } }>('/api/state');
    if (st.festival.active === 3) landed = st.tempo.beat;
    else {
      expect(st.festival.pending?.at_beat).toBe(at);
      await page.waitForTimeout(20);
    }
  }
  expect(landed).toBeGreaterThanOrEqual(at - 1 / 90);
  expect(landed - at).toBeLessThan(0.25);
  await expect(pill(page, 'Rouge hardstyle')).toHaveClass(/on/);
  await expect(pill(page, 'Rouge hardstyle')).not.toHaveClass(/pending/);
});

test('« Équilibre perçu » dims green and never brightens', async ({ page }) => {
  await page.locator('#festQuant button', { hasText: 'Immédiat' }).click();
  await pill(page, 'Blanc').click();
  await expect.poll(async () => (await lit()).every(p => Math.abs(p[2] - p[3]) < 1e-3)).toBe(true);
  const before = Math.max(...(await lit()).map(p => p[3]));
  await expect(page.locator('#festGains')).toBeHidden();
  await page.locator('#festBal').check();
  await expect.poll(async () => (await festival()).balance.enabled).toBe(true);
  await expect(page.locator('#festGains')).toBeVisible();
  await page.locator('#festPreset').click();
  await expect.poll(async () => (await festival()).balance.gains.map(g => Math.round(g * 100) / 100)).toEqual([1, 0.6, 1]);
  await expect.poll(async () => (await lit()).every(p => Math.abs(p[3] - 0.6 * p[2]) < 0.02)).toBe(true);
  expect(Math.max(...(await lit()).map(p => p[3]))).toBeLessThan(before);
  // Out-of-range gains from the API are pulled into 0..1.
  await studio.post('/api/festival', { quantize: 'now', balance: { enabled: true, gains: [3, 0.6, -1] } });
  expect((await festival()).balance.gains.map(g => Math.round(g * 100) / 100)).toEqual([1, 0.6, 0]);
  // Off: the white is even again.
  await page.locator('#festBal').uncheck();
  await expect.poll(async () => (await lit()).every(p => Math.abs(p[2] - p[3]) < 1e-3)).toBe(true);
});

test('the Effet tab offers « Vers le blanc » and « Chase couleur »', async ({ page }) => {
  await openWorkspace(page, 'creation');
  await page.locator('[data-kind="generator"]').click();
  await reveal(page, '#gMode');
  await page.locator('#gMode').selectOption({ label: 'Vers le blanc' });
  await expect.poll(async () => (await studio.state()).settings.content.params.color_mode).toBe('to_white');
  await page.locator('#gMode').selectOption({ label: 'Chase couleur' });
  await expect.poll(async () => (await studio.state()).settings.content.params.color_mode).toBe('color_chase');
  // The old modes are still there.
  await page.locator('#gMode').selectOption({ label: 'Alterné' });
  await expect.poll(async () => (await studio.state()).settings.content.params.color_mode).toBe('alternate');
});
