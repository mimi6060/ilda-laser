// Agenda (T-455): a show programmed one minute later starts by itself at
// its time, in the preview (the agenda never arms), with a notification
// that says so, and stops at its end; with the e-stop latched nothing
// starts; edits are locked in show mode. The studio's clock is set with
// --test-hooks (POST /api/test/clock), never the Mac's. Preview only.
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';

const studio = useStudio({ testHooks: true });

interface AgendaRun { when: string; entry: string; result: string }
interface AgendaState { enabled: boolean; next: { name: string; time: string } | null; suspended: string | null; last_runs: AgendaRun[] }
interface State { armed: boolean; timeline: { name: string | null; playing: boolean }; agenda: AgendaState }
const state = () => studio.get<State>('/api/state');
const clock = (at: string) => studio.post('/api/test/clock', { at, tz: 'Europe/Paris' });

test.beforeAll(async () => {
  const catalog = await studio.presets();
  const show = {
    name: 'Soir agenda', time_base: 'seconds', tempo_map: [{ at_s: 0, bpm: 120, beats_per_bar: 4 }],
    tracks: [{ name: 'Piste 1', layer: 1, events: [{ id: 1, start: 0, len: 3, source: { kind: 'cue', id: catalog.presets[0].id } }] }],
  };
  expect(await studio.post('/api/shows', show)).toBe(200);
});

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset');
  await studio.post('/api/lock', { show_mode: false });
  await studio.post('/api/timeline/stop');
  await studio.reset();
  expect(await studio.post('/api/agenda', { enabled: false, entries: [] })).toBe(200);
  expect(await clock('2026-10-09T20:29:00')).toBe(200);
  await openUi(page, studio);
});

test('an entry one minute later plays at its time in the preview, says so, and stops at its end', async ({ page }) => {
  expect((await state()).agenda.enabled).toBe(false);
  await openWorkspace(page, 'timeline');
  await expect(page.locator('#agNext')).toHaveText('Agenda inactif');
  await expect(page.locator('#agWarn')).toContainText('sécurité matérielle');
  await page.locator('#agAdd').click();
  await expect(page.locator('#agList li')).toHaveCount(1);
  await page.locator('#ag0n').fill('Show du soir');
  await page.locator('#ag0n').press('Tab');
  await expect.poll(async () => (await studio.get('/api/agenda')).agenda.entries[0].name).toBe('Show du soir');
  await page.locator('#ag0t').selectOption('show:Soir agenda');
  await expect.poll(async () => (await studio.get('/api/agenda')).agenda.entries[0].target).toEqual({ kind: 'show', name: 'Soir agenda' });
  await page.locator('#ag0h').fill('20:30');
  await expect.poll(async () => (await studio.get('/api/agenda')).agenda.entries[0].time).toBe('20:30');
  // Every day by default; one day off and on again through its button.
  const friday = page.locator('#agList .ag-day[aria-label="vendredi"]');
  await expect(friday).toHaveAttribute('aria-pressed', 'true');
  await page.locator('#agEnabled').click();
  await expect(page.locator('#agEnabled')).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('#agNext')).toContainText('Prochain : « Show du soir » — ven. 09/10 20:30');
  await expect(page.locator('#agendaPill')).toHaveText('Agenda actif — prochain 20:30');
  expect((await state()).timeline.playing).toBe(false);

  // The clock reaches the slot.
  expect(await clock('2026-10-09T20:30:01')).toBe(200);
  await expect.poll(async () => (await state()).timeline.playing, { timeout: 5000 }).toBe(true);
  const st = await state();
  expect(st.timeline.name).toBe('Soir agenda');
  expect(st.armed).toBe(false);
  expect(st.agenda.last_runs[0]).toMatchObject({ entry: 'Show du soir', result: 'joué en aperçu (laser désarmé)' });
  await expect(page.locator('#toasts .toast-text', { hasText: 'Agenda : « Show du soir » joue en aperçu (laser désarmé).' })).toBeVisible();
  await page.locator('#agHistory summary').click();
  await expect(page.locator('#agRuns li').first()).toHaveText('ven. 09/10 20:30 — Show du soir — joué en aperçu (laser désarmé)');
  await expect(page.locator('#agNext')).toContainText('sam. 10/10 20:30');
  // Three seconds of show, no loop: it ends by itself, once.
  await expect.poll(async () => (await state()).timeline.playing, { timeout: 8000 }).toBe(false);
  expect((await state()).agenda.last_runs.filter(r => r.entry === 'Show du soir')).toHaveLength(1);
  expect((await state()).armed).toBe(false);
  await page.screenshot({ path: 'test-results/agenda.png' });
});

test('with the e-stop latched nothing starts; edits are locked in show mode', async ({ page }) => {
  expect(await studio.post('/api/agenda', { enabled: true, entries: [{ name: 'Soir', target: { kind: 'show', name: 'Soir agenda' }, time: '20:30', days: [true, true, true, true, true, true, true] }] })).toBe(200);
  await openWorkspace(page, 'timeline');
  await expect(page.locator('#agList li')).toHaveCount(1);
  expect(await studio.post('/api/estop')).toBe(200);
  expect(await clock('2026-10-09T20:30:01')).toBe(200);
  // The history is the studio's (earlier tests' lines stay): the newest line.
  await expect.poll(async () => (await state()).agenda.last_runs[0]?.result, { timeout: 5000 }).toBe('non lancé (arrêt d\'urgence)');
  const st = await state();
  expect(st.agenda.last_runs[0].entry).toBe('Soir');
  expect(st.timeline.playing).toBe(false);
  expect(st.armed).toBe(false);
  // Suspended after the stop: said on the page, and in the top bar.
  await expect(page.locator('#agSuspended')).toContainText('Agenda suspendu');
  await expect(page.locator('#agendaPill')).toHaveText('Agenda suspendu');
  // Show mode: the agenda is not edited.
  await studio.post('/api/estop/reset');
  expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
  expect(await studio.post('/api/agenda', { enabled: false, entries: [] })).toBe(423);
  await expect(page.locator('#agAdd')).toBeHidden();
  expect(await studio.post('/api/lock', { show_mode: false })).toBe(200);
});
