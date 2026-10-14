// Readable controls (T-505, T-507): a checkbox sits next to its text, in
// every workspace; the effect names of LIVE › Effets read in full at the
// usual laptop sizes. Preview only: nothing is armed.
import { test, expect, useStudio, openUi, openWorkspace, reveal, type Workspace } from '../studio';

const studio = useStudio();
const WORKSPACES: Workspace[] = ['live', 'timeline', 'creation', 'settings', 'desk'];

test.beforeEach(async ({ page }) => {
  await studio.reset();
  await openUi(page, studio);
});

test('a checkbox and its text stay together, slider values stay on the right', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  for (const ws of WORKSPACES) {
    await openWorkspace(page, ws);
    const r = await page.evaluate(ws => {
      // Every folded section of this workspace open, to reach its checkboxes.
      document.querySelectorAll(`[data-ws="${ws}"] details`).forEach(d => { (d as HTMLDetailsElement).open = true; });
      const far: string[] = [];
      let checked = 0;
      for (const label of document.querySelectorAll('label.check')) {
        const box = label.querySelector(':scope > input[type=checkbox], :scope > input[type=radio]');
        if (!box || !(label as HTMLElement).offsetParent) continue;
        const b = box.getBoundingClientRect();
        if (!b.width) continue;
        // The first piece of text after (or before) the box.
        const walker = document.createTreeWalker(label, NodeFilter.SHOW_TEXT);
        let gap: number | null = null;
        for (let n = walker.nextNode(); n; n = walker.nextNode()) {
          if (!n.textContent!.trim() || !(n.parentElement as HTMLElement).offsetParent) continue;
          const range = document.createRange();
          range.selectNodeContents(n);
          const t = range.getBoundingClientRect();
          if (!t.width || Math.abs((t.top + t.bottom) / 2 - (b.top + b.bottom) / 2) > 12) continue;
          gap = t.left >= b.right ? t.left - b.right : b.left - t.right;
          break;
        }
        if (gap === null) continue;
        checked++;
        if (gap > 12) far.push(`${label.textContent!.trim().slice(0, 40)}: ${Math.round(gap)} px`);
      }
      // A slider's « name … value » label keeps its value at the right edge.
      const values = [...document.querySelectorAll('label:not(.check) > .v')].filter(v => (v as HTMLElement).offsetParent)
        .filter(v => v.parentElement!.getBoundingClientRect().right - v.getBoundingClientRect().right > 2)
        .map(v => v.parentElement!.textContent!.trim().slice(0, 40));
      return { far, checked, values };
    }, ws);
    expect(r.far, `checkbox and text apart in ${ws}`).toEqual([]);
    expect(r.values, `slider value not at the right in ${ws}`).toEqual([]);
    if (ws === 'settings') expect(r.checked).toBeGreaterThan(5);
  }
});

for (const [width, height] of [[1280, 800], [1440, 900]] as const) {
  test(`LIVE › Effets at ${width}×${height}: every effect name in full`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await reveal(page, '#fgGrid');
    await expect(page.locator('#fgGrid .fg-cell')).toHaveCount(32);
    const cells = await page.locator('#fgGrid .fg-cell:not(.empty)').evaluateAll(els => els.map(el => ({
      name: el.textContent!.trim(), title: el.getAttribute('title') ?? '',
      cut: el.scrollWidth > el.clientWidth || el.scrollHeight > el.clientHeight,
      // At most two lines of 11 px text.
      lines: Math.round((() => { const r = document.createRange(); r.selectNodeContents(el); return r.getClientRects().length ? r.getBoundingClientRect().height : 0; })() / 13),
    })));
    expect(cells.length).toBeGreaterThan(0);
    for (const c of cells) {
      expect(c.cut, `${c.name} cut`).toBe(false);
      expect(c.lines, `${c.name} on more than two lines`).toBeLessThanOrEqual(2);
      expect(c.title, `${c.name} in the title`).toContain(c.name.replace(/^⚡ /, ''));
    }
  });
}

// T-519: between the usual sizes too (the window dragged), the tab bar's
// shortcut hint and the MIDI state are never a fragment: the hint is
// whole, its first part or not shown, MIDI its long or short form, uncut.
// The window is resized live, so the page measures them again by itself.
test('the shortcut hint and the MIDI state are never cut while the window is resized', async ({ page }) => {
  for (const ws of ['live', 'timeline'] as Workspace[]) {
    await openWorkspace(page, ws);
    for (const width of [1100, 1366, 1600, 1920, 1180]) {
      await page.setViewportSize({ width, height: 860 });
      await expect.poll(() => page.evaluate(() => {
        const h = document.querySelector<HTMLElement>('#wsHint')!, inner = h.querySelector<HTMLElement>('.wh-in')!;
        const m = document.querySelector<HTMLElement>('#midiStatus')!;
        const hint = !inner.checkVisibility({ visibilityProperty: true }) ? 'hidden'
          : h.scrollWidth > h.clientWidth ? 'cut' : inner.innerText.replace(/\s+/g, ' ').trim();
        return { hint: hint === 'hidden' || hint.startsWith('Espace laser · Échap arrêt') ? 'ok' : hint,
          midi: m.scrollWidth <= m.clientWidth ? 'ok' : 'cut' };
      }), `${ws} at ${width}`).toEqual({ hint: 'ok', midi: 'ok' });
    }
  }
});

// T-519: with no show, the TIMELINE toolbar keeps no empty row: the
// invisible row of a selected event's presets used to hold ~25 px under
// the buttons. With a show open that row keeps its place (as before) and
// shows on a selection. (No spec of this file loads a show.)
test('TIMELINE: no empty band in the editor toolbar without a show', async ({ page }) => {
  const band = () => page.evaluate(() => {
    const tools = document.querySelector('#tlEditor .tl-tools')!;
    const kids = [...tools.children].filter(c => c.getClientRects().length);
    const ghosts = kids.filter(c => getComputedStyle(c).visibility === 'hidden').map(c => c.id || c.className);
    const last = Math.max(...kids.filter(c => getComputedStyle(c).visibility !== 'hidden').map(c => c.getBoundingClientRect().bottom));
    return { ghosts, gap: Math.round(tools.getBoundingClientRect().bottom - last) };
  });
  await openWorkspace(page, 'timeline');
  for (const [width, height] of [[1024, 768], [1280, 800], [1440, 900], [1920, 1080]]) {
    await page.setViewportSize({ width, height });
    expect(await band(), `no show at ${width}`).toEqual({ ghosts: [], gap: 0 });
  }
  await page.setViewportSize({ width: 1024, height: 768 });
  await page.locator('#tlExamples').click();
  await page.locator('#tlExDialog .tl-ex', { hasText: 'Montée 16 mesures' }).click();
  await expect(page.locator('#tlExDialog')).toBeHidden();
  await expect(page.locator('#tlEdMsg')).toContainText('Exemple');
  await expect(page.locator('#tlRefRow')).toBeHidden();
  // A clip selected: its presets row appears, no empty band either.
  await page.evaluate(async () => {
    const { TL } = await import('/ui/timeline-editor.js' as string);
    TL.sel = new Set([TL.show.tracks[0].events[0].id]);
    TL.need = true;
  });
  await expect(page.locator('#tlRefRow')).toBeVisible();
  expect(await band(), 'a clip selected').toEqual({ ghosts: [], gap: 0 });
});
