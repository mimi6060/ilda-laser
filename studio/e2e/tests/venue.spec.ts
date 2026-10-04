// Venue and several projectors (T-277): RÉGLAGES › Lieu et projecteurs
// adds, duplicates and removes projectors, gives one a (fake) output of its
// own, and every projector's frame comes back from /api/frames. One arm and
// one e-stop for all; changing a projector's output is refused while armed.
// Fake outputs only (--test-output for the main projector, --test-devices
// for the others): never --device, never a laser.
import { test, expect, useStudio, openUi, reveal, isLit } from '../studio';

const DEVS = [
  { kind: 'etherdream', id: 'etherdream:jardin', name: 'Ether Dream jardin' },
  { kind: 'etherdream', id: 'etherdream:cour', name: 'Ether Dream cour' },
];
const studio = useStudio({ devices: DEVS });

interface Projector { id: string; name: string; pos_m: number[]; yaw_deg: number; output: string | null; mirror_x: boolean }
interface VenueView { venue: { preset: string; room: { width_m: number; depth_m: number; height_m: number; ceiling: boolean }; projectors: Projector[] }; links: { id: string; output_name: string | null; state: string | null }[] }
interface Frames { armed: boolean; estop: boolean; frames: { id: string; lit: number; mirror_x: boolean; points: number[][] }[] }
const venue = () => studio.get<VenueView>('/api/venue');
const frames = () => studio.get<Frames>('/api/frames');
const DEFAULT_VENUE = { preset: 'club', projectors: [{ id: 'main', name: 'Projecteur 1' }] };

async function openVenue(page: import('@playwright/test').Page) {
  const summary = '#venuePanel summary';
  await reveal(page, summary);
  await page.locator(summary).click();
  await expect(page.locator('#vnList button')).not.toHaveCount(0);
}

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await studio.post('/api/estop/reset');
  expect(await studio.post('/api/venue', DEFAULT_VENUE)).toBe(200);
  await openUi(page, studio);
});

test.afterEach(async () => {
  await studio.post('/api/arm', { on: false });
  await studio.post('/api/estop/reset');
  await studio.post('/api/venue', DEFAULT_VENUE);
});

test('projectors are added, duplicated and removed; /api/frames has one entry each', async ({ page }) => {
  await openVenue(page);
  await expect(page.locator('#vnList button')).toHaveText(['Projecteur 1']);
  await expect(page.locator('#vnDel')).toBeDisabled();
  expect((await frames()).frames).toHaveLength(1);

  await page.locator('#vnAdd').click();
  await expect(page.locator('#vnList button')).toHaveCount(2);
  await expect(page.locator('#vnList button.active')).toHaveText('Projecteur 2');
  await page.locator('#vnName').fill('Jardin');
  await page.locator('#vnName').press('Enter');
  await page.locator('#vnPosX').fill('-3');
  await page.locator('#vnPosX').press('Enter');
  await expect.poll(async () => (await venue()).venue.projectors.map(p => [p.name, p.pos_m[0]])).toEqual([['Projecteur 1', 0], ['Jardin', -3]]);

  // Duplicate: the copy faces across the room, mirrored.
  await page.locator('#vnDup').click();
  await expect(page.locator('#vnList button')).toHaveCount(3);
  await expect.poll(async () => (await venue()).venue.projectors.map(p => [p.id, p.pos_m[0], p.mirror_x])).toEqual([['main', 0, false], ['p2', -3, false], ['p3', 3, true]]);
  await expect.poll(async () => (await frames()).frames.map(f => f.id)).toEqual(['main', 'p2', 'p3']);
  // Every projector without its own zones shows the default zone: lit in preview.
  await expect.poll(async () => (await frames()).frames.every(f => f.points.some(isLit as any))).toBe(true);

  await page.locator('#vnDel').click();
  await expect(page.locator('#vnList button')).toHaveCount(2);
  await expect.poll(async () => (await frames()).frames).toHaveLength(2);
  // The caps panel can now pick the projector.
  await expect(page.locator('#pwTarget option')).toHaveText(['Projecteur 1', 'Jardin']);
});

test('a projector drives its own output: one arm, one e-stop, no output change while armed', async ({ page }) => {
  // « Rechercher » (the fake scan) lists the devices the projectors can use.
  expect(await studio.post('/api/outputs/scan')).toBe(200);
  await expect.poll(async () => (await studio.get('/api/outputs')).devices.map((d: { id: string }) => d.id)).toContain('etherdream:cour');
  await openVenue(page);
  await page.locator('#vnAdd').click();
  await expect(page.locator('#vnList button')).toHaveCount(2);
  await page.locator('#vnOutput').selectOption('etherdream:jardin');
  await expect.poll(async () => (await venue()).links[1].output_name).toBe('Ether Dream jardin');
  await expect(page.locator('#vnOutState')).toContainText('connectée');

  expect(await studio.post('/api/arm', { on: true })).toBe(200);
  await expect.poll(async () => (await frames()).frames.map(f => f.lit > 0)).toEqual([true, true]);

  // Armed: the output can't change (the select goes back, a message says why).
  await page.locator('#vnOutput').selectOption('etherdream:cour');
  await expect(page.locator('#vnError')).toContainText('Désarmez');
  expect((await venue()).venue.projectors[1].output).toBe('etherdream:jardin');
  expect((await studio.state()).armed).toBe(true);

  // The e-stop darkens every projector.
  expect(await studio.post('/api/estop')).toBe(200);
  await expect.poll(async () => (await frames()).frames.map(f => f.lit)).toEqual([0, 0]);
  expect((await frames()).estop).toBe(true);
  expect((await studio.state()).armed).toBe(false);
});

test('a preset resizes the room and keeps the projectors', async ({ page }) => {
  await openVenue(page);
  await page.locator('#vnAdd').click();
  await expect(page.locator('#vnList button')).toHaveCount(2);
  await page.locator('#vnPreset').selectOption('festival');
  await expect.poll(async () => (await venue()).venue.room).toEqual({ width_m: 40, depth_m: 60, height_m: 30, ceiling: false });
  expect((await venue()).venue.projectors).toHaveLength(2);
  await expect(page.locator('#vnWidth')).toHaveValue('40');
  await expect(page.locator('#vnCeiling')).not.toBeChecked();
});

test('two mirrored projectors draw two symmetric fans in the 3D view', async ({ page }) => {
  await openVenue(page);
  await page.locator('#vnPosX').fill('-3');
  await page.locator('#vnPosX').press('Enter');
  await expect.poll(async () => (await venue()).venue.projectors[0].pos_m[0]).toBe(-3);
  await page.locator('#vnDup').click();
  await expect.poll(async () => (await venue()).venue.projectors.map(p => [p.pos_m[0], p.mirror_x])).toEqual([[-3, false], [3, true]]);
  await page.locator('#viewSeg button[data-view="3d"]').click();
  test.skip(await page.locator('#beamsMsg').isVisible(), 'no WebGL2 in this browser');
  // Mean hit x of each projector's beams: one on each side of the room.
  const sides = async () => page.evaluate(() => {
    // @ts-ignore: the page's own global
    const v = view3d;
    if (!v || !v.parts || v.parts.length !== 2) return null;
    v.draw();
    return v.parts.map((part: { beamStart: number; beams: number }) => {
      let sum = 0;
      for (let i = part.beamStart; i < part.beamStart + part.beams; i++) sum += v.geo.beam[i * 7];
      return part.beams ? sum / part.beams : 0;
    });
  });
  await expect.poll(async () => { const s = await sides(); return !!s && s[0] < -1 && s[1] > 1; }, { timeout: 10_000 }).toBe(true);
});
