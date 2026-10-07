// 3D objects (T-462): CRÉATION › Objets 3D. A cube written here (our own
// text, no third-party model) is previewed in the import dialog (12 edges
// with the coplanar option, 18 without), imported, played as a cue of the
// « Objets 3D » page that turns with time within the point budget, edited
// and kept across a restart; an invalid file gives a French message and
// saves nothing. Preview only: the laser stays disarmed.
import { test, expect, useStudio, openUi, openWorkspace, isLit, type Studio, type Point } from '../studio';
import type { Page } from '@playwright/test';

// A cube exported as 12 triangles, with the index forms OBJ allows.
const CUBE = [
  '# un cube écrit à la main', 'mtllib cube.mtl', 'o Cube',
  'v -1 -1 -1', 'v 1 -1 -1', 'v 1 1 -1', 'v -1 1 -1', 'v -1 -1 1', 'v 1 -1 1', 'v 1 1 1', 'v -1 1 1',
  'vn 0 0 1', 'vt 0 0', 'usemtl rouge', 's off',
  'f 1/1 2/1 3/1', 'f 1//1 3//1 4//1', 'f 5/1/1 8/1/1 7/1/1', 'f 5 7 6', 'f 1 5 6', 'f 1 6 2',
  'f 2 6 7', 'f 2 7 3', 'f 3 7 8', 'f 3 8 4', 'f 5 1 4', 'f -4 -5 -1',
].join('\r\n') + '\r\n';

async function openObjects(page: Page, studio: Studio) {
  await openUi(page, studio);
  await openWorkspace(page, 'creation');
  await page.locator('#creationTabs [data-ctab="objects"]').click();
  await expect(page.locator('#objPanel')).toBeVisible();
  // The rights reminder is always on screen.
  await expect(page.locator('#objRights')).toContainText('dont vous avez les droits');
}

const lit = (pts: Point[]) => pts.filter(isLit);

test.describe('objets 3D', () => {
  const studio = useStudio();

  test.beforeEach(async () => {
    await studio.reset();
    await studio.post('/api/control', { id: 'cue.stop_all' });
  });

  test('a cube is previewed, imported, turns as a cue within the budget and survives a restart', async ({ page }) => {
    await openObjects(page, studio);
    await expect(page.locator('#objLib')).toContainText('Aucun objet 3D importé');
    await page.locator('#objImport').click();
    await expect(page.locator('#objImportDlg')).toBeVisible();

    // An invalid file: a French message, nothing to import, nothing saved.
    await page.locator('#objFile').setInputFiles({ name: 'vide.obj', mimeType: 'text/plain', buffer: Buffer.from('# rien\nvt 0 0\n') });
    await expect(page.locator('#objImpMsg')).toContainText('Fichier OBJ illisible : aucun sommet.');
    await expect(page.locator('#objImpMsg')).toHaveClass(/err/);
    await expect(page.locator('#objImpOk')).toBeDisabled();
    await page.locator('#objFile').setInputFiles({ name: 'binaire.obj', mimeType: 'application/octet-stream', buffer: Buffer.from([0, 1, 2, 3, 0x76]) });
    await expect(page.locator('#objImpMsg')).toContainText('pas un fichier texte');

    // The cube: 12 edges with the coplanar option, 18 without.
    await page.locator('#objFile').setInputFiles({ name: 'Mon cube.obj', mimeType: 'text/plain', buffer: Buffer.from(CUBE) });
    await expect(page.locator('#objReport')).toContainText('Sommets : 8 · Faces : 12 · Arêtes : 12 · Points estimés :');
    await expect(page.locator('#objImpOk')).toBeEnabled();
    await page.locator('#objCoplanar').uncheck();
    await expect(page.locator('#objReport')).toContainText('Arêtes : 18');
    await expect(page.locator('#objAngle')).toBeDisabled();
    await page.locator('#objCoplanar').check();
    await expect(page.locator('#objReport')).toContainText('Arêtes : 12');
    await expect(page.locator('#objImpName')).toHaveValue('Mon cube');
    expect((await studio.get('/api/objects')).objects, 'a preview saves nothing').toEqual([]);

    await page.locator('#objImpOk').click();
    await expect(page.locator('#objImportDlg')).toBeHidden();
    await expect(page.locator('#objMsg')).toContainText('« Mon cube » importé : 12 arêtes');
    await expect(page.locator('#objLib .objitem[data-obj="Mon cube"]')).toContainText('12 arêtes');
    await expect(page.locator('#objProps')).toBeVisible();
    const presets = await studio.presets();
    expect(presets.categories).toContain('Objets 3D');
    expect(presets.presets.find(p => p.id === 'object:Mon cube')).toMatchObject({ name: 'Mon cube', category: 'Objets 3D' });

    // Played: the normal look path; it turns with time and stays within the budget, never armed.
    await page.locator('#objPlay').click();
    await expect(page.locator('#objMsg')).toContainText('joué');
    await expect.poll(async () => (await studio.state()).settings.content.kind).toBe('wire');
    const { budget } = await studio.get('/api/objects');
    await expect.poll(async () => lit((await studio.frame()).points).length).toBeGreaterThan(10);
    const first = await studio.frame();
    await expect.poll(async () => {
      const f = await studio.frame();
      const [a, b] = [lit(first.points)[0], lit(f.points)[0]];
      return Math.hypot(a[0] - b[0], a[1] - b[1]);
    }, { message: 'the cube turns' }).toBeGreaterThan(0.01);
    for (let i = 0; i < 5; i++) {
      const f = await studio.frame();
      expect(f.armed).toBe(false);
      expect(f.points.length).toBeLessThanOrEqual(budget);
      expect(f.points.every(p => Math.abs(p[0]) <= 1 && Math.abs(p[1]) <= 1)).toBe(true);
      await new Promise(r => setTimeout(r, 50));
    }

    // Its properties: saved (and undoable).
    await page.locator('#objColor').fill('#ff0000');
    await page.locator('#objPersp').fill('6');
    await page.locator('#objSave').click();
    await expect(page.locator('#objMsg')).toContainText('« Mon cube » enregistré');
    await expect.poll(async () => (await studio.get('/api/objects')).objects[0]).toMatchObject({ color: [255, 0, 0], perspective: 6 });

    // Kept across a restart.
    await studio.restart();
    await expect.poll(async () => (await studio.get('/api/objects')).objects.map((o: { name: string }) => o.name)).toEqual(['Mon cube']);
    expect((await studio.presets()).presets.some(p => p.id === 'object:Mon cube')).toBe(true);
    expect((await studio.frame()).armed).toBe(false);
    await openObjects(page, studio);
    await expect(page.locator('#objLib .objitem[data-obj="Mon cube"]')).toBeVisible();

    // Deleted: gone from the list and the cue page.
    await page.locator('#objLib .objitem[data-obj="Mon cube"] .n').click();
    page.once('dialog', d => d.accept());
    await page.locator('#objDelete').click();
    await expect(page.locator('#objMsg')).toContainText('supprimé');
    await expect(page.locator('#objLib')).toContainText('Aucun objet 3D importé');
    expect((await studio.presets()).presets.some(p => p.id === 'object:Mon cube')).toBe(false);
  });

  test('show mode locks the import; the list still reads', async () => {
    expect(await studio.post('/api/lock', { show_mode: true })).toBe(200);
    const r = await fetch(studio.url + '/api/objects/import?name=x', { method: 'POST', body: CUBE });
    expect(r.status).toBe(423);
    expect((await studio.get('/api/objects')).objects).toEqual([]);
    expect(await studio.post('/api/lock', { show_mode: false })).toBe(200);
  });
});
