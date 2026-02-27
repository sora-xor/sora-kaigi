import { expect, test } from '@playwright/test';

test.describe('Kaigi Web UI E2E', () => {
  test('edits meeting config and policy toggles', async ({ page }) => {
    await page.goto('/');

    await expect(page.getByTestId('kaigi.header.title')).toHaveText('Kaigi Web Client');

    const roomId = page.getByTestId('kaigi.config.room_id');
    await roomId.fill('ui-e2e-room');
    await expect(roomId).toHaveValue('ui-e2e-room');

    const participantName = page.getByTestId('kaigi.config.participant_name');
    await participantName.fill('UI E2E Tester');
    await expect(participantName).toHaveValue('UI E2E Tester');

    const signedModeration = page.getByTestId('kaigi.config.require_signed_moderation');
    await expect(signedModeration).toBeChecked();
    await signedModeration.uncheck();
    await expect(signedModeration).not.toBeChecked();
    await signedModeration.check();
    await expect(signedModeration).toBeChecked();
  });

  test('exercises connect, disconnect, fallback, and recover controls', async ({ page }) => {
    await page.goto('/');

    const phase = page.getByTestId('kaigi.state.phase');
    await expect(phase).toHaveText('Disconnected');

    await page.getByTestId('kaigi.controls.connect').click();
    await expect(phase).not.toHaveText('Disconnected', { timeout: 10_000 });

    await page.getByTestId('kaigi.controls.disconnect').click();
    await expect(phase).toHaveText('Disconnected');

    await page.getByTestId('kaigi.controls.trigger_fallback').click();
    await expect(phase).toHaveText('FallbackActive');

    await page.getByTestId('kaigi.controls.recover').click();
    await expect(phase).not.toHaveText('FallbackActive');

    await expect(page.getByTestId('kaigi.log.list')).toContainText('Connecting to');
  });
});
