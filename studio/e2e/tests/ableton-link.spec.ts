// Ableton Link (T-154): the e2e studio is a default build, without the
// `ableton-link` feature (Ableton's Link library is GPL): RÉGLAGES ›
// Ableton Link is greyed and says why, POST /api/link refuses to turn it
// on (501), and the tempo stays the studio's. Nothing here ever reaches
// the network: the default build has no Link code at all.
import { test, expect, useStudio, openUi, reveal } from '../studio';

const studio = useStudio();

test('Ableton Link is greyed in a build without it, and never turns on', async ({ page }) => {
  await studio.reset();
  await openUi(page, studio);
  const link = await studio.get('/api/link');
  expect(link).toMatchObject({ available: false, enabled: false, peers: 0, state: 'off', quantum: 4 });

  await reveal(page, '#linkPanel summary');
  await page.locator('#linkPanel summary').click();
  await expect(page.locator('#linkState')).toContainText('n\'est pas compilé');
  await expect(page.locator('#linkOn')).toBeDisabled();
  await expect(page.locator('#linkQuantum')).toBeDisabled();
  await expect(page.locator('#linkOnState')).toHaveText('Désactivé');

  expect(await studio.post('/api/link', { enabled: true })).toBe(501);
  expect(await studio.post('/api/link', { bogus: 1 })).toBe(400);
  const state = await studio.get('/api/state');
  expect(state.tempo.source).toBe('manual');
  expect(state.tempo.link).toMatchObject({ enabled: false, peers: 0 });
  await expect(page.locator('#linkHint')).toBeHidden();
});
