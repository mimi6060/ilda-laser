// The synchronised backup machine (T-456): two preview studios on this Mac
// (two ports, two temporary data dirs), one *Principale*, the other
// *Secours*. The backup pairs from RÉGLAGES with the code shown on the main
// one, follows its cue and its show, refuses arming while in standby, then
// takes over once the main one is gone: the laser stays disarmed and the
// playback carries on. The sync listener is bound to 127.0.0.1 (studio.ts,
// --standby-bind): never another machine, never a laser.
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';
import { createServer } from 'node:net';

const main = useStudio();
const backup = useStudio();

interface Standby { role: string; link: boolean; lag_ms: number; paired: boolean; code: string; bound_port: number | null; lost_s: number | null; backup_lost: boolean; takeover_after_s: number; peer_held_s: number | null }
interface State { armed: boolean; timeline: { name: string | null; playing: boolean; position: number }; standby: Standby }

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const srv = createServer();
    srv.on('error', reject);
    srv.listen(0, '127.0.0.1', () => {
      const a = srv.address();
      const p = typeof a === 'object' && a ? a.port : 0;
      srv.close(() => resolve(p));
    });
  });
}

test('the backup follows the main machine, never arms, and takes over disarmed', async ({ page }) => {
  // The main machine: role Principale, its own free loopback port.
  const port = await freePort();
  expect(await main.post('/api/standby', { role: 'primary', port })).toBe(200);
  const m = await main.get<Standby>('/api/standby');
  expect(m.role).toBe('primary');
  expect(m.bound_port).toBe(port);
  expect(m.code).toMatch(/^\d{6}$/);

  // The backup, from its own RÉGLAGES: Secours, the address, the code.
  await openUi(page, backup);
  await expect(page.locator('#standbyPill')).toBeHidden();
  await openWorkspace(page, 'settings');
  await page.locator('#standbyPanel > summary').click();
  await page.locator('#sbPeer').fill(`127.0.0.1:${port}`);
  await page.locator('#sbPeer').press('Tab');
  await expect.poll(async () => (await backup.get('/api/standby')).peer).toBe(`127.0.0.1:${port}`);
  await page.locator('#sbRole').selectOption('standby');
  await expect(page.locator('#standbyPill')).toHaveText(/^SECOURS — en veille/);
  await expect(page.locator('#takeoverBtn')).toBeVisible();
  await page.locator('#sbPairCode').fill(m.code);
  await page.locator('#sbPairBtn').click();
  await expect.poll(async () => (await backup.get<Standby>('/api/standby')).link, { timeout: 5000 }).toBe(true);
  await expect(page.locator('#sbLink')).toContainText('Lien : établi — retard');
  // In standby: no output, arming refused with its reason, edits refused.
  expect((await backup.state()).output).toBeNull();
  const arm = await fetch(backup.url + '/api/arm', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ on: true }) });
  expect(arm.ok).toBe(false);
  expect(await arm.text()).toContain('Machine de secours en veille');
  expect(await backup.post('/api/settings', {})).toBe(423);
  expect((await backup.state()).armed).toBe(false);

  // A show and a cue on the main machine.
  const catalog = await main.presets();
  const show = {
    name: 'Secours e2e', time_base: 'seconds', tempo_map: [{ at_s: 0, bpm: 120, beats_per_bar: 4 }],
    tracks: [{ name: 'Piste 1', layer: 1, events: [{ id: 1, start: 0, len: 120, source: { kind: 'cue', id: catalog.presets[0].id } }] }],
  };
  expect(await main.post('/api/shows', show)).toBe(200);
  expect(await main.post('/api/timeline/load', { name: 'Secours e2e' })).toBe(200);
  expect(await main.post('/api/timeline/play')).toBe(200);
  expect(await main.post('/api/cue', { id: catalog.presets[1].id })).toBe(200);
  const want = (await main.frame()).active_cue;
  expect(want).not.toBeNull();

  // The backup plays the same: cue, show, playhead within 100 ms.
  await expect.poll(async () => (await backup.frame()).active_cue, { timeout: 5000 }).toBe(want);
  await expect.poll(async () => (await backup.get<State>('/api/state')).timeline.playing, { timeout: 5000 }).toBe(true);
  await expect.poll(async () => {
    const t0 = Date.now();
    const [a, b] = await Promise.all([main.get<State>('/api/state'), backup.get<State>('/api/state')]);
    const spread = (Date.now() - t0) / 1000;
    return Math.abs(a.timeline.position - b.timeline.position) <= 0.1 + spread;
  }, { timeout: 5000 }).toBe(true);
  const f = await backup.frame();
  expect(f.armed).toBe(false);
  expect(f.points.length).toBeGreaterThan(0);

  // The main machine stops: the backup says so and keeps playing, disarmed.
  expect(await backup.post('/api/standby', { takeover_grace_s: 1 })).toBe(200);
  await main.stop();
  await expect(page.locator('#standbyPill')).toContainText('Lien perdu depuis', { timeout: 5000 });
  await page.waitForTimeout(1200);
  expect((await backup.get<State>('/api/state')).timeline.playing).toBe(true);

  // Too early: the main machine's lease (3 s) plus the margin (1.5 s) has
  // not run out, whatever the 1 s grace says. Refused, still in standby.
  const lost = await backup.get<Standby>('/api/standby');
  expect(lost.takeover_after_s).toBeGreaterThanOrEqual(5);
  expect(lost.lost_s ?? 0).toBeLessThan(lost.takeover_after_s);
  page.once('dialog', d => { d.accept().catch(() => {}); });
  const early = page.waitForResponse(r => r.url().endsWith('/api/standby/takeover'));
  await page.locator('#takeoverBtn').click();
  const refused = await early;
  expect(refused.status()).toBe(409);
  expect(await refused.text()).toContain('reprise possible après');
  expect((await backup.get<Standby>('/api/standby')).role).toBe('standby');
  await expect.poll(async () => (await backup.get<Standby>('/api/standby')).lost_s ?? 0, { timeout: 10000 }).toBeGreaterThanOrEqual(lost.takeover_after_s);

  // « Prendre le relais »: confirmed (with the hardware check), then main,
  // still disarmed.
  let asked = '';
  page.once('dialog', d => { asked = d.message(); d.accept().catch(() => {}); });
  await page.locator('#takeoverBtn').click();
  await expect(page.locator('#standbyPill')).toHaveText(/^PRINCIPALE/);
  expect(asked).toContain('Le laser restera désarmé');
  expect(asked).toContain('Vérifiez d\'abord que la machine principale est arrêtée ou que sa sortie est coupée (arrêt matériel).');
  expect(asked).toMatch(/tenant une sortie|tenait une sortie/);
  await expect(page.locator('#takeoverBtn')).toBeHidden();
  const after = await backup.get<State>('/api/state');
  expect(after.standby.role).toBe('primary');
  expect(after.armed).toBe(false);
  expect(after.timeline.playing).toBe(true);
  expect((await backup.frame()).active_cue).toBe(want);
  await expect(page.locator('#armBtn')).toHaveAttribute('aria-pressed', 'false');
});

test('Autonome by default: no sync port, the panel says so', async ({ page }) => {
  // A fresh studio (the other one took over in the test above).
  const s = await main.get<Standby>('/api/standby').catch(() => null);
  if (s === null) await main.start();
  expect(await main.post('/api/standby', { role: 'standalone' })).toBe(200);
  const v = await main.get<Standby>('/api/standby');
  expect(v.role).toBe('standalone');
  expect(v.bound_port).toBeNull();
  await openUi(page, main);
  await expect(page.locator('#standbyPill')).toBeHidden();
  await expect(page.locator('#takeoverBtn')).toBeHidden();
  await openWorkspace(page, 'settings');
  await page.locator('#standbyPanel > summary').click();
  await expect(page.locator('#sbRole')).toHaveValue('standalone');
  await expect(page.locator('#sbLink')).toContainText('aucun port ouvert');
  // What darkens the main machine, and the way out.
  await expect(page.locator('#standbyPanel')).toContainText('Quitter, mettre en veille ou redémarrer la machine de secours désarme la principale 3 s plus tard');
  await expect(page.locator('#standbyPanel')).toContainText('passez la principale en Autonome, puis réarmez');
});

test('a main machine whose backup goes silent disarms itself and says so', async ({ page }) => {
  // Both studios running again (the first test stopped the main one).
  const s = await main.get<Standby>('/api/standby').catch(() => null);
  if (s === null) await main.start();
  const port = await freePort();
  expect(await main.post('/api/standby', { role: 'primary', port })).toBe(200);
  const m = await main.get<Standby>('/api/standby');
  expect(await backup.post('/api/standby', { role: 'standalone' })).toBe(200);
  expect(await backup.post('/api/standby', { peer: `127.0.0.1:${port}`, role: 'standby' })).toBe(200);
  expect(await backup.post('/api/standby/pair', { code: m.code })).toBe(200);
  await expect.poll(async () => (await backup.get<Standby>('/api/standby')).link, { timeout: 5000 }).toBe(true);
  await openUi(page, main);
  await expect(page.locator('#standbyPill')).toHaveText(/^PRINCIPALE$/);
  expect((await main.get<Standby>('/api/standby')).backup_lost).toBe(false);

  // The backup stops polling: after the lease, the main machine warns,
  // stays main and disarmed (it never arms by itself).
  await backup.stop();
  await expect(page.locator('#standbyPill')).toContainText('Secours injoignable', { timeout: 8000 });
  await expect(page.locator('#standbyPill')).toHaveClass(/warn/);
  const after = await main.get<State>('/api/state');
  expect(after.standby.role).toBe('primary');
  expect(after.standby.backup_lost).toBe(true);
  expect(after.armed).toBe(false);

  // Re-arming while the backup is still unreachable: refused without the
  // operator's confirmation of the risk.
  const refused = await fetch(main.url + '/api/arm', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ on: true }) });
  expect(refused.status).toBe(409);
  const why = await refused.json();
  expect(why.blocking[0]).toContain('confirmation requise');
  expect(why.confirm_backup_lost).toContain('elle a peut-être pris le relais');
  expect((await main.state()).armed).toBe(false);
  // Declined: still disarmed. Accepted: armed.
  page.once('dialog', d => { d.dismiss().catch(() => {}); });
  await page.locator('#armBtn').click();
  await page.waitForTimeout(500);
  expect((await main.state()).armed).toBe(false);
  let asked = '';
  page.once('dialog', d => { asked = d.message(); d.accept().catch(() => {}); });
  await page.locator('#armBtn').click();
  await expect(page.locator('#armBtn')).toHaveAttribute('aria-pressed', 'true');
  expect(asked).toContain('Tant qu\'elle reste injoignable, cette machine ne se désarmera plus d\'elle-même.');
  expect((await main.state()).armed).toBe(true);
  expect(await main.post('/api/arm', { on: false })).toBe(200);
  expect((await main.state()).armed).toBe(false);
});
