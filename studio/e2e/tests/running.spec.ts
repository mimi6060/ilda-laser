// « En cours » (T-423): what plays, who drove it, and the stop buttons, in
// the LIVE panel. Preview only; the simulated APC40 (--midi-test) stands
// in for a controller, nothing is ever armed.
import { test, expect, useStudio, openUi, focusPage } from '../studio';
import { Apc, testProfileFiles, TEST_PORT } from '../midi';

const studio = useStudio({ midiTest: true, files: testProfileFiles() });
const apc = new Apc(studio);

interface Running {
  cues: { instance: number | null; cue: string; name: string; page: number | null; source_label: string; protect_stop: boolean; phase: string }[];
  timeline: unknown;
  masters: { id: string; source_label: string | null }[];
}
const running = () => studio.get<Running>('/api/running');
const playing = async () => (await running()).cues.map(c => c.cue);

test.beforeAll(async () => {
  await expect.poll(async () => {
    const d = (await studio.get('/api/midi')).devices.find((x: { name: string }) => x.name === TEST_PORT);
    return d && [d.connected, d.profile];
  }).toEqual([true, 'e2e-apc']);
});

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await studio.post('/api/running/stop', { kind: 'all' });
  await studio.post('/api/running/stop', { kind: 'masters' });
  await studio.post('/api/control', { id: 'page.1' });
  await studio.post('/api/control', { id: 'cue.multi', value: true });
  await openUi(page, studio);
});

test('a cue started on page 3 shows in « En cours » from page 1, is located there and stopped', async ({ page }) => {
  await page.locator('#cueTabs button').nth(2).click();
  const cell = page.locator('#cues .cue').first();
  const id = await cell.getAttribute('data-id');
  const name = (await cell.locator('.n').textContent()) ?? '';
  await cell.click();
  await expect.poll(playing).toEqual([id]);
  await page.locator('#cueTabs button').nth(0).click();
  await expect(page.locator('#runCount')).toHaveText('1');

  await page.locator('#liveTabs [data-panel-tab="running"]').click();
  const row = page.locator('#runCues .run-row');
  await expect(row).toHaveCount(1);
  await expect(row.locator('.n')).toHaveText(name);
  await expect(row.locator('.d')).toContainText('p. 3');
  await expect(row.locator('.src')).toHaveText('Interface');
  await expect(page.locator('#runTimeline')).toHaveText('Rien ne joue');

  // Clicking the name shows its page and rings its cell, without playing anything else.
  await row.locator('.n').click();
  await expect(page.locator('#cueTabs button').nth(2)).toHaveClass(/active/);
  await expect(page.locator(`#cues .cue[data-id="${id}"]`)).toHaveClass(/located/);
  expect(await playing()).toEqual([id]);

  await row.getByRole('button', { name: 'Arrêter' }).click();
  await expect.poll(playing).toEqual([]);
  await expect(page.locator('#runCues')).toHaveText('Rien ne joue');
  await expect(page.locator('#runCount')).toBeHidden();
});

test('« Tout arrêter » leaves a protected cue running, also in show mode; its own stop ends it', async ({ page }) => {
  const cells = page.locator('#cues .cue');
  const keep = (await cells.nth(0).getAttribute('data-id'))!;
  const other = (await cells.nth(1).getAttribute('data-id'))!;
  await cells.nth(0).click({ button: 'right' });
  await page.locator('#cueMenuProtStop').check();
  await expect.poll(async () => (await studio.get('/api/cues')).slots[keep]?.protect_stop).toBe(true);
  await page.locator('#cueMenuClose').click();
  await expect(cells.nth(0).locator('svg.lock')).toBeVisible();

  await cells.nth(0).click();
  await cells.nth(1).click();
  await expect.poll(async () => (await playing()).sort()).toEqual([keep, other].sort());

  // Show mode: the panel stays, its stop buttons work, nothing arms.
  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  await expect(page.locator('body')).toHaveClass(/show-mode/);
  await page.locator('#liveTabs [data-panel-tab="running"]').click();
  await expect(page.locator('#runPanel')).toBeVisible();
  await page.locator('#runAll').click();
  await expect.poll(playing).toEqual([keep]);
  await expect(page.locator('#runCues .run-row svg')).toHaveCount(1);
  await page.locator('#runCues .run-row').getByRole('button', { name: 'Arrêter' }).click();
  await expect.poll(playing).toEqual([]);
  expect((await studio.frame()).armed).toBe(false);
  expect(await studio.post('/api/lock', { show_mode: false })).toBe(200);
});

test('the source column reads « MIDI » after a fader, « Interface » after the mouse; reset empties « Maîtres »', async ({ page }) => {
  await page.locator('#liveTabs [data-panel-tab="running"]').click();
  await expect(page.locator('#runMasters')).toHaveText('Rien ne joue');
  // Master fader → master.brightness, with pickup: caught at the top first.
  await apc.moveFader(8, 127);
  await apc.moveFader(8, 40);
  await expect.poll(async () => (await studio.live()).brightness).toBeLessThan(0.5);
  const row = page.locator('#runMasters .run-row', { hasText: 'Luminosité maître' });
  await expect(row.locator('.src')).toHaveText('MIDI');
  await page.locator('#topMaster').fill('60');
  await expect(row.locator('.src')).toHaveText('Interface');
  expect((await running()).masters.find(m => m.id === 'master.brightness')?.source_label).toBe('Interface');

  await page.locator('#runPanel [data-run-stop="masters"]').click();
  await expect(page.locator('#runMasters')).toHaveText('Rien ne joue');
  expect((await studio.live()).brightness).toBe(1);
});

test('a quantised launch shows « en attente » and can be cancelled before its bar', async ({ page }) => {
  // A slow tempo and « Mesure »: a bar lasts 8 s.
  expect(await studio.post('/api/control', { id: 'tempo.bpm', value: 30 })).toBe(200);
  expect(await studio.post('/api/control', { id: 'cue.quantize', value: 2 })).toBe(200);
  try {
    await page.locator('#liveTabs [data-panel-tab="running"]').click();
    // T-480: click at a known place in the bar, never a random one. Put the
    // « 1 » now, then wait past the catch-up window (1/8 beat = 250 ms at
    // 30 BPM, where a press still starts on the bar it just missed): the
    // launch always waits, and its bar is at least 5 s away, enough to
    // check the row and cancel it even on a loaded machine.
    expect(await studio.post('/api/control', { id: 'tempo.resync' })).toBe(200);
    await expect.poll(async () => {
      const t = (await studio.frame()).tempo;
      const inBar = t.beat_in_bar + t.phase;
      return inBar >= 0.25 && inBar <= 1.5;
    }, { intervals: [50] }).toBe(true);
    const cell = page.locator('#cues .cue').nth(3);
    const id = await cell.getAttribute('data-id');
    await cell.click();
    await expect.poll(async () => (await running()).cues.map(c => [c.cue, c.phase])).toEqual([[id, 'pending']]);
    const row = page.locator('#runCues .run-row', { hasText: 'en attente' });
    await expect(row).toHaveCount(1);
    await expect(row.locator('.src')).toHaveText('Interface');
    await row.getByRole('button', { name: 'Arrêter' }).click();
    await expect(page.locator('#runCues')).toHaveText('Rien ne joue');
    await expect(page.locator('#runMsg')).toHaveText('');
    await page.waitForTimeout(300);
    expect(await playing()).not.toContain(id);
  } finally {
    await studio.post('/api/control', { id: 'cue.quantize', value: 0 });
    await studio.post('/api/control', { id: 'tempo.bpm', value: 120 });
  }
});

test('fits a 1280 px window without horizontal scrolling', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.locator('#liveTabs [data-panel-tab="running"]').click();
  await focusPage(page);
  await expect(page.locator('#runAll')).toBeVisible();
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(overflow).toBeLessThanOrEqual(0);
  const tabs = await page.locator('#liveTabs').evaluate(el => el.scrollWidth - el.clientWidth);
  expect(tabs).toBeLessThanOrEqual(0);
});
