// MIDI learn (T-203): right click a control → « Apprendre MIDI » → the next
// message of the simulated APC40 mkII (--midi-test) is mapped in its
// profile. The device starts on the built-in « apc40-mk2 » layout (T-204),
// which must never change: learning writes « apc40-mk2-perso ».
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { test, expect, useStudio, openUi, focusPage } from '../studio';
import { Apc, CC_CUE_LEVEL, TEST_PORT } from '../midi';

const studio = useStudio({ midiTest: true });
const apc = new Apc(studio);

interface Mapping { port: string; profile: string; builtin: boolean; index: number; message: string; channel: number | null; target: string; target_label: string; mode: string; shift: boolean }
const midi = () => studio.get('/api/midi');
const mappings = async () => (await midi()).mappings as Mapping[];
const size = async () => (await studio.live()).size;
const cc = (number: number, value: number, channel = 0) => apc.inject([0xb0 | channel, number, value]);

/** Empties the port's profile (a built-in one is first copied to -perso). */
async function clearMappings() {
  const maps = await mappings();
  for (const mp of maps.sort((a, b) => b.index - a.index)) {
    expect(await studio.post('/api/midi/mapping/delete', { port: mp.port, index: mp.index })).toBe(200);
  }
}

async function learnFromMenu(page: import('@playwright/test').Page, selector: string, item = '#midiMenuLearn') {
  await page.locator(selector).click({ button: 'right' });
  await expect(page.locator('#midiMenu')).toBeVisible();
  await page.locator(item).click();
  await expect(page.locator('#learnBanner')).toContainText('touchez un bouton, un potard ou un fader');
}

test.beforeAll(async () => {
  await expect.poll(async () => {
    const d = (await midi()).devices.find((x: { name: string }) => x.name === TEST_PORT);
    return d && [d.connected, d.model];
  }).toEqual([true, 'Apc40Mk2']);
});

test.beforeEach(async ({ page }) => {
  await studio.post('/api/midi/learn/cancel');
  await studio.post('/api/estop/reset', {});
  await studio.reset();
  await studio.post('/api/control', { id: 'page.1' });
  await studio.post('/api/midi/safety', { allow_arm: false });
  // The first test learns on top of the untouched built-in layout.
  if (!test.info().title.startsWith('right click')) await clearMappings();
  await openUi(page, studio);
});

test('right click « Taille » → Apprendre MIDI → the knob drives the size, in a -perso profile, after a restart too', async ({ page }) => {
  expect(await size()).toBe(1); // fader position 50 %
  const before = await mappings();
  const layout = before.length;
  expect(before.every(m => m.builtin && m.profile === 'apc40-mk2')).toBe(true);
  await learnFromMenu(page, '#mSize');
  expect((await midi()).learn).toMatchObject({ target: 'master.size', target_label: 'Taille maître' });

  // Device knob 1 (CC 16), which the layout leaves free.
  await cc(0x10, 20); // the learned message itself does nothing
  await expect(page.locator('#learnBanner')).toContainText('CC 16 · can. 1 → Taille maître (Absolu)');
  expect(await size()).toBe(1);
  const all = await mappings();
  expect(all).toHaveLength(layout + 1); // the layout is copied along
  expect(all.find(m => m.message === 'CC 16')).toMatchObject({ port: TEST_PORT, profile: 'apc40-mk2-perso', builtin: false, message: 'CC 16', channel: 0, mode: 'absolute', shift: false });

  // Pickup: turning on past 50 % takes over, then the knob drives it.
  await cc(0x10, 90);
  await expect.poll(size).toBeCloseTo(2 * 90 / 127, 3);
  await cc(0x10, 40);
  await expect.poll(size).toBeCloseTo(2 * 40 / 127, 3);
  await expect.poll(async () => +await page.locator('#mSize').inputValue()).toBeCloseTo(200 * 40 / 127, 0); // the slider moves

  // Listed in the Contrôleur section.
  await page.locator('#midiPanel summary').click();
  await expect(page.locator('#midiMappings')).toContainText('CC 16 · can. 1');
  await expect(page.locator('#midiMappings')).toContainText('Taille maître');

  // Built-in profile untouched; the copy is a file in the data dir.
  const file = path.join(studio.dataDir, 'midi', 'profiles', 'apc40-mk2-perso.json');
  expect(existsSync(file)).toBe(true);
  expect(JSON.parse(readFileSync(file, 'utf8')).mappings).toHaveLength(layout + 1);
  expect(existsSync(path.join(studio.dataDir, 'midi', 'profiles', 'apc40-mk2.json'))).toBe(false);

  await studio.restart();
  await expect.poll(async () => (await mappings()).filter(m => m.message === 'CC 16').map(m => [m.profile, m.target]))
    .toEqual([['apc40-mk2-perso', 'master.size']]);
  await studio.post('/api/control', { id: 'master.size', value: 1 });
  await cc(0x10, 60);
  await cc(0x10, 70); // crosses 50 %
  await expect.poll(size).toBeCloseTo(2 * 70 / 127, 3);
});

test('an encoder is learned as relative; an existing mapping is only replaced after « Oui »', async ({ page }) => {
  await learnFromMenu(page, '#mPosX');
  await apc.turnKnob(CC_CUE_LEVEL, 1); // Cue Level: an encoder in the APC profile
  await expect.poll(async () => (await mappings()).map(m => m.mode)).toEqual(['relative']);
  expect((await studio.live()).pos_x).toBe(0);
  await apc.turnKnob(CC_CUE_LEVEL, 3);
  await expect.poll(async () => (await studio.live()).pos_x).toBeCloseTo(3 * 2 / 127, 4);

  // The same knob for « Vitesse d'animation »: asks first.
  await learnFromMenu(page, '#mSpeed');
  await apc.turnKnob(CC_CUE_LEVEL, 1);
  await expect(page.locator('#learnBanner')).toContainText('Remplacer l\'ancienne affectation (Position X)');
  await page.locator('#learnNo').click();
  await expect.poll(async () => (await mappings()).map(m => m.target)).toEqual(['master.pos_x']);
  await expect.poll(async () => (await midi()).learn_conflict).toBeNull();

  await learnFromMenu(page, '#mSpeed');
  await apc.turnKnob(CC_CUE_LEVEL, 1);
  await page.locator('#learnYes').click();
  await expect.poll(async () => (await mappings()).map(m => m.target)).toEqual(['master.speed']);
  await expect(page.locator('#learnBanner')).toContainText('remplace Position X');
});

test('« Oublier MIDI » and « Supprimer » remove mappings; Échap cancels learning and still stops', async ({ page }) => {
  await learnFromMenu(page, '#mSize');
  await cc(0x07, 11);
  await learnFromMenu(page, '#mBright');
  await cc(0x0e, 12, 0);
  await expect.poll(async () => (await mappings()).length).toBe(2);

  await expect(page.locator('#mSize')).toHaveAttribute('data-midi', /CC 7/); // the page has seen it
  await page.locator('#mSize').click({ button: 'right' });
  await expect(page.locator('#midiMenuMaps')).toContainText('CC 7');
  await page.locator('#midiMenuForget').click();
  await expect.poll(async () => (await mappings()).map(m => m.target)).toEqual(['master.brightness']);

  await page.locator('#midiPanel summary').click();
  await page.locator('#midiMappings [data-mdel="0"]').click();
  await expect.poll(async () => (await mappings()).length).toBe(0);
  await expect(page.locator('#midiMappings')).toContainText('Aucune affectation');

  await learnFromMenu(page, '#mSize');
  await focusPage(page);
  await page.keyboard.press('Escape');
  await expect.poll(async () => (await midi()).learn).toBeNull();
  expect((await studio.state()).estop).toBe(true); // Échap is still the emergency stop
  await expect(page.locator('#learnBanner')).toContainText('Apprentissage MIDI annulé');
  await cc(0x07, 13);
  await new Promise(r => setTimeout(r, 300));
  expect(await mappings()).toEqual([]);
});

test('learn mode: click a cue, press a pad, the pad plays that cue', async ({ page }) => {
  const catalog = await studio.presets();
  const cue = catalog.presets.filter(p => p.category === catalog.categories[0])[2];
  await page.locator('#midiPanel summary').click();
  await page.locator('#midiLearnMode').click();
  await expect(page.locator('body')).toHaveClass(/learning/);
  await page.locator(`#cues .cue[data-id="${cue.id}"]`).click();
  await expect.poll(async () => (await midi()).learn?.target).toBe('grid.1.1.3');
  expect((await studio.controlValues()).active_cue).toBeNull(); // picked, not played
  await apc.pressPad(3, 3);
  await apc.releasePad(3, 3);
  await expect.poll(async () => (await mappings()).map(m => [m.message, m.target, m.mode])).toEqual([['Note 11', 'grid.1.1.3', 'momentary']]);
  expect((await studio.controlValues()).active_cue).toBeNull(); // the learned press didn't play it
  await expect(page.locator(`#cues .cue[data-id="${cue.id}"]`)).toHaveAttribute('data-midi-pill', 'Note 11 · can. 1');
  await page.locator('#midiLearnMode').click(); // « Terminer »
  await expect(page.locator('body')).not.toHaveClass(/learning/);

  await apc.pressPad(3, 3);
  await expect.poll(async () => (await studio.controlValues()).active_cue).toBe(cue.id);
  await apc.releasePad(3, 3);
});

test('transport.arm: refused without the MIDI arming option; learning never arms or disarms', async ({ page }) => {
  await page.locator('#armBtn').click({ button: 'right' });
  await page.locator('#midiMenuLearn').click();
  await expect(page.locator('#learnBanner')).toContainText('Autoriser l\'armement depuis le contrôleur');
  expect((await midi()).learn).toBeNull();
  expect(await studio.post('/api/midi/learn', { target: 'transport.arm' })).toBe(403);

  // With the option: always a Shift mapping, and learning it doesn't arm.
  expect(await studio.post('/api/midi/safety', { allow_arm: true })).toBe(200);
  expect(await studio.post('/api/midi/learn', { target: 'transport.arm' })).toBe(200);
  expect((await midi()).learn).toMatchObject({ target: 'transport.arm', shift: true });
  await apc.button(0x5b, true);
  await new Promise(r => setTimeout(r, 1_200)); // held longer than the arming gesture
  await apc.button(0x5b, false);
  await expect.poll(async () => (await mappings()).map(m => [m.message, m.target, m.shift])).toEqual([['Note 91', 'transport.arm', true]]);
  expect((await studio.state()).armed).toBe(false);

  // Armed by hand: learning a button keeps it armed.
  expect(await studio.post('/api/arm', { on: true })).toBe(200);
  expect(await studio.post('/api/midi/learn', { target: 'tempo.tap' })).toBe(200);
  await apc.button(0x5c, true);
  await apc.button(0x5c, false);
  await expect.poll(async () => (await mappings()).map(m => m.target)).toEqual(['transport.arm', 'tempo.tap']);
  expect((await studio.state()).armed).toBe(true);
  expect(await studio.post('/api/arm', { on: false })).toBe(200);
});
