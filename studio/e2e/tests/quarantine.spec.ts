// Damaged data files (T-402): moved to quarantine/, defaults used, and a
// banner that names the file. A damaged safety file blocks arming until
// « J'ai compris ». Preview only: "armed" comes from a studio with no output.
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';

const TRUNCATED = '[{"name": "coup';

test.describe('working files', () => {
  const studio = useStudio({ files: { 'scenes.json': TRUNCATED, 'grid.json': '{"max_active": 4,' } });

  test('each damaged file is named in its own banner, set aside, and does not block arming', async ({ page }) => {
    const dw = await studio.get('/api/data/warnings');
    expect(dw.warnings.map((w: { file: string }) => w.file).sort()).toEqual(['grid.json', 'scenes.json']);
    expect(dw.arm_blocked).toBe(false);
    for (const w of dw.warnings) expect(w.moved_to).toMatch(/^quarantine\/(scenes|grid)-\d{8}-\d{6}\.json$/);

    await openUi(page, studio);
    const banner = page.locator('#dataWarn');
    await expect(banner).toBeVisible();
    const scenes = banner.locator('.dw[data-file="scenes.json"]');
    await expect(scenes).toContainText('Fichier illisible : scenes.json');
    await expect(scenes).toContainText('quarantine/scenes-');
    await expect(scenes).toContainText('Valeurs par défaut utilisées.');
    await expect(scenes).not.toHaveClass(/safety/);

    await scenes.getByRole('button', { name: "J'ai compris" }).click();
    await expect(scenes).toHaveCount(0);
    await expect(banner.locator('.dw[data-file="grid.json"]')).toBeVisible();
    await banner.locator('.dw[data-file="grid.json"]').getByRole('button', { name: "J'ai compris" }).click();
    await expect(banner).toBeHidden();
    const after = await studio.get('/api/data/warnings');
    expect(after.warnings.every((w: { acknowledged: boolean }) => w.acknowledged)).toBe(true);
  });
});

test.describe('safety file', () => {
  const studio = useStudio({ files: { 'safety.json': '{"strobe_max_hz": 2, "zones": [' } });

  test('a damaged safety.json gives strict settings and refuses arming until acknowledged', async ({ page }) => {
    const st = await studio.state();
    expect(st.armed).toBe(false);
    expect(st.safety.dwell.profile).toBe('strict');
    expect(st.data_warnings.arm_blocked).toBe(true);

    await openUi(page, studio);
    const line = page.locator('#dataWarn .dw[data-file="safety.json"]');
    await expect(line).toHaveClass(/safety/);
    await expect(line).toContainText('Fichier illisible : safety.json');
    await expect(line).toContainText("L'armement est bloqué tant que vous n'avez pas validé.");

    await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
    await page.locator('#armBtn').click();
    await expect(page.locator('#armRefused')).toContainText('Fichier de sécurité abîmé : à valider');
    expect((await studio.state()).armed).toBe(false);

    // RÉGLAGES › Sécurité repeats the message.
    await openWorkspace(page, 'settings');
    await page.locator('#safetyPanel > summary').click();
    await expect(page.locator('#sfError')).toContainText('Fichier illisible : safety.json');
    await expect(line).toBeVisible();

    await line.getByRole('button', { name: "J'ai compris" }).click();
    await expect(page.locator('#dataWarn')).toBeHidden();
    await page.locator('#armBtn').click();
    await expect.poll(async () => (await studio.state()).armed).toBe(true);
    await studio.post('/api/arm', { on: false });
    expect((await studio.state()).armed).toBe(false);
  });
});
