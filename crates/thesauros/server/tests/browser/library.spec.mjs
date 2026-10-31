import { test, expect } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
const token = 'thesauros-browser-test-token-32-characters';
async function login(page) {
  await page.goto('/');
  await page.getByLabel('Access token').fill(token);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('searchbox', { name: 'Search photos' })).toBeVisible();
}
test('Topcoat sign-in and local assets work with the production CSP', async ({ page }) => {
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => { if (message.type() === 'error') errors.push(message.text()); });
  await login(page);
  expect(await page.evaluate(() => window.htmx.version)).toBe('2.0.11');
  const color = await page.locator('body').evaluate((element) => getComputedStyle(element).backgroundColor);
  expect(color).not.toBe('rgba(0, 0, 0, 0)');
  expect(errors).toEqual([]);
  await page.getByRole('button', { name: 'Sign out' }).click();
  await expect(page.getByRole('button', { name: 'Sign in' })).toBeVisible();
});
test('batch upload, native preview, HTMX search, rating, and RAW download', async ({ page }) => {
  await login(page);
  await page.getByLabel('Batch upload').setInputFiles(resolve('tests/fixtures/synthetic.dng'));
  await page.getByRole('button', { name: 'Upload RAW files' }).click();
  await expect(page.getByRole('status')).toContainText('1 file queued');
  const card = page.getByRole('article').filter({ has: page.getByRole('heading', { name: 'synthetic.dng' }) });
  await expect(card).toBeVisible();
  await expect(card.getByAltText('Preview of synthetic.dng')).toBeVisible({ timeout: 20_000 });
  await expect(card).toContainText('Synthetic Camera');
  const image = card.getByRole('img');
  await expect.poll(() => image.evaluate((element) => element.complete && element.naturalWidth > 0)).toBe(true);
  await card.getByLabel('Rating for synthetic.dng').selectOption('5');
  await card.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByRole('status')).toContainText('Rating saved');
  await expect(card.getByLabel('Rating for synthetic.dng')).toHaveValue('5');
  await page.getByLabel('Search photos', { exact: true }).fill('nothing-matches-this');
  await expect(page.getByRole('article')).toHaveCount(0);
  await page.getByLabel('Search photos', { exact: true }).fill('synthetic');
  await expect(card).toBeVisible();
  const downloadEvent = page.waitForEvent('download');
  await card.getByRole('link', { name: 'Download RAW' }).click();
  const download = await downloadEvent;
  expect(download.suggestedFilename()).toBe('synthetic.dng');
  const downloaded = await download.path();
  expect(readFileSync(downloaded)).toEqual(readFileSync('tests/fixtures/synthetic.dng'));
});
test('phone layout keeps upload controls and the gallery inside the viewport', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await login(page);
  await page.getByRole('button', { name: 'Filters & tools', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Upload RAW files' })).toBeVisible();
  await page.getByRole('button', { name: /^Camera / }).click();
  await expect(page.getByRole('combobox', { name: 'Filter camera options' })).toBeVisible();
  await page.keyboard.press('Escape');
  await page.keyboard.press('Escape');
  await expect(page.getByRole('button', { name: 'Filters & tools', exact: true })).toBeFocused();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test('untrusted filenames remain text and cannot create HTML event handlers', async ({ page, request }) => {
  const raw = Buffer.from(readFileSync('tests/fixtures/synthetic.dng'));
  raw[raw.length - 1] ^= 1;
  const filename = 'evil"><img src=x onerror=alert(1)>.DNG';
  const response = await request.post('/api/upload', {headers: {Authorization: 'Bearer ' + token, 'X-Filename': encodeURIComponent(filename)}, data: raw});
  expect(response.status()).toBe(201);
  await login(page);
  await expect(page.getByRole('heading', {name: filename, exact: true})).toBeVisible();
  await expect(page.locator('[onerror]')).toHaveCount(0);
});

test('full-screen grid and searchable camera and lens selections', async ({ page, request }) => {
  const raw = Buffer.from(readFileSync('tests/fixtures/synthetic.dng'));
  for (const [before, after] of [['Synthetic Camera', 'Alternate Camera'], ['Synthetic 50mm f/2', 'Synthetic 35mm f/2']]) {
    let offset = 0;
    while ((offset = raw.indexOf(before, offset)) !== -1) {
      raw.write(after, offset); offset += before.length;
    }
  }
  const response = await request.post('/api/upload', { headers: { Authorization: 'Bearer ' + token, 'X-Filename': 'alternate.dng' }, data: raw });
  expect(response.status()).toBe(201);
  await login(page);
  const camera = page.getByRole('button', { name: /^Camera / });
  await expect.poll(async () => {
    await camera.click();
    const found = await page.getByRole('option', { name: 'Thesauros Alternate Camera', exact: true }).count();
    await page.keyboard.press('Escape');
    return found;
  }).toBe(1);
  await camera.click();
  const search = page.getByRole('combobox', { name: 'Filter camera options' });
  await search.fill('alternate');
  await expect(page.locator('#camera-select')).toHaveValue('');
  await expect(page.getByRole('listbox', { name: 'Camera options' }).getByRole('option')).toHaveCount(1);
  await search.press('Enter');
  await expect(page.getByRole('article')).toHaveCount(1);
  await expect(page.getByRole('heading', { name: 'alternate.dng', exact: true })).toBeVisible();
  await page.getByRole('button', { name: /^Lens / }).click();
  await page.getByRole('combobox', { name: 'Filter lens options' }).fill('50mm');
  await page.getByRole('option', { name: 'Synthetic 50mm f/2', exact: true }).click();
  await expect(page.getByRole('article')).toHaveCount(0);
  await page.getByRole('button', { name: /^Lens / }).click();
  await page.getByRole('combobox', { name: 'Filter lens options' }).fill('35mm');
  await page.keyboard.press('Enter');
  await expect(page.getByRole('article')).toHaveCount(1);
  expect(await page.evaluate(() => {
    const sidebar = document.querySelector('.sidebar').getBoundingClientRect();
    const workspace = document.querySelector('.workspace').getBoundingClientRect();
    const gallery = document.querySelector('#gallery');
    return workspace.x === 0 && workspace.width === innerWidth && Math.abs(workspace.bottom - innerHeight) < 1 && sidebar.height === workspace.height && getComputedStyle(gallery).overflowY === 'auto' && document.documentElement.scrollHeight <= innerHeight;
  })).toBe(true);
  await page.screenshot({ path: '/tmp/thesauros-grid-desktop.png' });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByRole('button', { name: 'Filters & tools', exact: true }).click();
  await camera.click();
  const box = await page.locator('[data-picker-menu="camera-picker"]').boundingBox();
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(390);
  await page.screenshot({ path: '/tmp/thesauros-grid-mobile.png' });
});
