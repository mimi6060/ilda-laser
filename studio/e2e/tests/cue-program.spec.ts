// Cue programs (T-157): a cue as a mini-timeline. Cues with a program
// carry a « ∿ » badge, play it in their picture on hover, play it on the
// preview when clicked, and a program can be set on a cue of the grid
// through POST /api/cues/program. Preview only: nothing is ever armed.
import { test, expect, useStudio, openUi } from '../studio';

const studio = useStudio();

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await openUi(page, studio);
});

test('a program cue has a badge, plays on hover and when clicked', async ({ page }) => {
  await page.locator('#cueTabs button', { hasText: 'Vagues' }).click();
  const helix = page.locator('#cues .cue[data-id="vagues-028"]');
  await expect(helix.locator('.prog')).toHaveText('∿');
  await expect(page.locator('#cues .cue[data-id="vagues-001"] .prog')).toHaveCount(0);

  // Hover: the picture plays the program, then goes back to its still.
  const preview = page.waitForResponse(r => r.url().includes('/api/presets/preview?id=vagues-028'));
  await helix.hover();
  const body = await (await preview).json();
  expect(body.frames.length).toBe(32);
  await expect(helix).toHaveClass(/previewing/);
  await page.locator('h1').hover();
  await expect(helix).not.toHaveClass(/previewing/);

  // Clicked: the look carries the program and the progress bar follows it.
  await helix.click();
  await expect.poll(async () => (await studio.state()).settings.program?.play).toBe('ping_pong');
  await expect(page.locator('#evoBar')).toBeVisible();
  await expect(page.locator('#evoState')).toContainText('programme');
  await expect.poll(async () => (await studio.frame()).points.length).toBeGreaterThan(0);
  expect((await studio.state()).armed).toBe(false);
});

test('a program set on a grid cue shows its badge and plays with it', async ({ page }) => {
  const program = {
    play: 'once',
    layers: [{ kind: 'shape', shape: 'circle' }],
    lanes: [{ target: 'size', keys: [{ beat: 0, value: 0.3 }, { beat: 4, value: 1 }] }],
  };
  try {
    expect(await studio.post('/api/cues/program', { id: 'tunnels-001', program })).toBe(200);
    expect(await studio.post('/api/cues/program', { id: 'tunnels-001', program: { lanes: [{ target: 'mirror' }] } })).toBe(400);
    await openUi(page, studio);
    await page.locator('#cueTabs button', { hasText: 'Tunnels' }).click();
    const cue = page.locator('#cues .cue[data-id="tunnels-001"]');
    await expect(cue.locator('.prog')).toHaveText('∿');
    await cue.click();
    await expect.poll(async () => (await studio.state()).settings.program?.play).toBe('once');
    expect((await studio.state()).armed).toBe(false);
  } finally {
    await studio.post('/api/cues/program', { id: 'tunnels-001', program: null });
  }
});
