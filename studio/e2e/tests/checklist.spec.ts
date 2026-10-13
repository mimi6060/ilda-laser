// Pre-show checklist (T-258). With a real output configured, arming waits
// until the list is completed; completing it never arms. Simulated here
// with the hidden --test-checklist flag: the studio stays preview only.
import { test, expect, useStudio, openUi, openWorkspace, focusPage } from '../studio';

const LABEL = 'Liste de contrôle avant show à faire';

test.describe('pure preview', () => {
  const studio = useStudio();

  test('no checklist interlock: the laser arms as before', async ({ page }) => {
    const st = await studio.state();
    expect(st.checklist).toEqual({ enforced: false, complete: false });
    await openUi(page, studio);
    await expect(page.locator('#armBtn')).toHaveText('LASER OFF');
    await page.locator('#armBtn').click();
    await expect.poll(async () => (await studio.state()).armed).toBe(true);
    await studio.post('/api/arm', { on: false });
    await openWorkspace(page, 'settings');
    await expect(page.locator('#clLed')).toHaveText('Aperçu : facultative');
  });
});

test.describe('simulated real output', () => {
  const studio = useStudio({ testChecklist: true });

  test('arming waits for the list, completing it never arms, restarting it disarms', async ({ page }) => {
    await openUi(page, studio);
    const armBtn = page.locator('#armBtn');
    await expect(armBtn).toHaveText('Liste de contrôle à faire');

    // Space asks anyway: refused with the reason.
    await focusPage(page);
    await page.keyboard.press('Space');
    await expect(page.locator('#armRefused')).toContainText(LABEL);
    expect((await studio.state()).armed).toBe(false);

    // The button opens the list.
    await armBtn.click();
    await expect(page.locator('#checklistPanel')).toHaveAttribute('open', '');
    await expect(page.locator('#clLed')).toHaveText("À faire avant d'armer");
    const items = page.locator('#clItems li');
    await expect(items).toHaveCount(9);
    await expect(page.locator('#clItems li[data-item="outdoor_permit"]')).toHaveCount(0);

    // Outdoor: the permit item appears and needs its reference.
    await page.locator('#clOutdoor').check();
    await expect(items).toHaveCount(10);
    const permit = page.locator('#clItems li[data-item="outdoor_permit"]');
    await permit.locator('input[type=checkbox]').click();
    await expect(page.locator('#clError')).toContainText("Référence de l'autorisation");
    await expect(permit.locator('input[type=checkbox]')).not.toBeChecked();
    await permit.locator('input[type=text]').fill('DGTA-2026-042');
    await permit.locator('input[type=checkbox]').check();
    await expect(permit).toHaveClass(/done/);

    // Incomplete: the missing items are listed.
    await page.locator('#clComplete').click();
    await expect(page.locator('#clError')).toContainText("Nom de l'opérateur");

    await page.locator('#clOperator').fill('Michel');
    const ids = await page.locator('#clItems li:not(.done)').evaluateAll(lis => lis.map(li => (li as HTMLElement).dataset.item!));
    expect(ids).toHaveLength(9);
    for (const id of ids) {
      const li = page.locator(`#clItems li[data-item="${id}"]`);
      await li.locator('input[type=checkbox]').click();
      await expect(li).toHaveClass(/done/);
    }
    await expect(page.locator('#clItems li.done')).toHaveCount(10);
    await page.locator('#clComplete').click();
    await expect(page.locator('#clLed')).toHaveText('Terminée');
    await expect(page.locator('#clDone')).toContainText('Liste terminée par Michel');
    await expect(armBtn).toHaveText('LASER OFF');
    expect((await studio.state()).armed).toBe(false);

    // The safety log has the operator, the items and the permit reference.
    const log = await studio.get('/api/safety/log');
    const done = log.events.find((e: { kind: string }) => e.kind === 'checklist');
    expect(done.detail.operator).toBe('Michel');
    expect(done.detail.items).toHaveLength(10);
    expect(done.detail.items.find((i: { id: string }) => i.id === 'outdoor_permit').value).toBe('DGTA-2026-042');
    expect(done.text).toContain('terminée par Michel');

    // T-255: with a real output, the audience must be located too.
    await armBtn.click();
    await expect(page.locator('#armRefused')).toContainText('Aucune zone Public');
    expect((await studio.state()).armed).toBe(false);
    await studio.declareAudience();

    await armBtn.click();
    await expect.poll(async () => (await studio.state()).armed).toBe(true);

    page.once('dialog', d => d.accept());
    await page.locator('#clRestart').click();
    await expect.poll(async () => (await studio.state()).armed).toBe(false);
    await expect(armBtn).toHaveText('Liste de contrôle à faire');
    await expect(page.locator('#clItems li.done')).toHaveCount(0);
    const arm = await studio.get('/api/arm');
    expect(arm.last_disarm.reason_fr).toBe(`Verrou : ${LABEL}`);
  });

  test('an edited list survives a restart, its ticks do not', async ({ page }) => {
    await openUi(page, studio);
    await openWorkspace(page, 'settings');
    await page.locator('#checklistPanel > summary').click();
    await page.locator('#clEdit').click();
    await page.locator('#clAdd').click();
    await page.locator('#clEditRows .cl-row').last().locator('textarea').fill('Extincteur CO2 à portée de main');
    await page.locator('#clSave').click();
    await expect(page.locator('#clEditor')).toBeHidden();
    await expect(page.locator('#clItems li', { hasText: 'Extincteur CO2' })).toHaveCount(1);

    await studio.completeChecklist();
    expect((await studio.get('/api/checklist')).complete).toBe(true);

    await studio.restart();
    const c = await studio.get('/api/checklist');
    expect(c.complete).toBe(false);
    expect(c.items.some((i: { text_fr: string }) => i.text_fr === 'Extincteur CO2 à portée de main')).toBe(true);
    expect(c.items.every((i: { done: boolean }) => !i.done)).toBe(true);
    expect((await studio.get('/api/arm')).blocking).toContain(LABEL);
  });
});
