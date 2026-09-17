import { test, expect } from '@playwright/test'

test.describe('Pricing page', () => {
  test('has correct title and heading', async ({ page }) => {
    await page.goto('/pricing')
    await expect(page).toHaveTitle(/Pricing.*Cmdr/i)

    const heading = page.locator('h1')
    await expect(heading).toBeVisible()
    await expect(heading).toContainText(/free for personal use/i)
  })

  test('displays both pricing tiers', async ({ page }) => {
    await page.goto('/pricing')

    // Check for all tier names (use exact match to avoid matching h1)
    await expect(page.getByRole('heading', { name: 'Personal', exact: true })).toBeVisible()
    await expect(page.getByRole('heading', { name: 'Commercial', exact: true })).toBeVisible()
  })

  test('shows correct prices', async ({ page }) => {
    await page.goto('/pricing')

    const content = await page.textContent('body')
    expect(content).toContain('Free')
    expect(content).toContain('$59')
    expect(content).toContain('$39')
    // The retired tiers must not creep back in through a half-finished copy edit. $59 is
    // deliberately absent from this list: it's the new one-time price, and the retired
    // subscription happened to carry the same number, so only "/year" next to it is wrong.
    expect(content).not.toContain('$59/year')
    expect(content).not.toContain('$199')
  })

  test('has download button', async ({ page }) => {
    await page.goto('/pricing')

    // Use the main download button (with "Download Cmdr" text)
    const downloadButton = page.getByRole('link', { name: /download cmdr/i })
    await expect(downloadButton).toBeVisible()
    await expect(downloadButton).toHaveAttribute('href', /\.dmg$/)
  })

  test('buy button is present and clickable', async ({ page }) => {
    await page.goto('/pricing')

    // Find the buy button by its text content
    const commercialButton = page.getByRole('button', { name: /buy commercial/i })

    await expect(commercialButton).toBeVisible()

    // Verify the button has the correct data attribute for Paddle
    await expect(commercialButton).toHaveAttribute('data-paddle-price', 'commercial')
  })

  test('clicking buy button opens org name modal (when Paddle configured)', async ({ page }) => {
    await page.goto('/pricing')

    // Wait for page to be ready
    await page.waitForLoadState('domcontentloaded')

    // The buy buttons should be present
    const commercialButton = page.getByRole('button', { name: /buy commercial/i })
    await expect(commercialButton).toBeVisible()

    // Check if Paddle is configured (button will be enabled)
    const isEnabled = await commercialButton.isEnabled()

    if (isEnabled) {
      // With Paddle configured, clicking commercial opens our org name modal
      await commercialButton.click()

      // The org name modal should appear for commercial plans
      await expect(page.getByLabel(/company name/i)).toBeVisible()
      await expect(page.getByRole('textbox', { name: /company name/i })).toBeVisible()
    } else {
      // In CI without Paddle credentials, buttons are disabled - that's expected
      // Just verify the button structure is correct
      await expect(commercialButton).toHaveAttribute('data-paddle-price', 'commercial')
    }
  })

  test('FAQ section is present', async ({ page }) => {
    await page.goto('/pricing')

    await expect(page.getByRole('heading', { name: /frequently asked/i })).toBeVisible()

    // Check for some FAQ questions
    const content = await page.textContent('body')
    expect(content).toContain('commercial use')
    expect(content).toContain('multiple machines')
  })
})
