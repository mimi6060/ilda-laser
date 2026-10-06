// ShowNET SD card bank (T-302) with the safety chain in the files (T-355):
// RÉGLAGES › Carte SD ShowNET. Two cues played from the grid are added
// with automatic numbers, a duration is edited, a reserved number is
// refused, the bank is renumbered and exported to a temporary folder
// (never the repository, never the ShowNET): 001.ild, 002.ild and
// banque.json, read back by the studio, nothing above the output's cap.
// Show mode refuses the export. Preview only: the laser stays disarmed.
import { test, expect, useStudio, openUi, reveal, openWorkspace } from '../studio';
import { mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';

const studio = useStudio();

interface Slot { number: number; cue_id: string; seconds: number; name: string | null; file: string }

test('two cues are numbered, exported to a folder and read back, laser disarmed', async ({ page }) => {
  const out = mkdtempSync(path.join(tmpdir(), 'laser-studio-e2e-sd-'));
  try {
    await studio.reset();
    await openUi(page, studio);
    const cues = page.locator('#cues .cue');
    const [first, second] = [await cues.nth(0).getAttribute('data-id'), await cues.nth(1).getAttribute('data-id')];

    // Nothing in the bank yet.
    await reveal(page, '#sdBankPanel');
    await expect(page.locator('#sdEmpty')).toBeVisible();

    // Play a cue, add it; play another, add it: 001 then 002.
    for (const id of [first, second]) {
      await openWorkspace(page, 'live');
      await page.locator(`#cues .cue[data-id="${id}"]`).click();
      await expect.poll(async () => (await studio.frame()).active_cue).toBe(id);
      await reveal(page, '#sdBankPanel');
      await page.locator('#sdAdd').click();
      await expect(page.locator('#sdMsg')).toContainText('ajouté');
    }
    await expect(page.locator('#sdRows tr')).toHaveCount(2);
    let bank = await studio.get<{ slots: Slot[] }>('/api/sdbank');
    expect(bank.slots.map(s => [s.number, s.cue_id])).toEqual([[1, first], [2, second]]);

    // A shorter duration is saved; a reserved number is refused and undone.
    await page.locator('#sdRows tr').nth(1).locator('.sd-secs').fill('0.5');
    await page.locator('#sdRows tr').nth(1).locator('.sd-secs').dispatchEvent('change');
    await expect.poll(async () => (await studio.get<{ slots: Slot[] }>('/api/sdbank')).slots[1].seconds).toBe(0.5);
    await page.locator('#sdRows tr').nth(1).locator('.sd-num').fill('230');
    await page.locator('#sdRows tr').nth(1).locator('.sd-num').dispatchEvent('change');
    await expect(page.locator('#sdMsg')).toContainText('réservés');
    await expect(page.locator('#sdRows tr').nth(1).locator('.sd-num')).toHaveValue('2');

    // A hole closed by « Numéroter automatiquement ».
    await page.locator('#sdRows tr').nth(1).locator('.sd-num').fill('40');
    await page.locator('#sdRows tr').nth(1).locator('.sd-num').dispatchEvent('change');
    await expect(page.locator('#sdRows tr').nth(1)).toHaveAttribute('data-number', '40');
    await page.locator('#sdRenumber').click();
    await expect(page.locator('#sdRows tr').nth(1)).toHaveAttribute('data-number', '2');

    // Export to the temporary folder, the main output's safety chain inside.
    await expect(page.locator('#sdOutput')).toHaveValue('main');
    // The points per frame baked into the files are shown with the output (T-476).
    await expect(page.locator('#sdOutput option[value="main"]')).toContainText('points par image');
    await page.locator('#sdDir').fill(out);
    await page.locator('#sdExport').click();
    await expect(page.locator('#sdMsg')).toContainText('2 fichiers exportés', { timeout: 30_000 });
    await expect(page.locator('#sdMsg')).toContainText('main');
    expect(readdirSync(out).sort()).toEqual(['001.ild', '002.ild', 'banque.json']);
    const index = JSON.parse(readFileSync(path.join(out, 'banque.json'), 'utf8'));
    expect(index.files.map((f: { number: number; cue_id: string }) => [f.number, f.cue_id])).toEqual([[1, first], [2, second]]);
    expect(index.files[1].frames).toBe(25); // 0.5 s at 50 images/s

    // Read back by the studio's own reader (T-001): a format 5 file.
    const bytes = readFileSync(path.join(out, '002.ild'));
    expect(bytes[7]).toBe(5);
    const r = await fetch(studio.url + '/api/media/ilda?name=sdcheck.ild', { method: 'POST', body: bytes });
    expect(r.status).toBe(200);
    expect(await r.json()).toMatchObject({ frames: 25 });
    // Never brighter than the output's cap (50 % by default): every colour
    // byte of every point record is at most 128 (0.5 rounded down: 127).
    const lim = await studio.get('/api/outputs/limits');
    const cap = lim.outputs.find((o: { id: string }) => o.id === 'main').limits.max_power;
    let max = 0, at = 0;
    while (at + 32 <= bytes.length) {
      const records = bytes.readUInt16BE(at + 24);
      if (records === 0) break;
      for (let i = 0; i < records; i++) {
        const rec = at + 32 + i * 8;
        max = Math.max(max, bytes[rec + 5], bytes[rec + 6], bytes[rec + 7]);
      }
      at += 32 + records * 8;
    }
    expect(max).toBeGreaterThan(0);
    expect(max / 255).toBeLessThanOrEqual(cap + 1e-6);

    // Show mode: the bank can't be exported.
    await studio.post('/api/lock', { show_mode: true });
    expect(await studio.post('/api/sdbank/export', { dir: out })).toBe(423);
    await studio.post('/api/lock', { show_mode: false });
    expect((await studio.frame()).armed).toBe(false);
    bank = await studio.get('/api/sdbank');
    expect(bank.slots).toHaveLength(2);
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
});
