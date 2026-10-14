// Layout of the workspaces' stage (T-503, T-506, T-509): at 1024×768,
// 1280×800, 1440×900 and 1920×1080, in 2D, 3D and 2D+3D, every workspace
// shows a usable view (each visible canvas at least 160 px), the 3D
// settings drawer and the safety badge stay inside the stage, the
// toolbars under the stage stay clickable (nothing from the stage covers
// them), the cue grid keeps rows on a small screen, the TIMELINE panel
// shows its transport and timecode without scrolling, the markers' labels
// never overlap and the tracks stay on screen in show mode, read only.
// T-515 / T-516: at every size the TIMELINE editor shows two tracks with
// their clips (an example show open) and leaves no hole beside the 2D
// preview, LIVE shows two rows of cues whose names wrap rather than get
// cut, and « Préparer » in 2D+3D keeps a 2D output of at least 120 px.
//
// Preview only: the fake test output, the laser never armed; everything
// here is display, measured with getBoundingClientRect / elementFromPoint.
import { test, expect, useStudio, openUi, openWorkspace, open3dSettings, openCueMore, type Workspace } from '../studio';
import type { Locator, Page } from '@playwright/test';

const studio = useStudio();
const SIZES = [{ width: 1024, height: 768 }, { width: 1280, height: 800 }, { width: 1440, height: 900 }, { width: 1920, height: 1080 }] as const;
const WORKSPACES: Workspace[] = ['live', 'timeline', 'creation', 'settings'];
const MODES = ['2d', '3d', 'both'] as const;
const MIN_VIEW = 160;

type Box = { x: number; y: number; width: number; height: number };
const bottom = (b: Box) => b.y + b.height;
const right = (b: Box) => b.x + b.width;
const inside = (a: Box, b: Box, tol = 0.5) => a.x >= b.x - tol && a.y >= b.y - tol && right(a) <= right(b) + tol && bottom(a) <= bottom(b) + tol;

async function box(loc: Locator): Promise<Box> {
  const b = await loc.boundingBox();
  expect(b, `${loc} has a box`).not.toBeNull();
  return b!;
}

/** The view mode, like the operator: the 2D / 3D / 2D+3D buttons. */
async function setMode(page: Page, mode: typeof MODES[number]) {
  await page.locator(`#viewSeg [data-view="${mode}"]`).click();
  await expect(page.locator(`#viewSeg [data-view="${mode}"]`)).toHaveClass(/active/);
  // In 2D+3D with the prepared pane in a narrow view the 3D canvas is
  // hidden (T-516): viewUsable checks it where it must show.
  if (mode === '3d') await expect(page.locator('#beams')).toBeVisible();
  if (mode !== '3d') await expect(page.locator('#preview')).toBeVisible();
  // One layout pass after the switch.
  await page.evaluate(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))));
}

/** elementFromPoint at the centre of every visible button of `selector` is the button itself. */
async function clickable(page: Page, selector: string) {
  const hits = await page.evaluate(sel => {
    const out: string[] = [];
    for (const b of document.querySelectorAll<HTMLElement>(sel)) {
      if (b.hidden || b.closest('[hidden]') || !b.offsetParent) continue;
      const r = b.getBoundingClientRect();
      if (!r.width || !r.height) continue;
      if (r.bottom > innerHeight || r.right > innerWidth || r.top < 0) { out.push(`${b.id || b.textContent?.trim()} off screen (${Math.round(r.top)}…${Math.round(r.bottom)})`); continue; }
      const el = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2);
      if (el !== b && !b.contains(el)) out.push(`${b.id || b.textContent?.trim()} covered by ${el?.tagName}#${(el as HTMLElement | null)?.id}.${el?.className}`);
    }
    return out;
  }, selector);
  expect(hits, `${selector} clickable`).toEqual([]);
}

/** The canvases on screen measure at least MIN_VIEW and sit inside the stage. */
async function viewUsable(page: Page, mode: typeof MODES[number], label: string) {
  const stage = await box(page.locator('#stage'));
  const vp = page.viewportSize()!;
  for (const [id, shown] of [['preview', mode !== '3d'], ['beams', mode !== '2d']] as const) {
    const c = page.locator('#' + id);
    if (!shown) { await expect(c, `${label} #${id}`).toBeHidden(); continue; }
    const b = await box(c);
    expect(b.height, `${label} #${id} height`).toBeGreaterThanOrEqual(MIN_VIEW);
    expect(b.width, `${label} #${id} width`).toBeGreaterThanOrEqual(MIN_VIEW);
    expect(inside(b, stage), `${label} #${id} inside the stage`).toBe(true);
    expect(inside(b, { x: 0, y: 0, width: vp.width, height: vp.height }), `${label} #${id} on screen`).toBe(true);
  }
  // The stats line under the view (points, « Faisceaux dans le public ») is on screen.
  const st = await box(page.locator('#stPoints'));
  expect(inside(st, stage), `${label} stats inside the stage`).toBe(true);
  // No vertical page scroll: the workspace fits the window.
  expect(await page.evaluate(() => document.documentElement.scrollHeight <= innerHeight + 1), `${label} no page scroll`).toBe(true);
}

const TOOLBARS: Record<Workspace, string[]> = {
  live: ['#viewSeg button', '#cueModes button', '#cueMoreBtn', '#cuePageAdd'],
  timeline: ['#viewSeg button', '#tlEditor .tl-tools button', '#tlPlay', '#tlStop'],
  creation: ['#viewSeg button', '#creationTabs button'],
  settings: ['#viewSeg button'],
  desk: ['#viewSeg button'],
};

test.beforeEach(async ({ page }) => {
  if ((await studio.state()).estop) await studio.post('/api/estop/reset');
  await studio.reset();
  await page.goto(studio.url + '/');
});

test.afterEach(async () => {
  await studio.post('/api/arm', { on: false });
  if ((await studio.get('/api/lock')).show_mode) await studio.post('/api/lock', { show_mode: false });
});

for (const size of SIZES) {
  test.describe(`${size.width}×${size.height}`, () => {
    for (const ws of WORKSPACES) {
      test(`${ws}: the view keeps ${MIN_VIEW} px, the toolbars stay clickable, the 3D safety badge is on screen`, async ({ page }) => {
        await page.setViewportSize(size);
        await openUi(page, studio);
        await openWorkspace(page, ws);
        for (const mode of MODES) {
          const label = `${ws} ${mode}`;
          await setMode(page, mode);
          await viewUsable(page, mode, label);
          for (const sel of TOOLBARS[ws]) await clickable(page, sel);
          if (mode === '2d') continue;
          // The safety badge of the 3D view, inside the view, in every mode.
          const view = await box(page.locator('#stage .view'));
          await expect(page.locator('#aud3d'), label).toBeVisible();
          expect(inside(await box(page.locator('#aud3d')), view), `${label} badge inside the view`).toBe(true);
          // The settings drawer never leaves the stage, and the badge and
          // the toolbars stay reachable while it is open.
          await open3dSettings(page);
          const drawer = await box(page.locator('#render3d'));
          expect(inside(drawer, view), `${label} drawer inside the view`).toBe(true);
          expect(drawer.y, `${label} drawer below the badge`).toBeGreaterThanOrEqual(bottom(await box(page.locator('#aud3d'))) - 0.5);
          await expect(page.locator('#aud3d'), label).toBeVisible();
          await expect(page.locator('#cam3dSeg'), label).toBeVisible();
          for (const sel of TOOLBARS[ws]) await clickable(page, sel);
          await page.locator('#render3dClose').click();
          await expect(page.locator('#render3d')).toBeHidden();
          await expect(page.locator('#render3dBtn')).toHaveAttribute('aria-expanded', 'false');
        }
        await setMode(page, '2d');
        await expect(page.locator('#render3dBtn')).toBeHidden();
      });
    }

    test('LIVE: rows of cues on screen, the rare settings behind « Plus… », pages beyond the edge reachable', async ({ page }) => {
      await page.setViewportSize(size);
      await openUi(page, studio);
      const scroll = await box(page.locator('#cueRegion .cue-scroll'));
      const cells = await page.locator('#cues .cue').evaluateAll((els, s) => els.map(e => e.getBoundingClientRect())
        .filter(r => r.top >= s.y - 0.5 && r.bottom <= s.y + s.height + 0.5).map(r => Math.round(r.top)), scroll);
      const rows = new Set(cells).size;
      // T-516: two full rows at 1024×768 too (the cue bar on one row, lower header rows).
      expect(rows, 'full rows of cues on screen').toBeGreaterThanOrEqual(2);
      // Cue names are never cut sideways (« Lissajo| »): the text wraps in
      // its box, each shown line is whole, and a name that needs more
      // than the three lines ends in « … » (the clamp), never mid-word.
      const names = await page.locator('#cues .cue .n').evaluateAll(els => els.map(n => {
        const cs = getComputedStyle(n);
        return { text: n.textContent ?? '', cw: n.clientWidth, sw: n.scrollWidth, ch: n.clientHeight, sh: n.scrollHeight, line: parseFloat(cs.lineHeight), clamp: (cs as unknown as { webkitLineClamp: string }).webkitLineClamp };
      }));
      expect(names.length).toBeGreaterThan(0);
      for (const n of names) {
        expect(n.sw, `« ${n.text} » not cut sideways (${n.sw} px in ${n.cw})`).toBeLessThanOrEqual(n.cw + 1);
        expect(Math.abs(n.ch / n.line - Math.round(n.ch / n.line)), `« ${n.text} » whole lines (${n.ch} px for ${n.line} px lines)`).toBeLessThan(0.15);
        if (n.sh > n.ch + 1) expect(n.clamp, `« ${n.text} » cut with « … »`).not.toBe('none');
      }
      // Folded by default, nothing removed: « Plus… » shows them, the choice is kept on reload.
      await expect(page.locator('#cueMax')).toBeHidden();
      await expect(page.locator('#cueBeatMode')).toBeHidden();
      await openCueMore(page);
      await expect(page.locator('#cueMax')).toBeVisible();
      await expect(page.locator('#gridMenuBtn')).toBeVisible();
      await expect(page.locator('#cueMoreBtn')).toHaveText('Moins');
      await page.reload();
      await expect(page.locator('#cueMax')).toBeVisible();
      await page.locator('#cueMoreBtn').click();
      await expect(page.locator('#cueMax')).toBeHidden();
      // The layers' speed selects are not cut: the widest option's text,
      // measured with the select's font, fits its content box less the arrow.
      for (const sel of await page.locator('#layers .spd').all()) {
        const m = await sel.evaluate(e => {
          const s = e as HTMLSelectElement, cs = getComputedStyle(s);
          const c = document.createElement('canvas').getContext('2d')!;
          c.font = `${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`;
          const widest = Math.max(...Array.from(s.options, o => c.measureText(o.text).width));
          const room = s.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight) - 22; // 22 px: the native arrow
          return { widest, room, shown: s.options[s.selectedIndex].text };
        });
        expect(m.widest, `speed select « ${m.shown} » fits (${m.widest.toFixed(1)} px in ${m.room.toFixed(1)} px)`).toBeLessThanOrEqual(m.room);
      }
      // The executors: on a tall screen they start on screen (as on develop,
      // where their last 24 px scroll at 1080); on short screens the layers
      // column scrolls to them (the 1280×800 trade-off: two rows of cues).
      const region = await box(page.locator('#layerRegion'));
      if (size.height >= 1000) {
        const exec = await box(page.locator('#execRow'));
        expect(exec.y + 24, 'executors start on screen without scrolling').toBeLessThanOrEqual(bottom(region));
      }
      await page.locator('#execRow').scrollIntoViewIfNeeded();
      expect(inside(await box(page.locator('#execRow')), region), 'executors whole after scrolling the layers column').toBe(true);
      // Page tabs past the right edge: the tab row says so and scrolls to them.
      const tabs = page.locator('#cueTabs');
      const overflow = await tabs.evaluate(t => t.scrollWidth > t.clientWidth + 1);
      if (overflow) {
        await expect(tabs).toHaveClass(/more-right/);
        const last = tabs.locator('button').last();
        await last.scrollIntoViewIfNeeded();
        await expect(tabs).toHaveClass(/more-left/);
        await expect(last).toBeInViewport();
      } else {
        await expect(tabs).not.toHaveClass(/more-right/);
      }
    });

    // T-515: the editor shows its tracks. With an example show open (its
    // message line and « Tempo conseillé » in the toolbar), in 2D, 3D and
    // 2D+3D, at least two tracks with their clips are on screen without
    // scrolling, the view keeps 160 px and the page does not scroll. In
    // 2D the stage column is no wider than its square needs: no hole over
    // 200 px beside the preview (1920 used to leave 400 px).
    test('TIMELINE: two tracks with their clips on screen with an example show, no hole beside the 2D preview', async ({ page }) => {
      await page.setViewportSize(size);
      await openUi(page, studio);
      await openWorkspace(page, 'timeline');
      await page.locator('#tlExamples').click();
      await page.locator('#tlExDialog .tl-ex', { hasText: 'Montée 16 mesures' }).click();
      await expect(page.locator('#tlExDialog')).toBeHidden();
      await expect.poll(async () => (await studio.get('/api/timeline')).state.name).toBe('Montée 16 mesures vers le drop');
      await page.locator('#tlFit').click();
      await expect(page.locator('#tlEdMsg')).toContainText('Exemple');
      for (const mode of MODES) {
        const label = `timeline ${mode}`;
        await setMode(page, mode);
        await viewUsable(page, mode, label);
        await clickable(page, '#tlEditor .tl-tools button');
        const tracks = page.locator('#tlTracks');
        expect(await tracks.evaluate(t => t.scrollTop), `${label} tracks not scrolled`).toBe(0);
        const tb = await box(tracks);
        const vp = page.viewportSize()!;
        expect(inside(tb, { x: 0, y: 0, width: vp.width, height: vp.height }), `${label} tracks box on screen`).toBe(true);
        // The ruler, markers and waveform band (88 px) and two 34 px lanes.
        expect(await tracks.evaluate(t => t.clientHeight), `${label} tracks height`).toBeGreaterThanOrEqual(88 + 2 * 34);
        for (const ti of [0, 1]) {
          const head = await box(page.locator(`#tlHeads .tl-head[data-ti="${ti}"]`));
          expect(inside(head, tb), `${label} track ${ti + 1} head whole in the tracks box`).toBe(true);
        }
        // A clip of each of the first two tracks is drawn inside the visible canvas area.
        const lanes = await page.evaluate(async () => {
          const { TL, TL_LANE, tlLaneY } = await import('/ui/timeline-editor.js' as string);
          return [0, 1].map(ti => ({ y: tlLaneY(ti), h: TL_LANE, clips: TL.show.tracks[ti].events.length }));
        });
        for (const [i, l] of lanes.entries()) {
          expect(l.clips, `${label} track ${i + 1} has clips`).toBeGreaterThan(0);
          expect(tb.y + 1 + l.y + l.h, `${label} track ${i + 1} lane inside the tracks box`).toBeLessThanOrEqual(bottom(tb) + 0.5);
        }
        if (mode === '2d') {
          const stage = await box(page.locator('#stage'));
          const pv = await box(page.locator('#preview'));
          expect(stage.width - pv.width, `${label} hole beside the preview`).toBeLessThanOrEqual(200);
        }
      }
      await studio.post('/api/timeline/stop');
    });

    // T-516: « Préparer » in 2D+3D. Three squares and the 196 px pane
    // left the output 69 px at 1024; in a view up to 420 px wide the 2D
    // output alone is kept, at least 120 px, with the audience count
    // still shown (badge and stats); wider, the 3D view stays beside it.
    // The collapse is never silent: the bar says the 3D is hidden and how
    // to get it back, the 3D-only controls go, and the 3D stays hidden
    // while a prepared cue keeps the pane (« Préparer » off included)
    // until « Vider » / « Envoyer ».
    test('LIVE and TIMELINE: « Préparer » in 2D+3D keeps a usable output, and says when the 3D is hidden', async ({ page }) => {
      await page.setViewportSize(size);
      await openUi(page, studio);
      const cueId = await page.locator('#cues .cue[data-id]').first().getAttribute('data-id');
      expect(cueId).toBeTruthy();
      const beams = page.locator('#beams'), note = page.locator('#no3dNote'), prep = page.locator('#prepBtn');
      const controls3d = ['#recenter3d', '#hint3d', '#vid3d', '#render3dBtn', '#render3d'];
      // The 3D view and its controls, or the notice and none of them. The
      // notice names what brings the 3D back in this state: with
      // « Préparer » on, the pane stays whatever « Vider » / « Envoyer » do.
      const expect3d = async (shown: boolean, label: string, prepOn = false) => {
        if (shown) {
          await expect(beams, `${label} 3D view`).toBeVisible();
          expect((await box(beams)).height, `${label} 3D view height`).toBeGreaterThanOrEqual(MIN_VIEW);
          await expect(note, `${label} no notice`).toBeHidden();
          for (const sel of ['#recenter3d', '#render3dBtn', '#vid3d']) await expect(page.locator(sel), `${label} ${sel}`).toBeVisible();
        } else {
          await expect(beams, `${label} 3D view hidden`).toBeHidden();
          await expect(note, `${label} notice`).toBeVisible();
          await expect(note, label).toContainText(/3D/, { useInnerText: true });
          if (prepOn) {
            await expect(note, `${label} notice with « Préparer » on`).toContainText(/Préparer/, { useInnerText: true });
            await expect(note, `${label} notice does not offer « Vider »`).not.toContainText(/Vider|Envoyer/, { useInnerText: true });
          } else await expect(note, `${label} notice with « Préparer » off`).toContainText(/Vider/, { useInnerText: true });
          for (const sel of controls3d) await expect(page.locator(sel), `${label} ${sel} hidden with the 3D`).toBeHidden();
          // The notice is whole, on one line, inside the stage and off the view (badges and pane are in the view).
          const n = await note.evaluate(e => ({ sw: e.scrollWidth, cw: e.clientWidth, lh: parseFloat(getComputedStyle(e).lineHeight), h: e.clientHeight }));
          expect(n.sw, `${label} notice whole (${n.sw} px in ${n.cw})`).toBeLessThanOrEqual(n.cw + 1);
          expect(n.h, `${label} notice on one line`).toBeLessThanOrEqual(n.lh + 2);
          const nb = await box(note);
          expect(inside(nb, await box(page.locator('#stage'))), `${label} notice inside the stage`).toBe(true);
          expect(bottom(nb), `${label} notice above the view`).toBeLessThanOrEqual((await box(page.locator('#stage .view'))).y + 0.5);
          // The audience count goes on: badge and stats agree and are not reset.
          await expect.poll(async () => page.evaluate(() => [document.getElementById('aud3dN')!.textContent, document.getElementById('stAud')!.textContent]), { message: `${label} audience count` })
            .toEqual(expect.arrayContaining([expect.stringMatching(/^[1-9]\d*$/)]));
          const [badge, stats] = await page.evaluate(() => [document.getElementById('aud3dN')!.textContent, document.getElementById('stAud')!.textContent]);
          expect(badge, `${label} badge and stats agree`).toBe(stats);
        }
      };
      for (const ws of ['live', 'timeline'] as const) {
        await openWorkspace(page, ws);
        await setMode(page, 'both');
        // The drawer open before the pane shows: it must not float over a 2D-only view.
        await open3dSettings(page);
        await prep.click();
        await expect(prep).toHaveAttribute('aria-pressed', 'true');
        await expect(page.locator('#prepPane')).toBeVisible();
        await page.evaluate(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))));
        const label = `${ws} both prepared`;
        const view = await box(page.locator('#stage .view'));
        const narrow = view.width <= 420;
        const pv = await box(page.locator('#preview'));
        expect(pv.width, `${label} 2D output width`).toBeGreaterThanOrEqual(120);
        expect(pv.height, `${label} 2D output height`).toBeGreaterThanOrEqual(120);
        expect(inside(pv, view), `${label} 2D output inside the view`).toBe(true);
        expect(inside(await box(page.locator('#prepPane')), view), `${label} pane inside the view`).toBe(true);
        await expect3d(!narrow, label, true);
        if (!narrow) expect((await box(beams)).width, `${label} 3D view width`).toBeGreaterThanOrEqual(120);
        // A second click on « 2D+3D » (setView shows the 3D controls again) changes nothing.
        await setMode(page, 'both');
        await expect3d(!narrow, `${label}, 2D+3D clicked again`, true);
        await expect(page.locator('#aud3d'), `${label} audience badge`).toBeVisible();
        await expect(page.locator('#stAudWrap'), `${label} audience count in the stats`).toBeVisible();
        expect(inside(await box(page.locator('#stAudWrap')), await box(page.locator('#stage'))), `${label} stats inside the stage`).toBe(true);
        // A real cue prepared with « Préparer » on: « Vider » empties the pane but
        // keeps it, so the 3D stays collapsed and the notice must not offer « Vider ».
        expect(await studio.post('/api/prepared', { id: cueId })).toBe(200);
        await expect(page.locator('#prepSend')).toBeEnabled();
        await expect3d(!narrow, `${label}, cue with « Préparer » on`, true);
        await page.locator('#prepClear').click();
        await expect(page.locator('#prepSend')).toBeDisabled();
        await expect(page.locator('#prepPane')).toBeVisible();
        await page.evaluate(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))));
        await expect3d(!narrow, `${label}, « Vider » with « Préparer » on`, true);
        // The cue prepared again, then « Préparer » off: the pane stays, so does the collapse; now « Vider » applies.
        expect(await studio.post('/api/prepared', { id: cueId })).toBe(200);
        await expect(page.locator('#prepSend')).toBeEnabled();
        await prep.click();
        await expect(prep).toHaveAttribute('aria-pressed', 'false');
        await expect(page.locator('#prepPane')).toBeVisible();
        await page.evaluate(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r))));
        await expect3d(!narrow, `${label}, « Préparer » off with a cue`);
        // 3D mode shows the 3D beside the pane; back to 2D+3D collapses it again.
        await setMode(page, '3d');
        await expect3d(true, `${ws} 3d with the pane`);
        await setMode(page, 'both');
        await expect3d(!narrow, `${label} again`);
        // « Vider »: the pane goes, the 3D view is back with its controls.
        await page.locator('#prepClear').click();
        await expect(page.locator('#prepPane')).toBeHidden();
        await expect3d(true, `${ws} both after Vider`);
        await page.locator('#render3dClose').click();
        await setMode(page, '2d');
        await expect(note).toBeHidden();
      }
      await studio.post('/api/prepared/clear');
    });
  });
}

test('TIMELINE at 1440×900: transport, TC-IN and TC-OUT on screen without scrolling, a 240 px preview', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await openUi(page, studio);
  await openWorkspace(page, 'timeline');
  await setMode(page, '2d');
  expect((await box(page.locator('#preview'))).height).toBeGreaterThanOrEqual(240);
  const panel = page.locator('#timelinePanel');
  expect(await panel.evaluate(p => p.scrollTop)).toBe(0);
  const pb = await box(panel);
  for (const id of ['tlPlay', 'tlStop', 'tcArm', 'tcoToggle']) {
    const b = await box(page.locator('#' + id));
    expect(inside(b, pb), `#${id} visible in the panel without scrolling`).toBe(true);
  }
  // The editor's own buttons are not covered by the stage.
  await clickable(page, '#tlEditor .tl-tools button');
});

test('CRÉATION › Figures at 1024×768: the tools do not overlap the frames row', async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 768 });
  await openUi(page, studio);
  await openWorkspace(page, 'creation');
  await page.locator('#creationTabs [data-ctab="figures"]').click();
  await expect(page.locator('#figEditor')).toBeVisible();
  const frames = await box(page.locator('#figEditor .fig-frames'));
  const tools = page.locator('#figTools > *');
  const boxes: Box[] = [];
  for (const t of await tools.all()) {
    const b = await box(t);
    expect(bottom(b), 'tool above the frames row').toBeLessThanOrEqual(frames.y + 0.5);
    boxes.push(b);
  }
  for (let i = 0; i < boxes.length; i++) for (let j = i + 1; j < boxes.length; j++) {
    const a = boxes[i], b = boxes[j];
    const overlap = a.x < right(b) - 1 && b.x < right(a) - 1 && a.y < bottom(b) - 1 && b.y < bottom(a) - 1;
    expect(overlap, `tools ${i} and ${j} overlap`).toBe(false);
  }
  await clickable(page, '#figTools button');
  expect((await box(page.locator('#figCanvas'))).height).toBeGreaterThanOrEqual(MIN_VIEW);
});

test('marker labels never overlap, the full name shows on hover', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await openUi(page, studio);
  await openWorkspace(page, 'timeline');
  // The layout rule itself: labels cut before the next flag, nothing under 2 characters.
  const layout = await page.evaluate(async () => {
    const m = await import('/ui/timeline-canvas.js' as string);
    const measure = (s: string) => s.length * 7;
    const marks = [{ x: 10, name: 'Faisceaux' }, { x: 60, name: 'Grand noir' }, { x: 75, name: 'Ciseaux' }, { x: 200, name: 'Rayons tournants inversés' }];
    return { out: m.tlMarkerLayout(marks, 260, measure), marks };
  });
  const out = layout.out as { text: string; x0: number; x1: number; cut: boolean }[];
  expect(out.map(l => l.cut)).toEqual([true, true, false, true]);
  expect(out[0].text.endsWith('…') && out[0].text.length < 'Faisceaux'.length).toBe(true);
  expect(out[1].text).toBe(''); // 15 px: no room for two characters and « … »
  expect(out[2].text).toBe('Ciseaux');
  for (let i = 0; i + 1 < out.length; i++) expect(out[i].x1, `label ${i} ends before flag ${i + 1}`).toBeLessThanOrEqual(layout.marks[i + 1].x - 4);
  expect(out[3].x1).toBeLessThanOrEqual(260 - 4);

  // On the example show: no two drawn labels overlap, hovering one gives its name.
  await page.locator('#tlExamples').click();
  const dlg = page.locator('#tlExDialog');
  await dlg.locator('.tl-ex', { hasText: 'Montée 16 mesures' }).click();
  await expect(dlg).toBeHidden();
  await page.locator('#tlFit').click();
  const labels = await page.evaluate(async () => {
    const { TL } = await import('/ui/timeline-editor.js' as string);
    await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
    return (TL.markerLabels as { text: string; x0: number; x1: number }[]).map((l, i) => ({ ...l, name: TL.show.markers[i].name }));
  });
  expect(labels.length).toBeGreaterThan(3);
  const drawn = labels.filter(l => l.text);
  for (let i = 0; i + 1 < drawn.length; i++) expect(drawn[i].x1, `${drawn[i].text} / ${drawn[i + 1].text}`).toBeLessThanOrEqual(drawn[i + 1].x0 - 11);
  const cut = labels.find(l => l.text && l.text !== l.name) ?? drawn[0];
  const cv = await box(page.locator('#tlCanvas'));
  await page.mouse.move(cv.x + cut.x0 + 3, cv.y + 30 + 9); // TL_RULER + TL_MARK / 2
  await expect(page.locator('#tlCanvas')).toHaveAttribute('title', cut.name);
  await page.mouse.move(cv.x + 10, cv.y + 120);
  await expect(page.locator('#tlCanvas')).not.toHaveAttribute('title', cut.name);
});

test('show mode: the tracks and the playhead stay on screen, read only', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await openUi(page, studio);
  await openWorkspace(page, 'timeline');
  await page.locator('#tlExamples').click();
  await page.locator('#tlExDialog .tl-ex', { hasText: 'Montée 16 mesures' }).click();
  await expect(page.locator('#tlExDialog')).toBeHidden();
  const before = (await studio.get('/api/timeline')).show;
  await page.locator('#lockBtn').click();
  await expect.poll(async () => (await studio.get('/api/lock')).show_mode).toBe(true);
  await expect(page.locator('#tlEditor')).toBeVisible();
  await expect(page.locator('#tlTracks')).toBeVisible();
  await expect(page.locator('#tlCanvas')).toBeVisible();
  await expect(page.locator('#tlEditor .tl-tools')).toBeHidden();
  await expect(page.locator('#tlEditor .tl-lib')).toBeHidden();
  await expect(page.locator('#tlPlay')).toBeVisible();
  // A drag on an event moves nothing; a double-click adds no marker.
  const cv = await box(page.locator('#tlCanvas'));
  await page.mouse.move(cv.x + 120, cv.y + 110);
  await page.mouse.down();
  await page.mouse.move(cv.x + 260, cv.y + 110, { steps: 6 });
  await page.mouse.up();
  await expect(page.locator('#lockToast')).toHaveText('Verrouillé en mode spectacle');
  await page.mouse.dblclick(cv.x + 300, cv.y + 40);
  expect((await studio.get('/api/timeline')).show).toEqual(before);
  // Unlocking (a 1 s hold) brings the toolbar back.
  await page.locator('#lockBtn').hover();
  await page.mouse.down();
  await page.waitForTimeout(1300);
  await page.mouse.up();
  await expect.poll(async () => (await studio.get('/api/lock')).show_mode).toBe(false);
  await expect(page.locator('#tlEditor .tl-tools')).toBeVisible();
});
