// Horloge MIDI (T-207): a simulated deck plugged with --midi-test plays a
// MIDI clock (POST /api/midi/clock/simulate: each tick at its time, through
// the real worker). The tempo follows it, Tap is refused meanwhile, losing
// the clock goes back to manual without a jump. The clock output is off by
// default, only goes to the port picked in RÉGLAGES › Horloge MIDI, and is
// refused on the followed port. Simulator only: never a real MIDI port.
import { test, expect, useStudio, openUi, reveal } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio({ midiTest: true });
const DECK = 'Test Platine';
const BOX = 'Test Boîte à rythmes';

interface Sent { port: string; sent: number[][]; clock_ticks: number }
const tempo = async () => (await studio.get('/api/state')).tempo;
const clock = () => studio.get('/api/midi/clock');
const sentTo = async (port: string) => ((await studio.get('/api/midi/sent')).devices as Sent[]).find(d => d.port === port)!;

async function openPanel(page: Page) {
  await reveal(page, '#midiClockPanel summary');
  if (!await page.locator('#midiClockPanel').evaluate(e => (e as HTMLDetailsElement).open)) await page.locator('#midiClockPanel summary').click();
}

test.beforeAll(async () => {
  for (const port of [DECK, BOX]) expect(await studio.post('/api/midi/plug', { port })).toBe(200);
  await expect.poll(async () => (await clock()).inputs, { timeout: 8000 }).toEqual(expect.arrayContaining([DECK, BOX]));
});

test.beforeEach(async ({ page }) => {
  expect(await studio.post('/api/midi/clock', { follow: false, send: false })).toBe(200);
  await studio.reset();
  expect(await studio.post('/api/control', { id: 'tempo.bpm', value: 100 })).toBe(200);
  await openUi(page, studio);
});

test('a deck clock sets the tempo, Tap is refused, losing it goes back to manual', async ({ page }) => {
  await expect(page.locator('#midiClockHint')).toBeHidden();
  await openPanel(page);
  await expect(page.locator('#mclkSource option', { hasText: `Horloge MIDI (${DECK})` })).toHaveCount(1);
  await page.locator('#mclkSource').selectOption(DECK);
  await expect.poll(async () => (await clock()).config).toMatchObject({ follow: true, in_port: DECK });
  await expect(page.locator('#midiClockHint')).toHaveText('Horloge MIDI : en attente');

  // 96 ticks at 120 BPM (4 beats, 2 s), Start first.
  expect(await studio.post('/api/midi/clock/simulate', { port: DECK, bpm: 120, beats: 4, start: true })).toBe(200);
  await expect.poll(async () => (await tempo()).source, { timeout: 5000 }).toBe('midi_clock');
  await expect.poll(async () => Math.abs((await tempo()).bpm - 120), { timeout: 5000 }).toBeLessThan(0.5);
  await expect(page.locator('#midiClockHint')).toHaveText(/^Horloge MIDI : 1(19|20),\d BPM$/);
  await expect(page.locator('#mclkInState')).toContainText('tops reçus');

  // Tap is the clock's job now: refused, and the tempo does not move.
  expect(await studio.post('/api/control', { id: 'tempo.tap' })).toBe(403);
  expect((await tempo()).source).toBe('midi_clock');

  // The deck stops sending: 2 s later, manual tempo at the same BPM.
  await expect.poll(async () => (await clock()).input.ticks, { timeout: 6000 }).toBe(96);
  const bpm = (await tempo()).bpm;
  await expect.poll(async () => (await tempo()).source, { timeout: 6000 }).toBe('manual');
  expect((await tempo()).bpm).toBe(bpm);
  await expect(page.locator('#midiClockHint')).toHaveText('Horloge MIDI perdue : tempo manuel');
  expect(await studio.post('/api/control', { id: 'tempo.tap' })).toBe(200);
  expect((await studio.get('/api/state')).armed).toBe(false);

  // Back to manual from the menu: the hint goes away.
  await page.locator('#mclkSource').selectOption('');
  await expect(page.locator('#midiClockHint')).toBeHidden();
});

test('the clock output is off by default, goes to the picked port and never to the followed one', async ({ page }) => {
  const c = await clock();
  expect(c.config.send).toBe(false);
  expect(c.output.open).toBeNull();
  const before = (await sentTo(BOX)).clock_ticks;
  await page.waitForTimeout(300);
  expect((await sentTo(BOX)).clock_ticks).toBe(before);

  await openPanel(page);
  await expect(page.locator('#mclkSend')).not.toBeChecked();
  await expect(page.locator('#mclkOutState')).toHaveText('Horloge non envoyée.');
  await page.locator('#mclkSource').selectOption(DECK);
  await page.locator('#mclkOut').selectOption(BOX);
  await page.locator('#mclkSend').check();
  await expect.poll(async () => (await clock()).config).toMatchObject({ send: true, out_port: BOX });
  // 24 ticks a beat (100 BPM: 40 a second), Start on the bar line.
  await expect.poll(async () => (await sentTo(BOX)).clock_ticks - before, { timeout: 5000 }).toBeGreaterThan(48);
  await expect.poll(async () => (await sentTo(BOX)).sent.some(m => m.length === 1 && m[0] === 0xfa), { timeout: 5000 }).toBe(true);
  await expect(page.locator('#mclkOutState')).toContainText(`Envoyée sur ${BOX}`);

  // The followed port is refused: the studio would follow itself.
  await page.locator('#mclkOut').selectOption(DECK);
  await expect(page.locator('#mclkError')).toContainText('le studio suivrait sa propre horloge');
  expect(await studio.post('/api/midi/clock', { out_port: DECK })).toBe(409);
  expect((await clock()).config.out_port).toBe(BOX);

  // Off: Stop, and no more ticks.
  await page.locator('#mclkSend').uncheck();
  await expect.poll(async () => JSON.stringify((await sentTo(BOX)).sent.slice(-1)), { timeout: 5000 }).toBe('[[252]]');
  const after = (await sentTo(BOX)).clock_ticks;
  await page.waitForTimeout(300);
  expect((await sentTo(BOX)).clock_ticks).toBe(after);
  expect((await studio.get('/api/state')).armed).toBe(false);
});
