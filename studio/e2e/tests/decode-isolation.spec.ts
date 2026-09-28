// Decoding isolation (T-298): a song file that crashes the decoder (a
// panic, an abort, a hang) is refused with a clear French message while the
// studio keeps running and the laser stays armed. The studio runs with
// --test-hooks, which makes files starting with LSPANIC!, LSABORT! or
// LSHANG!! crash the decoder child on purpose, and a 2 s decoding limit.
// Preview only (no --device): "armed" here only drives the preview gate.
import { readdirSync } from 'node:fs';
import path from 'node:path';
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';

const studio = useStudio({ testHooks: true, decodeTimeoutMs: 2000 });
const arm = () => studio.get<{ armed: boolean; last_disarm: { reason: string } | null }>('/api/arm');
const armed = async () => (await arm()).armed;
const songs = () => {
  try { return readdirSync(path.join(studio.dataDir, 'media', 'audio')).filter(f => f !== '.peaks'); } catch { return []; }
};

/** A 16-bit mono WAV of a sine. */
function sineWav(seconds: number, rate = 22_050): Buffer {
  const n = Math.round(seconds * rate);
  const b = Buffer.alloc(44 + 2 * n);
  b.write('RIFF', 0); b.writeUInt32LE(36 + 2 * n, 4); b.write('WAVE', 8);
  b.write('fmt ', 12); b.writeUInt32LE(16, 16); b.writeUInt16LE(1, 20); b.writeUInt16LE(1, 22);
  b.writeUInt32LE(rate, 24); b.writeUInt32LE(rate * 2, 28); b.writeUInt16LE(2, 32); b.writeUInt16LE(16, 34);
  b.write('data', 36); b.writeUInt32LE(2 * n, 40);
  for (let i = 0; i < n; i++) b.writeInt16LE(Math.round(0.5 * 32767 * Math.sin(2 * Math.PI * 440 * i / rate)), 44 + 2 * i);
  return b;
}

test.beforeAll(async () => {
  const cue = (await studio.presets()).presets[0].id;
  expect(await studio.post('/api/shows', {
    name: 'Isolation e2e', time_base: 'seconds',
    tracks: [{ name: 'Piste 1', layer: 1, events: [{ id: 1, start: 0, len: 2, source: { kind: 'cue', id: cue } }] }],
  })).toBe(200);
});

test('a crashing decoder is refused with a clear message; the armed studio carries on', async ({ page }) => {
  test.setTimeout(60_000);
  await openUi(page, studio);
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(true);
  await openWorkspace(page, 'timeline');
  await page.locator('#tlShow').selectOption('Isolation e2e');

  const cases: [string, string, string][] = [
    ['piège.mp3', 'LSPANIC!', 'le décodeur a planté'],
    ['abort.mp3', 'LSABORT!', 'arrêté brutalement'],
    ['lent.mp3', 'LSHANG!!', 'décodage trop long (plus de 2 s)'],
  ];
  for (const [name, magic, reason] of cases) {
    const started = Date.now();
    await page.locator('#tlImportFile').setInputFiles({ name, mimeType: 'audio/mpeg', buffer: Buffer.from(magic + ' crafted file') });
    await expect(page.locator('#tlMsg')).toContainText('Import refusé', { timeout: 10_000 });
    await expect(page.locator('#tlMsg')).toContainText(reason);
    await expect(page.locator('#tlMsg')).toContainText('le studio continue');
    expect(Date.now() - started).toBeLessThan(8_000);
    // Still running, still armed (the heartbeats kept flowing), nothing kept.
    expect(await armed()).toBe(true);
    expect((await studio.frame()).points.length).toBeGreaterThan(0);
    expect(songs()).toEqual([]);
  }

  // A real song still imports afterwards, through the same decoder child.
  await page.locator('#tlImportFile').setInputFiles({ name: 'Bon.wav', mimeType: 'audio/wav', buffer: sineWav(1) });
  await expect.poll(songs).toEqual(['Bon.wav']);
  await expect(page.locator('#tlMsg')).not.toContainText('Import refusé');
  expect(await armed()).toBe(true);
  await page.locator('#armBtn').click();
  await expect.poll(armed).toBe(false);
  expect((await arm()).last_disarm?.reason).not.toBe('ui_lost');
});
