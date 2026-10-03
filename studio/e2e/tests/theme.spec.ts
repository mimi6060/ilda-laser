// Lighting-desk theme (T-299): a picture per cue, a colour per cue page.
import { test, expect, useStudio, openUi } from '../studio';

const studio = useStudio();

test('every cue has a thumbnail, packed as hex', async () => {
  const { presets } = await studio.get('/api/presets');
  const thumbs = await studio.get('/api/presets/thumbs');
  for (const p of presets) expect(thumbs[p.id], p.id).toMatch(/^([0-9a-f]{10})+$/);
});

test('the cue tiles show their picture and the page colour', async ({ page }) => {
  await openUi(page, studio);
  const tile = page.locator('#cues .cue').first();
  await expect(tile.locator('canvas.th')).toBeVisible();
  // Something lit was drawn on the picture.
  await expect.poll(() => tile.locator('canvas.th').evaluate((c: HTMLCanvasElement) => {
    const d = c.getContext('2d')!.getImageData(0, 0, c.width, c.height).data;
    for (let i = 0; i < d.length; i += 4) if (d[i] + d[i + 1] + d[i + 2] > 60) return true;
    return false;
  })).toBe(true);
  const cat = await page.locator('#cueRegion').evaluate(e => e.style.getPropertyValue('--cat'));
  expect(cat).toMatch(/^#[0-9a-f]{6}$/);
  // The cue's name is still its text, the picture adds none.
  await expect(tile).toContainText('Lissajous');
});
