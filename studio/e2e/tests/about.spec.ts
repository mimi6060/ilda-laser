// « À propos de Laser Studio » and the release notes (T-452): version,
// data folder, third-party licences, the notes shown once after an
// update and never on a first launch. Échap stays the e-stop with the
// window open. The page never contacts anything but the studio itself.
// Preview only.
import { readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import AxeBuilder from '@axe-core/playwright';
import { test, expect, useStudio, type Page } from '../studio';

const studio = useStudio();

/** Every request the page makes must stay on the studio's own address. */
function watchHosts(page: Page) {
  const outside: string[] = [];
  page.on('request', r => {
    const u = new URL(r.url());
    if (u.protocol === 'data:' || u.protocol === 'blob:') return;
    if (u.hostname !== '127.0.0.1') outside.push(r.url());
  });
  return outside;
}

/** Serious or critical axe findings inside the dialog. */
async function audit(page: Page, where: string) {
  const r = await new AxeBuilder({ page }).include('#aboutDialog').withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
  const bad = r.violations.filter(v => v.impact === 'serious' || v.impact === 'critical').map(v => `${where}: ${v.id} ${v.nodes.slice(0, 5).map(n => n.target.join(' ')).join(', ')}`);
  expect(bad, bad.join('\n')).toEqual([]);
}

const prefs = () => JSON.parse(readFileSync(path.join(studio.dataDir, 'prefs.json'), 'utf8'));

test('« À propos » shows version, folders and licences; Échap stays the e-stop', async ({ page }) => {
  const outside = watchHosts(page);
  const about = await studio.get('/api/about');
  // The first read on a new data folder recorded the version: nothing to show.
  expect(about.unseen).toBeNull();
  expect(prefs().about.last_seen_version).toBe(about.version);

  await page.goto(studio.url + '/');
  await expect(page.locator('#cues .cue').first()).toBeVisible();
  await expect(page.locator('#whatsNew')).toBeHidden();

  await page.locator('#projBtn').click();
  await page.locator('#projMenu [data-proj="about"]').click();
  const dlg = page.locator('#aboutDialog');
  await expect(dlg).toBeVisible();
  await expect(page.locator('#aboutVersion')).toContainText(`Laser Studio ${about.version}`);
  await expect(page.locator('#aboutBuild')).toHaveText(/^\d{4}-\d{2}-\d{2}$/);
  await expect(page.locator('#aboutDataDir')).toHaveText(studio.dataDir);
  await expect(page.locator('#aboutProjDir')).toHaveText(path.join(studio.dataDir, 'projects'));
  await expect(page.locator('#aboutFormat')).toHaveText(/^\d+$/);
  await expect(page.locator('#aboutSafety')).toContainText("ne remplace pas l'arrêt matériel");
  await expect(page.locator('#aboutNetwork')).toContainText('ne contacte aucun serveur');
  await expect(page.locator('#aboutThird table')).toContainText('Open Font License');
  await expect(page.locator('#aboutThird table')).toContainText('Hershey');
  await audit(page, 'À propos');

  // Release notes tab.
  await page.locator('#aboutTabNotes').click();
  await expect(page.locator('#aboutNotes')).toBeVisible();
  await expect(page.locator('#aboutInfo')).toBeHidden();
  await expect(page.locator('#aboutNotes h4').first()).toHaveText(new RegExp(`^Version ${about.version.replace(/\./g, '\\.')} — `));
  await expect(page.locator('#aboutNotes li').first()).toBeVisible();
  await audit(page, 'Notes de version');

  // Échap with the window open (focus inside it): the e-stop, the window stays.
  await page.locator('#aboutTabNotes').focus();
  await page.keyboard.press('Escape');
  await expect.poll(async () => (await studio.state()).estop).toBe(true);
  await expect(dlg).toBeVisible();
  expect(await studio.post('/api/estop/reset', {})).toBe(200);

  await page.locator('#aboutClose').click();
  await expect(dlg).toBeHidden();
  expect((await studio.state()).armed).toBe(false);
  expect(outside).toEqual([]);
});

test('after an update the new notes show once, not after « Fermer »', async ({ page }) => {
  const outside = watchHosts(page);
  const version = (await studio.get('/api/about')).version;
  writeFileSync(path.join(studio.dataDir, 'prefs.json'), JSON.stringify({ ...prefs(), about: { last_seen_version: '0.0.1' } }));

  await page.goto(studio.url + '/');
  const box = page.locator('#whatsNew');
  await expect(box).toBeVisible();
  await expect(page.locator('#whatsNewTitle')).toHaveText(`Nouveautés de la version ${version}`);
  await expect(page.locator('#whatsNewBody li').first()).toBeVisible();
  // Shown until closed: a reload shows it again.
  await page.reload();
  await expect(box).toBeVisible();
  await page.locator('#whatsNewClose').click();
  await expect(box).toBeHidden();
  await expect.poll(() => prefs().about.last_seen_version).toBe(version);

  await page.reload();
  await expect(page.locator('#cues .cue').first()).toBeVisible();
  // Give the page's own check time to answer before asserting nothing shows.
  await expect.poll(async () => (await studio.get('/api/about')).unseen).toBeNull();
  await page.waitForTimeout(300);
  await expect(box).toBeHidden();
  expect(outside).toEqual([]);
});
