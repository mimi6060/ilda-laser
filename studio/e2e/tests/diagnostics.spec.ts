// Application log and diagnostic report (T-403): RÉGLAGES › Diagnostic
// shows the log's tail and folder, and « Exporter un rapport de
// diagnostic » downloads a zip with about.json and no media; the project
// only when its box is ticked. Preview only.
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';

const studio = useStudio();

/** Entry names of a zip, read from its central directory. */
function zipNames(zip: Buffer): string[] {
  const end = zip.lastIndexOf(Buffer.from([0x50, 0x4b, 0x05, 0x06]));
  expect(end).toBeGreaterThan(0);
  const count = zip.readUInt16LE(end + 10);
  let at = zip.readUInt32LE(end + 16);
  const names: string[] = [];
  for (let i = 0; i < count; i++) {
    expect(zip.readUInt32LE(at)).toBe(0x02014b50);
    const len = zip.readUInt16LE(at + 28);
    names.push(zip.subarray(at + 46, at + 46 + len).toString('utf8'));
    at += 46 + len + zip.readUInt16LE(at + 30) + zip.readUInt16LE(at + 32);
  }
  return names;
}

async function exportReport(page: import('@playwright/test').Page): Promise<Buffer> {
  const download = page.waitForEvent('download');
  await page.locator('#diagExport').click();
  const d = await download;
  expect(d.suggestedFilename()).toMatch(/^laser-studio-diagnostic-\d{4}-\d{2}-\d{2}\.zip$/);
  return readFileSync(await d.path());
}

test.beforeEach(async ({ page }) => {
  await openUi(page, studio);
  await openWorkspace(page, 'settings');
  await page.locator('#diagPanel > summary').click();
});

test('the panel shows the log folder and the start line', async ({ page }) => {
  await expect(page.locator('#diagDir')).toContainText('logs');
  await expect(page.locator('#diagTail')).toContainText('Démarrage de Laser Studio');
  await expect(page.locator('#diagWarn')).toHaveText('');
  const st = await studio.state();
  expect(st.applog.enabled).toBe(true);
  expect(st.applog.write_failed).toBe(false);
  expect(st.armed).toBe(false);
});

test('export: a zip with about.json and no media; the project only when ticked', async ({ page }) => {
  // Media in the data directory are never picked up.
  mkdirSync(path.join(studio.dataDir, 'media', 'audio'), { recursive: true });
  writeFileSync(path.join(studio.dataDir, 'media', 'audio', 'chanson.wav'), 'RIFF');
  writeFileSync(path.join(studio.dataDir, 'logs', 'logo.svg'), '<svg/>');

  const zip = await exportReport(page);
  expect(zip.subarray(0, 2).toString()).toBe('PK');
  const names = zipNames(zip);
  for (const name of ['about.json', 'devices.json', 'settings.json', 'state.json']) expect(names).toContain(name);
  expect(names.some(n => n.startsWith('logs/app-'))).toBe(true);
  expect(names).not.toContain('project.json');
  for (const n of names) expect(n).not.toMatch(/\.(wav|mp3|flac|ild|svg|png|jpe?g)$/i);
  expect(zip.includes(Buffer.from(studio.dataDir))).toBe(false);

  await page.locator('#diagProject').check();
  const withProject = zipNames(await exportReport(page));
  expect(withProject).toContain('project.json');
  for (const n of withProject) expect(n).not.toMatch(/\.(wav|mp3|flac|ild|svg|png|jpe?g)$/i);
});
