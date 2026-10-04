// Cue lists with « Go » (T-410), preview only: a list built in LIVE ›
// Liste de cues, GO / Page suivante step through it, an « À la suite » step
// starts by itself, Pause holds a fade, show mode keeps GO but refuses
// edits, and a pad learned on list.go (simulated APC40, --midi-test)
// advances the list. Nothing here ever arms.
import { test, expect, useStudio, openUi, reveal, focusPage, isLit } from '../studio';
import { Apc } from '../midi';

const studio = useStudio({ midiTest: true });
const apc = new Apc(studio);

interface ListView { name: string; index: number | null; paused: boolean; progress: number | null; phase: { phase: string }; next: number | null; source_label: string }
const player = async () => ((await studio.frame()) as unknown as { cuelist: ListView | null }).cuelist;
const lists = async () => (await studio.get('/api/cuelists')).lists as { name: string; steps: { source: { type: string; id?: string }; trigger: { type: string }; fade_in_s: number }[] }[];

test.beforeEach(async ({ page }) => {
  await studio.post('/api/lock', { show_mode: false, source: 'ui' });
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  for (const l of await lists()) await studio.post('/api/cuelists/delete', { name: l.name });
  await openUi(page, studio);
});

test('build a three-step list, GO, Page suivante, an « À la suite » step and Pause', async ({ page }) => {
  const { presets } = await studio.presets();
  await reveal(page, '#cqNew');
  page.once('dialog', d => d.accept('Acte 1'));
  await page.locator('#cqNew').click();
  await expect(page.locator('#cqSelect')).toHaveValue('Acte 1');
  for (let i = 0; i < 3; i++) await page.locator('#cqAdd').click();
  const steps = page.locator('#cqSteps .cq-step');
  await expect(steps).toHaveCount(3);
  await steps.nth(0).locator('.cq-src').selectOption('cue:' + presets[0].id);
  await steps.nth(1).locator('.cq-src').selectOption('cue:' + presets[1].id);
  await steps.nth(1).locator('.cq-in').fill('3');
  await steps.nth(1).locator('.cq-in').dispatchEvent('change');
  await steps.nth(2).locator('.cq-trig').selectOption('follow');
  await expect.poll(async () => (await lists())[0]?.steps.map(s => [s.source.type, s.trigger.type, s.fade_in_s]))
    .toEqual([['cue', 'go', 1], ['cue', 'go', 3], ['black', 'follow', 1]]);

  await page.locator('#cqGo').click();
  await expect.poll(async () => (await player())?.index).toBe(0);
  await expect.poll(async () => (await studio.frame()).points.some(isLit)).toBe(true);
  await expect(page.locator('#cqSteps .cq-step').nth(0)).toHaveClass(/current/);
  await expect(page.locator('#cqStatus')).toContainText('Pas 1/3');

  // Page suivante (a presentation remote) = GO: step 2, fading in over 3 s.
  await focusPage(page);
  await page.keyboard.press('PageDown');
  await expect.poll(async () => (await player())?.index).toBe(1);
  expect((await player())?.source_label).toBe('Clavier');

  // Pause holds the fade where it is; GO resumes it.
  await page.locator('#cqPause').click();
  await expect.poll(async () => (await player())?.paused).toBe(true);
  const held = (await player())!.progress;
  expect(held).not.toBeNull();
  await page.waitForTimeout(600);
  expect((await player())!.progress).toBe(held);
  expect((await player())!.index).toBe(1);
  await page.locator('#cqGo').click();
  await expect.poll(async () => (await player())?.paused).toBe(false);
  expect((await player())!.index).toBe(1);

  // The « À la suite » step starts by itself once the 3 s fade is over.
  await expect.poll(async () => (await player())?.index, { timeout: 8_000 }).toBe(2);
  // Début: before the first step.
  await page.locator('#cqTop').click();
  await expect.poll(async () => (await player())?.index).toBeNull();
  expect((await studio.state()).armed).toBe(false);
});

test('show mode: GO plays, editing the list is refused; « En cours » lists it', async ({ page }) => {
  const { presets } = await studio.presets();
  const list = { name: 'Spectacle', steps: [{ source: { type: 'cue', id: presets[0].id } }, { source: { type: 'cue', id: presets[1].id } }] };
  expect(await studio.post('/api/cuelists', list)).toBe(200);
  expect(await studio.post('/api/cuelist/load', { name: 'Spectacle' })).toBe(200);
  expect(await studio.post('/api/lock', { show_mode: true, source: 'ui' })).toBe(200);
  await reveal(page, '#cqGo');
  await expect(page.locator('#cqAdd')).toBeHidden();
  await page.locator('#cqGo').click();
  await expect.poll(async () => (await player())?.index).toBe(0);
  expect(await studio.post('/api/cuelists', { ...list, steps: [] })).toBe(423);
  expect(await studio.post('/api/cuelist/goto', { step: 1 })).toBe(200);
  const running = await studio.get('/api/running');
  expect(running.cuelist).toMatchObject({ name: 'Spectacle', index: 1 });
  // A step may never fire the arm control (refused at save).
  expect(await studio.post('/api/lock', { show_mode: false, source: 'ui' })).toBe(200);
  expect(await studio.post('/api/cuelists', { name: 'Armer', steps: [{ fire: { control: 'transport.arm', value: 1 } }] })).toBe(400);
  expect((await studio.state()).armed).toBe(false);
});

test('a pad learned on list.go advances the list; Échap pauses it', async ({ page }) => {
  const { presets } = await studio.presets();
  const list = { name: 'Midi', steps: [{ source: { type: 'cue', id: presets[0].id } }, { source: { type: 'cue', id: presets[1].id } }] };
  expect(await studio.post('/api/cuelists', list)).toBe(200);
  expect(await studio.post('/api/cuelist/load', { name: 'Midi' })).toBe(200);
  expect(await studio.post('/api/midi/learn', { target: 'list.go' })).toBe(200);
  await apc.button(0x5c, true);
  await apc.button(0x5c, false);
  await expect.poll(async () => ((await studio.get('/api/midi')).mappings as { target: string; message: string }[]).some(m => m.target === 'list.go' && m.message === 'Note 92')).toBe(true);
  expect((await player())?.index).toBeNull(); // the learned message itself does nothing
  await apc.button(0x5c, true);
  await apc.button(0x5c, false);
  await expect.poll(async () => (await player())?.index).toBe(0);
  expect((await player())?.source_label).toBe('MIDI');
  await apc.button(0x5c, true);
  await apc.button(0x5c, false);
  await expect.poll(async () => (await player())?.index).toBe(1);

  // Échap: the e-stop pauses the list, and GO waits for the reset.
  await focusPage(page);
  await page.keyboard.press('Escape');
  await expect.poll(async () => (await player())?.paused).toBe(true);
  expect(await studio.post('/api/cuelist/go')).toBe(409);
  expect((await studio.state()).armed).toBe(false);
});
