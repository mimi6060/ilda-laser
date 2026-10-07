// Safety event log (T-259): arming, cues played while armed and the
// emergency stop land in the day's log, in order and with their sources,
// and show up in RÉGLAGES › Journal de sécurité, with a CSV export.
// Preview only: "armed" is just a flag in a studio that has no output.
import { readFileSync } from 'node:fs';
import { test, expect, useStudio, openUi, openWorkspace, focusPage } from '../studio';

const studio = useStudio();

interface LogEvent { ts: string; kind: string; source: string | null; source_fr: string; text: string; detail: any }
const log = () => studio.get<{ date: string; today: string; days: string[]; events: LogEvent[]; status: any }>('/api/safety/log');
const armed = async () => (await studio.state()).armed as boolean;
/** The log from `after` on (lines written by earlier tests are skipped). */
async function linesAfter(after: number) {
  return (await log()).events.slice(after).filter(e => e.kind !== 'presence');
}

test.beforeEach(async ({ page }) => {
  await studio.post('/api/estop/reset', {});
  await studio.post('/api/arm', { on: false });
  await openUi(page, studio);
});

test('arm, a cue, Escape: three lines in order with their sources, shown in the Journal', async ({ page }) => {
  const start = (await log()).events.length;
  const cue = (await studio.presets()).presets[2];

  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
  await page.locator(`#cues .cue[data-id="${cue.id}"]`).click();
  await expect.poll(async () => (await studio.controlValues()).active_cue).toBe(cue.id);
  await focusPage(page);
  await page.keyboard.press('Escape');
  await expect.poll(armed).toBe(false);

  await expect.poll(async () => (await linesAfter(start)).map(e => [e.kind, e.source])).toEqual([
    ['arm', 'ui'],
    ['cue', 'ui'],
    ['estop', 'keyboard'],
  ]);
  const lines = await linesAfter(start);
  expect(lines[1].detail.id).toBe(cue.id);
  expect(lines[2].detail.was_armed).toBe(true);
  // RFC 3339 local time with its offset, and today's file.
  expect(lines[0].ts).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}[+-]\d{2}:\d{2}$/);
  expect(lines[0].ts.slice(0, 10)).toBe((await log()).today);

  // RÉGLAGES › Journal de sécurité: newest first.
  await openWorkspace(page, 'settings');
  await page.locator('#safetyLogPanel > summary').click();
  const rows = page.locator('#slRows tr');
  await expect(rows.first()).toHaveAttribute('data-kind', 'estop');
  await expect(rows.first()).toContainText("Arrêt d'urgence");
  await expect(rows.first()).toContainText('clavier');
  await expect(page.locator('#slRows tr[data-kind="cue"]').first()).toContainText(cue.name);
  await expect(page.locator('#slRows tr[data-kind="arm"]').first()).toContainText('interface');

  // The filter keeps one kind of event.
  await page.locator('#slKind').selectOption('estop');
  await expect(page.locator('#slRows tr[data-kind="arm"]')).toHaveCount(0);
  await expect(page.locator('#slRows tr[data-kind="estop"]')).not.toHaveCount(0);

  // Resetting the stop from the banner shows up by itself (the panel refreshes).
  await page.locator('#estopReset').click();
  await expect(rows.first()).toHaveAttribute('data-kind', 'estop_reset', { timeout: 5_000 });
  expect(await armed()).toBe(false);

  // « Exporter CSV » downloads the day.
  const [download] = await Promise.all([page.waitForEvent('download'), page.locator('#slCsv').click()]);
  expect(download.suggestedFilename()).toMatch(/^journal-securite-\d{4}-\d{2}-\d{2}\.csv$/);
  const csv = readFileSync((await download.path())!, 'utf8');
  expect(csv).toContain('heure,type,source,événement,détail,session');
  expect(csv).toMatch(/,estop,clavier,/);
  expect(csv).toMatch(/,cue,interface,/);
});

test('Shift+Escape is logged as a plain disarm from the keyboard; a refused arm too', async ({ page }) => {
  const start = (await log()).events.length;
  await focusPage(page);
  await page.keyboard.press('Space');
  await expect.poll(armed).toBe(true);
  await page.keyboard.press('Shift+Escape');
  await expect.poll(armed).toBe(false);
  // A controller can never arm: refused, and logged.
  expect((await fetch(studio.url + '/api/arm', { method: 'POST', body: JSON.stringify({ on: true, source: 'midi' }) })).status).toBe(409);
  await expect.poll(async () => (await linesAfter(start)).map(e => [e.kind, e.source])).toEqual([
    ['arm', 'keyboard'],
    ['disarm', 'keyboard'],
    ['arm_refused', 'midi'],
  ]);
  const lines = await linesAfter(start);
  expect(lines[1].detail.reason).toBe('user');
  expect(lines[2].text).toContain('Armement refusé');
});

// T-482 (found by the soak): with the panel open, the page fetched the
// whole day every 2 s and rebuilt every row; a show day's thousands of
// events grew the page and froze it. Now the newest 300 rows, rebuilt only
// when the log changed, and a line pointing to the CSV for the rest.
test('a long day shows its newest 300 rows, with a pointer to the CSV, and is not rebuilt at rest', async ({ page }) => {
  for (let i = 0; i < 170; i++) {
    expect(await studio.post('/api/arm', { on: true })).toBe(200);
    expect(await studio.post('/api/arm', { on: false })).toBe(200);
  }
  const total = (await log()).events.length;
  expect(total).toBeGreaterThan(340);
  await openWorkspace(page, 'settings');
  await page.locator('#safetyLogPanel > summary').click();
  await expect(page.locator('#slRows tr[data-kind]')).toHaveCount(300);
  await expect(page.locator('#slMore')).toContainText(`${total - 300} événement(s) plus ancien(s)`);
  await expect(page.locator('#slMore')).toContainText('Exporter CSV');
  await expect(page.locator('#slRows tr').first()).toHaveAttribute('data-kind', 'disarm');
  // At rest, the 2 s refresh keeps the same rows (no rebuild).
  await page.locator('#slRows tr').first().evaluate(tr => { tr.setAttribute('data-mark', '1'); });
  await page.waitForTimeout(2_500);
  await expect(page.locator('#slRows tr').first()).toHaveAttribute('data-mark', '1');
  // A new event comes in at the top.
  expect(await studio.post('/api/arm', { on: true })).toBe(200);
  await expect(page.locator('#slRows tr').first()).toHaveAttribute('data-kind', 'arm', { timeout: 5_000 });
  expect(await studio.post('/api/arm', { on: false })).toBe(200);
});

test('safety settings changes are logged with before and after', async () => {
  const start = (await log()).events.length;
  const { settings } = await studio.get('/api/safety');
  expect(await studio.post('/api/safety', { ...settings, strobe_max_hz: 3 })).toBe(200);
  await expect.poll(async () => (await linesAfter(start)).map(e => e.kind)).toEqual(['safety_settings']);
  const [line] = await linesAfter(start);
  expect(line.detail.changed).toEqual(['strobe_max_hz']);
  expect([line.detail.before.strobe_max_hz, line.detail.after.strobe_max_hz]).toEqual([4, 3]);
  expect(line.text).toContain('Réglages de sécurité modifiés');
  // Loosening back needs the confirmation, which is logged too.
  expect(await studio.post('/api/safety', { ...settings, confirm_loosen: true })).toBe(200);
  await expect.poll(async () => (await linesAfter(start)).length).toBe(2);
  expect((await linesAfter(start))[1].detail.confirmed_loosen).toBe(true);
});
