// DAC détectés (T-303), as in CI: the scan finds nothing (--test-devices
// '[]', never a real scan) and the panel says so without an error.
import { test, expect, useStudio, openUi, openWorkspace } from '../studio';

const studio = useStudio();

test('an empty scan shows « Aucun DAC trouvé » and no error', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', e => errors.push(e.message));
  await openUi(page, studio);
  await openWorkspace(page, 'settings');
  await page.locator('#devScan').click();
  await expect(page.locator('#devList li')).toHaveText(['Aucun DAC trouvé.']);
  await expect(page.locator('#devStatus')).toHaveText('');
  const d = await studio.get('/api/devices');
  expect(d).toMatchObject({ devices: [], scanned: true, scanning: false, error: null });
  expect(errors).toEqual([]);
  expect((await studio.state()).armed).toBe(false);
});
