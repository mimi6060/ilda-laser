// First launch (T-437): the guided tour on a fresh data folder, and the
// empty states. The only spec without the harness's « tour seen » file.
// The tour must never arm (no POST /api/arm), must leave Échap and the
// cues working, and once finished or skipped must not come back.
// Preview only.
import { test, expect, useStudio, focusPage, type Page } from '../studio';

const studio = useStudio({ tour: true });
const bubble = (page: Page) => page.locator('#tourBubble');

/** Every POST the page sends, by path. */
function watchPosts(page: Page) {
  const posts: string[] = [];
  page.on('request', r => { if (r.method() === 'POST') posts.push(new URL(r.url()).pathname); });
  return posts;
}

test('a fresh data folder shows the tour; it never arms and leaves Échap and the cues alone', async ({ page }) => {
  expect((await studio.state()).first_run).toBe(true);
  const posts = watchPosts(page);
  await page.goto(studio.url + '/');
  await expect(bubble(page)).toBeVisible();
  const steps = await page.evaluate(() => (window as any).helpDebug.steps()) as { title: string; ws: string | null }[];
  await expect(page.locator('#tourTitle')).toHaveText('Allumer et arrêter');
  await expect(page.locator('#tourCount')).toHaveText(`1 / ${steps.length}`);
  await expect(page.locator('#tourRing')).toBeVisible();
  await expect(page.locator('#tourPrev')).toBeDisabled();

  // Not modal: a cue still plays (preview), Échap is still the e-stop.
  await expect(page.locator('#cues .cue').first()).toBeVisible();
  await page.locator('#cues .cue').first().click();
  await expect.poll(async () => (await studio.frame()).active_cue).not.toBeNull();
  await focusPage(page);
  await page.keyboard.press('Escape');
  await expect.poll(async () => (await studio.state()).estop).toBe(true);
  await expect(bubble(page)).toBeVisible();
  expect(await studio.post('/api/estop/reset', {})).toBe(200);

  // Through every step: each one names its title, shows its workspace.
  for (let i = 1; i < steps.length; i++) {
    await page.locator('#tourNext').click();
    await expect(page.locator('#tourTitle')).toHaveText(steps[i].title);
    await expect(page.locator('#tourCount')).toHaveText(`${i + 1} / ${steps.length}`);
    if (steps[i].ws) await expect(page.locator(`#wsTabs [data-ws-tab="${steps[i].ws}"]`)).toHaveClass(/active/);
  }
  await expect(page.locator('#tourSkip')).toBeHidden();
  await page.locator('#tourPrev').click();
  await expect(page.locator('#tourTitle')).toHaveText(steps[steps.length - 2].title);
  await page.locator('#tourNext').click();
  await expect(page.locator('#tourNext')).toHaveText('Terminer');
  await page.locator('#tourNext').click();
  await expect(bubble(page)).toBeHidden();
  await expect(page.locator('#tourRing')).toBeHidden();
  await expect(page.locator('#wsTabs [data-ws-tab="live"]')).toHaveClass(/active/); // back where it started
  await expect.poll(async () => (await studio.get('/api/onboarding')).tour_done).toBe(true);
  expect((await studio.state()).first_run).toBe(false);
  expect((await studio.state()).armed).toBe(false);
  expect(posts.filter(p => p === '/api/arm')).toEqual([]);

  // Seen: not on reload, nor in another browser (a new context).
  await page.reload();
  await expect(page.locator('#cues .cue').first()).toBeVisible();
  await page.waitForTimeout(1200);
  await expect(bubble(page)).toBeHidden();
  const other = await page.context().browser()!.newPage();
  await other.goto(studio.url + '/');
  await expect(other.locator('#cues .cue').first()).toBeVisible();
  await other.waitForTimeout(1200);
  await expect(other.locator('#tourBubble')).toBeHidden();
  await other.close();
});

test('« Revoir la visite » replays it and « Passer la visite » ends it', async ({ page }) => {
  const posts = watchPosts(page);
  await page.goto(studio.url + '/');
  await expect(page.locator('#cues .cue').first()).toBeVisible();
  await page.locator('#helpBtn').click();
  await page.locator('#helpMenu').getByRole('menuitem', { name: 'Revoir la visite' }).click();
  await expect(bubble(page)).toBeVisible();
  await expect(page.locator('#tourTitle')).toHaveText('Allumer et arrêter');
  await page.locator('#tourNext').click();
  await page.locator('#tourSkip').click();
  await expect(bubble(page)).toBeHidden();
  await page.reload();
  await expect(page.locator('#cues .cue').first()).toBeVisible();
  await page.waitForTimeout(1200);
  await expect(bubble(page)).toBeHidden();
  expect(posts.filter(p => p === '/api/arm')).toEqual([]);
});

test('empty lists say what they are for and offer the first action', async ({ page }) => {
  await page.goto(studio.url + '/');
  await expect(page.locator('#cues .cue').first()).toBeVisible();
  // LIVE › Scènes: one click saves the current look, the empty state goes.
  await page.locator('#liveTabs [data-panel-tab="scenes"]').click();
  const scenes = page.locator('#sceneList');
  await expect(scenes).toContainText('Une scène garde le look courant pour le rejouer.');
  await scenes.getByRole('button', { name: 'Sauver le look courant' }).click();
  await expect.poll(async () => ((await studio.state()).scenes as unknown[]).length).toBe(1);
  await expect(scenes.locator('.empty-state')).toHaveCount(0);
  await expect(scenes.locator('.scene')).toHaveCount(1);

  // LIVE › Modulateurs and Liens audio.
  await page.locator('#liveTabs [data-panel-tab="lfo"]').click();
  await expect(page.locator('#lfoHint')).toContainText('Un modulateur fait osciller un réglage');
  await expect(page.locator('#arHint')).toContainText('Un lien audio fait réagir un réglage');
  await page.locator('#lfoHintAdd').click();
  await expect(page.locator('#lfoHint')).toBeHidden();

  // TIMELINE: no show loaded.
  await page.locator('#wsTabs [data-ws-tab="timeline"]').click();
  await expect(page.locator('#tlEmpty')).toContainText('Un show enchaîne des cues dans le temps');
  await expect(page.locator('#tlEmpty').getByRole('button', { name: 'Nouveau show' })).toBeVisible();

  // CRÉATION › Figures.
  await page.locator('#wsTabs [data-ws-tab="creation"]').click();
  await page.locator('#creationTabs [data-ctab="figures"]').click();
  await expect(page.locator('#figLib')).toContainText('Dessinez vos propres figures ou importez un SVG.');
  await page.locator('#figLib').getByRole('button', { name: 'Importer…' }).click();
  await expect(page.locator('#figImportDlg')).toBeVisible();
  await page.locator('#impClose').click();

  // RÉGLAGES › Contrôleur: the learn mode in one click.
  await page.locator('#wsTabs [data-ws-tab="settings"]').click();
  await page.locator('#midiPanel summary').click();
  const maps = page.locator('#midiMappings');
  await expect(maps).toContainText('Reliez un bouton ou un curseur de votre contrôleur à un réglage');
  await maps.getByRole('button', { name: 'Apprendre MIDI' }).click();
  await expect(page.locator('#midiLearnMode')).toHaveText('Terminer');
  await page.locator('#midiLearnMode').click();
  expect((await studio.state()).armed).toBe(false);
});
