// Modèles de phrases (T-165): save a selection as a template, lay it on
// another region with Répéter (exact copies, aligned on bars) and Étirer
// (positions × 2), undo it like any edit, locked tracks refused, delete the
// template. Preview only: nothing here arms the laser.
import AxeBuilder from '@axe-core/playwright';
import { test, expect, useStudio, openUi, openWorkspace, type Page } from '../studio';

const studio = useStudio();
test.describe.configure({ mode: 'serial' });

interface Ev { id: number; start: number; len: number; source: { kind: string; id: string } }
interface Show { name: string; tracks: { name: string; locked?: boolean; events: Ev[] }[] }
const serverShow = async () => (await studio.get<{ show: Show }>('/api/timeline')).show;
const starts = async (track = 0) => (await serverShow()).tracks[track].events.map(e => [e.start, e.len, e.source.id]);
let cues: string[] = [];

test.beforeAll(async () => {
  cues = (await studio.presets()).presets.map(p => p.id);
});

test.beforeEach(async ({ page }) => {
  await studio.post('/api/timeline/stop');
  await studio.reset();
  await openUi(page, studio);
});

test.afterEach(async () => {
  expect((await studio.state()).armed, 'templates never arm').toBe(false);
});

/** A Secondes show at 120 BPM in 4/4 (1 bar = 2 s): A at bar 1, B at bar 4. */
async function seed(name: string) {
  expect(await studio.post('/api/timeline/new', { name, time_base: 'seconds' })).toBe(200);
  const show = await serverShow();
  show.tracks[0].events = [
    { id: 1, start: 0, len: 2, source: { kind: 'cue', id: cues[0] } },
    { id: 2, start: 6, len: 2, source: { kind: 'cue', id: cues[1] } },
  ];
  expect(await studio.post('/api/timeline/edit', { show })).toBe(200);
}

async function openEditor(page: Page, name: string) {
  await openWorkspace(page, 'timeline');
  await expect.poll(() => page.evaluate(() => (window as any).tlDebug.show()?.name)).toBe(name);
}

/** Select every event (Cmd+A with the timeline focused). */
async function selectAll(page: Page) {
  const box = (await page.locator('#tlCanvas').boundingBox())!;
  const v = await page.evaluate(() => (window as any).tlDebug.view());
  await page.mouse.click(box.x + box.width - 20, box.y + v.top + v.lane * 1.5); // empty lane: focus
  await page.keyboard.press('ControlOrMeta+a');
}

async function applyOn(page: Page, fit: string, fromBar: number, bars: number) {
  const dlg = page.locator('#tlTplDialog');
  await dlg.locator('[data-slot="A"]').selectOption(`cue:${cues[0]}`);
  await dlg.locator('[data-slot="B"]').selectOption(`cue:${cues[1]}`);
  await dlg.locator('#tlTplFrom').fill(String(fromBar));
  await dlg.locator('#tlTplBars').fill(String(bars));
  await dlg.locator(`#tlTplFit [data-fit="${fit}"]`).click();
  await dlg.locator('#tlTplGo').click();
}

test('save a selection as a template, lay it with Répéter and Étirer, undo, delete', async ({ page }) => {
  await seed('Modeles e2e');
  await openEditor(page, 'Modeles e2e');
  await selectAll(page);
  await page.locator('#tlTpl').click();
  const dlg = page.locator('#tlTplDialog');
  await expect(dlg).toBeVisible();
  await dlg.locator('#tlTplName').fill('Phrase e2e');
  await dlg.locator('#tlTplDesc').fill('Deux cues sur quatre mesures');
  await dlg.locator('#tlTplSave').click();
  await expect(dlg.locator('#tlTplMsg')).toContainText('Modèle « Phrase e2e » enregistré : 4 mesure(s), emplacements A, B');
  const list = await studio.get<{ templates: { name: string; builtin: boolean; length_bars: number }[] }>('/api/timeline/templates');
  expect(list.templates.find(t => t.name === 'Phrase e2e')).toMatchObject({ builtin: false, length_bars: 4 });
  await expect(dlg.locator('.tl-ex[aria-pressed="true"]')).toContainText('Phrase e2e');
  await expect(dlg.locator('#tlTplApply')).toBeVisible();
  await expect(dlg.locator('#tlTplPreview')).toBeVisible();

  // Répéter on bars 9 to 16 (16 s to 32 s): two exact copies, 8 s apart.
  await applyOn(page, 'repeat', 9, 8);
  await expect(dlg.locator('#tlTplMsg')).toContainText('« Phrase e2e » posé : 4 événement(s)');
  await expect.poll(() => starts()).toEqual([
    [0, 2, cues[0]], [6, 2, cues[1]],
    [16, 2, cues[0]], [22, 2, cues[1]], [24, 2, cues[0]], [30, 2, cues[1]],
  ]);
  await page.screenshot({ path: 'test-results/timeline-templates.png' });
  const axe = await new AxeBuilder({ page }).include('#tlTplDialog').withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
  expect(axe.violations.filter(v => v.impact === 'serious' || v.impact === 'critical').map(v => v.id), 'axe on the dialog').toEqual([]);

  // One edit: Annuler takes the whole phrase back.
  await dlg.locator('#tlTplClose').click();
  await page.locator('#tlUndo').click();
  await expect.poll(async () => (await starts()).length).toBe(2);

  // Étirer the 4 bars over 8: positions and lengths × 2.
  await page.locator('#tlTpl').click();
  await dlg.locator('.tl-ex', { hasText: 'Phrase e2e' }).click();
  await applyOn(page, 'stretch', 9, 8);
  await expect.poll(() => starts()).toEqual([[0, 2, cues[0]], [6, 2, cues[1]], [16, 4, cues[0]], [28, 4, cues[1]]]);

  // A slot without a cue is refused with a message; nothing changes.
  await dlg.locator('[data-slot="A"]').selectOption('');
  await dlg.locator('#tlTplGo').click();
  await expect(dlg.locator('#tlTplMsg')).toContainText('Emplacement A : choisissez un cue');

  page.once('dialog', d => d.accept());
  await dlg.locator('#tlTplDel').click();
  await expect(dlg.locator('#tlTplMsg')).toContainText('supprimé');
  expect((await studio.get<{ templates: { name: string }[] }>('/api/timeline/templates')).templates.map(t => t.name)).not.toContain('Phrase e2e');
});

test('a locked target track is refused; Auto picks cues of a page', async ({ page }) => {
  await seed('Verrou modeles');
  await openEditor(page, 'Verrou modeles');
  await selectAll(page);
  await page.locator('#tlTpl').click();
  const dlg = page.locator('#tlTplDialog');
  await dlg.locator('#tlTplName').fill('Phrase verrou');
  await dlg.locator('#tlTplSave').click();
  await expect(dlg.locator('#tlTplMsg')).toContainText('enregistré');
  await dlg.locator('#tlTplClose').click();

  const show = await serverShow();
  show.tracks[0].locked = true;
  expect(await studio.post('/api/timeline/edit', { show })).toBe(200);
  await expect.poll(() => page.evaluate(() => (window as any).tlDebug.show().tracks[0].locked)).toBe(true);
  await page.locator('#tlTpl').click();
  await dlg.locator('.tl-ex', { hasText: 'Phrase verrou' }).click();
  await applyOn(page, 'crop', 9, 4);
  await expect(dlg.locator('#tlTplMsg')).toContainText('piste verrouillée');
  expect((await starts()).length).toBe(2);

  // On the second track with Auto: two cues of the page, never arming.
  const all = (await studio.presets()).presets;
  const cat = all[0].category;
  const page1 = all.filter(p => p.category === cat).map(p => p.id);
  await dlg.locator('#tlTplTrack').selectOption('1');
  await dlg.locator('[data-slot="A"]').selectOption(`auto:${cat}`);
  await dlg.locator('[data-slot="B"]').selectOption(`auto:${cat}`);
  await dlg.locator('#tlTplGo').click();
  await expect(dlg.locator('#tlTplMsg')).toContainText('posé : 2 événement(s)');
  const laid = await starts(1);
  expect(laid.map(e => e[0])).toEqual([16, 22]);
  expect(laid.every(e => page1.includes(e[2] as string))).toBe(true);
  expect(laid[0][2]).not.toBe(laid[1][2]);
});
