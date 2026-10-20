// Preview rendering guard (T-531): the 2D stage preview draws the frame
// exactly once — one circle for a circle, never a doubled or tripled copy
// — and switching stage views never stacks a second render loop. The 2D
// preview is a fixed-size canvas the frame is drawn into (presence.js
// `draw`, cleared every frame), so it is device-pixel-ratio independent;
// this pins the single-copy behaviour the visual regression test cannot
// see, because it masks the preview canvas. Preview only: no arming.
import { test, expect, useStudio, openUi } from '../studio';
import type { Page } from '@playwright/test';

const studio = useStudio();

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await openUi(page, studio);
});

// Runs of lit (green) pixels along a line of the 2D preview. A single
// circle crosses an off-centre line exactly twice; three circles would
// cross it six times. The green channel is read, so the red/amber safety
// overlays (horizon, zones) are never counted as beam.
async function litRuns(page: Page, axis: 'h' | 'v', at: number): Promise<number> {
  return page.evaluate(([axis, at]) => {
    const c = document.getElementById('preview') as HTMLCanvasElement;
    const g = c.getContext('2d')!;
    const W = c.width, H = c.height, n = axis === 'h' ? W : H;
    const line = axis === 'h'
      ? g.getImageData(0, Math.round(H * at), W, 1).data
      : g.getImageData(Math.round(W * at), 0, 1, H).data;
    let runs = 0, inRun = false;
    for (let i = 0; i < n; i++) {
      const lit = line[i * 4 + 1] > 60; // green channel of the beam core
      if (lit && !inRun) { runs++; inRun = true; } else if (!lit) inRun = false;
    }
    return runs;
  }, [axis, at] as const);
}

test('the 2D preview draws the circle once, never doubled or tripled', async ({ page }) => {
  // Default content is a green circle (content.spec). Wait for a frame.
  await expect(page.locator('#stPoints')).not.toHaveText('0');
  // Off-centre lines (away from the centre crosshair and horizon overlay):
  // a lone circle crosses each exactly twice.
  for (const at of [0.33, 0.66]) {
    await expect.poll(() => litRuns(page, 'h', at)).toBe(2);
    await expect.poll(() => litRuns(page, 'v', at)).toBe(2);
  }
});

test('switching 2D / 3D / 2D+3D never stacks a render loop', async ({ page }) => {
  // Each animation frame runs at most a couple of callbacks (a view's
  // render loop and a one-shot layout pass). A loop leaked on every view
  // switch would make this grow without bound.
  const maxPerFrame = await page.evaluate(async () => {
    const raf = window.requestAnimationFrame.bind(window);
    let max = 0, inFrame = 0, lastTs = -1;
    const orig = window.requestAnimationFrame;
    window.requestAnimationFrame = (cb: FrameRequestCallback) => raf((ts: number) => {
      if (ts !== lastTs) { max = Math.max(max, inFrame); inFrame = 0; lastTs = ts; }
      inFrame++;
      return cb(ts);
    });
    const click = (v: string) => (document.querySelector(`#viewSeg button[data-view="${v}"]`) as HTMLElement | null)?.click();
    for (const v of ['3d', '2d', 'both', '3d', '2d', 'both', '3d', 'both', '2d', '3d', 'both']) {
      click(v);
      await new Promise(r => setTimeout(r, 120));
    }
    await new Promise(r => setTimeout(r, 600));
    window.requestAnimationFrame = orig;
    return max;
  });
  expect(maxPerFrame).toBeLessThanOrEqual(3);
});
