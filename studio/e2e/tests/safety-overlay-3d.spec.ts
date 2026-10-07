// Safety overlays in the 3D view (T-279): the venue's audience volume, the
// horizon and the safety zones cast from each projector, beams entering the
// audience drawn red and counted. Display only: the overlays are worked out
// in the page from frames it already has and never change /api/frame or the
// output. Preview only: the studio has no output and stays disarmed.
import { test, expect, useStudio, openUi, watchWrites, expectOnlyPrefs } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();
const SAFETY_DEFAULTS = { strobe_max_hz: 4, strobe_burst_s: 5, strobe_cooldown_s: 2, horizon: { y: 0 } };
const DEFAULT_VENUE = { preset: 'club', projectors: [{ id: 'main', name: 'Projecteur 1' }] };
const TIP = 'Aide à la conception, ne remplace pas une étude de sécurité ni la réglementation locale';

/** Evaluate inside the page with the beam3d module (window.__b3 in the callback). */
async function withModule<T>(page: Page, fn: string): Promise<T> {
  return page.evaluate(async src => {
    (window as any).__b3 = await import('/beam3d.js');
    return new Function('return (' + src + ')(window.__b3)')();
  }, fn);
}
const inAudience = (page: Page) => page.evaluate('view3d ? (view3d.draw(), view3d.inAudience) : -1') as Promise<number>;

test.beforeEach(async () => {
  await studio.post('/api/control', { id: 'cue.stop_all' });
  await studio.reset();
  expect(await studio.post('/api/safety', { ...SAFETY_DEFAULTS, confirm_loosen: true })).toBe(200);
  expect(await studio.post('/api/venue', DEFAULT_VENUE)).toBe(200);
});

test('geometry: segment / audience volume, red beams, overlay layers', async ({ page }) => {
  await openUi(page, studio);
  const r = await withModule<any>(page, `m => {
    const box = m.audienceBox({ rect_m: [4, 9, -4, 1], min_height_m: 3 });
    const hits = {
      through: m.segmentHitsBox([0, 3, 11], [0, 0, 5], box),     // down into the audience
      above: m.segmentHitsBox([0, 3.5, 11], [0, 3.5, 0], box),    // over it
      grazing: m.segmentHitsBox([0, 3, 11], [0, 3, 0], box),      // exactly at the minimum height
      short: m.segmentHitsBox([0, 3, 11], [0, 2, 10], box),       // stops before it
      inside: m.segmentHitsBox([0, 1, 5], [0, 1, 6], box),
    };
    const p = m.DEFAULT_PROJECTOR, room = m.DEFAULT_ROOM;
    // Down (pitch < 0), level, up, unlit down, down again.
    const pts = [[0, -0.5, 0, 1, 0], [0, 0, 0, 1, 0], [0, 0.5, 0, 1, 0], [0, -0.5, 0, 0, 0], [0.2, -0.6, 0, 0, 1]];
    const geo = {};
    const counts = m.buildGeometry(pts, p, room, geo, true, { beams: 0, sheets: 0 }, box);
    const colours = [0, 1, 2, 3].map(i => Array.from(geo.beam.slice(i * 7 + 3, i * 7 + 6)));
    const off = m.buildGeometry(pts, p, room, {}, true, { beams: 0, sheets: 0 }, null);
    const safety = { zones: [{ kind: 'blank', points: [[-1, -1], [1, -1], [1, 0], [-1, 0]] }], horizon: { y: 0 } };
    const size = o => [o.tris.length / 7, o.lines.length / 7];
    const all = size(m.buildSafetyOverlay({ room, projectors: [p, { ...p, pos: [3, 3, 11] }], audience: box, safety }));
    const noZones = size(m.buildSafetyOverlay({ room, projectors: [p, { ...p, pos: [3, 3, 11] }], audience: box, safety, show: { zones: false } }));
    const onlyAudience = size(m.buildSafetyOverlay({ room, projectors: [p], audience: box, safety, show: { zones: false, horizon: false } }));
    const nothing = size(m.buildSafetyOverlay({ room, projectors: [p], audience: box, safety, show: { audience: false, zones: false, horizon: false } }));
    return { box, hits, counts, colours, red: m.AUDIENCE_HIT_RGB, off, all, noZones, onlyAudience, nothing,
      empty: m.audienceBox({ rect_m: [0, 0, 0, 5], min_height_m: 3 }), noHeight: m.audienceBox({ rect_m: [-1, 1, 1, 2], min_height_m: 0 }) };
  }`);
  expect(r.box).toEqual({ lo: [-4, 0, 1], hi: [4, 3, 9] });
  expect(r.hits).toEqual({ through: true, above: false, grazing: false, short: false, inside: true });
  // The two downward lit beams are red and counted; level and up keep their colour.
  expect(r.counts).toEqual({ beams: 4, sheets: 2, inAudience: 2 });
  expect(r.colours[0][0]).toBeCloseTo(r.red[0], 5);
  expect(r.colours[0][1]).toBeCloseTo(r.red[1], 5);
  expect(r.colours[1]).toEqual([0, 1, 0]);
  expect(r.colours[2]).toEqual([0, 1, 0]);
  expect(r.colours[3][0]).toBeCloseTo(r.red[0], 5);
  // Without the audience volume nothing is counted.
  expect(r.off.inAudience).toBe(0);
  // Each layer adds geometry; the audience box is 12 triangles and 12 edges.
  expect(r.onlyAudience).toEqual([36, 24]);
  expect(r.nothing).toEqual([0, 0]);
  expect(r.all[0]).toBeGreaterThan(r.noZones[0]);
  expect(r.noZones[0]).toBeGreaterThan(r.onlyAudience[0]);
  expect(r.empty).toBeNull();
  expect(r.noHeight).toBeNull();
});

test('a look aimed down is red and counted; a Blank zone over the audience brings it to 0', async ({ page }) => {
  // Every write to the studio, except the page's heartbeat (T-252) and the
  // operator's preference saves (T-426, display only: `prefSaves`).
  const { writes: posts, prefSaves } = watchWrites(page);
  await openUi(page, studio);
  await page.locator('#viewSeg [data-view="3d"]').click();
  await expect(page.locator('#beams')).toBeVisible();
  // On by default, with the « design aid » wording on the counter.
  await expect(page.locator('#r3safety')).toBeChecked();
  await expect(page.locator('#aud3d')).toBeVisible();
  await expect(page.locator('#aud3d')).toHaveAttribute('title', TIP);
  await expect(page.locator('#stAudWrap')).toBeVisible();

  // The default look moved down: its lower half goes into the audience.
  expect(await studio.post('/api/control', { id: 'master.pos_y', value: -0.5 })).toBe(200);
  await expect.poll(() => inAudience(page)).toBeGreaterThan(0);
  await expect(page.locator('#aud3d')).toHaveClass(/hot/);
  await expect.poll(async () => +(await page.locator('#aud3dN').textContent() ?? 0)).toBeGreaterThan(0);
  await expect(page.locator('#aud3d')).toContainText('Faisceaux dans le public : ');
  await page.evaluate('view3d.setCamera({ dist: 16, az: Math.PI - 0.7, el: 0.35 })');
  await page.screenshot({ path: 'test-results/safety-overlay-3d-hot.png' });

  // A Blank zone over the lower half of the output (where the audience is):
  // the final frame has nothing lit there any more, the counter drops to 0.
  expect(await studio.post('/api/safety', { ...SAFETY_DEFAULTS, zones: [{ id: 0, name: 'Public', kind: 'blank', level: 0.3, points: [[-1, -1], [1, -1], [1, 0], [-1, 0]] }] })).toBe(200);
  await page.evaluate('reloadSafety()');
  await expect.poll(() => page.evaluate('view3d.safety && view3d.safety.zones.length')).toBe(1);
  await expect.poll(() => inAudience(page)).toBe(0);
  await expect(page.locator('#aud3dN')).toHaveText('0');
  await expect(page.locator('#aud3d')).not.toHaveClass(/hot/);
  // A closer look for the PR: the audience volume, the zone cone and the horizon.
  await page.setViewportSize({ width: 1400, height: 1400 });
  await page.evaluate('view3d.setCamera({ dist: 16, az: Math.PI - 0.7, el: 0.35 })');
  await page.screenshot({ path: 'test-results/safety-overlay-3d.png' });

  // Turning the overlays off is a page matter: nothing is sent to the
  // studio, the counter goes away, the layer switches grey out.
  const before = posts.length, savesBefore = prefSaves.length;
  const frameBefore = (await studio.get('/api/frame')).points.length;
  const revBefore = (await studio.get('/api/frame')).settings_rev;
  await page.locator('#r3safety').uncheck();
  await expect(page.locator('#aud3d')).toBeHidden();
  await expect(page.locator('#stAudWrap')).toBeHidden();
  await expect(page.locator('#r3sfZones')).toBeDisabled();
  expect(await page.evaluate('view3d.overlayTris + view3d.overlayLines')).toBe(0);
  await page.locator('#r3safety').check();
  await expect(page.locator('#aud3d')).toBeVisible();
  expect(await page.evaluate('view3d.overlayTris')).toBeGreaterThan(0);
  expect(posts.slice(before)).toEqual([]);
  expectOnlyPrefs(prefSaves, savesBefore);
  expect((await studio.get('/api/frame')).points.length).toBe(frameBefore);
  expect((await studio.get('/api/frame')).settings_rev).toBe(revBefore);
  expect((await studio.state()).armed).toBe(false);
});

test('each layer switch rebuilds only its part of the overlay', async ({ page }) => {
  await openUi(page, studio);
  await page.locator('#viewSeg [data-view="3d"]').click();
  await expect(page.locator('#beams')).toBeVisible();
  await expect.poll(() => page.evaluate('view3d.overlayTris')).toBeGreaterThan(0);
  const size = () => page.evaluate('[view3d.overlayTris, view3d.overlayLines]') as Promise<number[]>;
  const all = await size();
  await page.locator('#r3sfHorizon').uncheck();
  const noHorizon = await size();
  expect(noHorizon[0]).toBeLessThan(all[0]);
  await page.locator('#r3sfAudience').uncheck();
  // No zones by default, no horizon, no audience: empty.
  expect(await size()).toEqual([0, 0]);
  // Remembered in this browser, like the other render settings.
  await page.reload();
  await page.locator('#viewSeg [data-view="3d"]').click();
  await expect(page.locator('#r3sfHorizon')).not.toBeChecked();
  await expect(page.locator('#r3sfAudience')).not.toBeChecked();
  await expect(page.locator('#r3safety')).toBeChecked();
});
