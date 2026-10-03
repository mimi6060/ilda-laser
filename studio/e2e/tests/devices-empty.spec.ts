// Sorties (T-303, T-401), as in CI: no output connected and the scan finds
// nothing (--test-devices '[]', never a real scan): the panel says so
// without an error.
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';

const studio = useStudio({ output: 'none' });

test('an empty scan shows « Aucune sortie trouvée » and no error', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', e => errors.push(e.message));
  await openUi(page, studio);
  await openWorkspace(page, 'settings');
  await expect(page.locator('#devList li')).toHaveText(['Pas encore cherché : cliquez sur Rechercher.']);
  await page.locator('#devScan').click();
  await expect(page.locator('#devList li')).toHaveText(['Aucune sortie trouvée.']);
  await expect(page.locator('#devStatus')).toHaveText('');
  await expect(page.locator('#outState')).toHaveText('Aucune sortie : aperçu seulement');
  await expect(page.locator('#outDisconnect')).toBeDisabled();
  const d = await studio.get('/api/devices');
  expect(d).toMatchObject({ devices: [], scanned: true, scanning: false, error: null });
  expect(errors).toEqual([]);
  expect((await studio.state()).armed).toBe(false);
});
