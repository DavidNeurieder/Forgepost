import { expect, test, type Browser } from '@playwright/test';

// Full creator journey through the real UI against the real `openpublish`
// server: setup -> write/publish -> external read + comment -> moderation ->
// analytics -> experiments -> logout/login. Runs serially because later steps
// depend on earlier ones mutating shared state, and shares the owner session
// (saved to .auth.json after setup) since Playwright gives each test a fresh
// browser context.
//
// Note: assertions wait for data-dependent content (not the SSR-rendered h1)
// so the app has finished hydrating before we interact with it.

const AUTH_FILE = 'e2e/.auth.json';
const EMAIL = 'e2e@example.com';
const PASSWORD = 'correct horse battery staple';
const DISPLAY = 'E2E Owner';
const TITLE = 'Hello from E2E';
const MARKDOWN = [
	'# Hello from E2E',
	'',
	'This is the first paragraph of the post.',
	'',
	'## Section two',
	'',
	'More content lives here.',
	''
].join('\n');

test.describe.configure({ mode: 'serial' });

let docId = '';
let slug = '';

async function adminPage(browser: Browser) {
	const context = await browser.newContext({ storageState: AUTH_FILE });
	return context.newPage();
}

async function gotoDashboard(page: import('@playwright/test').Page) {
	await page.goto('/admin');
	await expect(page.getByText(`Signed in as ${DISPLAY}`)).toBeVisible();
}

test('first-run setup creates the owner account', async ({ page }) => {
	await page.goto('/setup');
	await expect(page.getByLabel('Email', { exact: true })).toBeVisible();

	await page.locator('#email').fill(EMAIL);
	await page.locator('#display').fill(DISPLAY);
	await page.locator('#password').fill(PASSWORD);
	await page.locator('#confirm').fill(PASSWORD);
	await page.getByRole('button', { name: 'Create account' }).click();

	await expect(page).toHaveURL(/\/admin$/);
	await expect(page.getByText(`Signed in as ${DISPLAY}`)).toBeVisible();

	await page.context().storageState({ path: AUTH_FILE });
});

test('the owner creates and publishes a post', async ({ browser }) => {
	const page = await adminPage(browser);
	await gotoDashboard(page);

	await page.getByRole('button', { name: 'New post' }).click();
	await page.waitForURL(/\/admin\/editor\//);

	let published: { id?: string; slug?: string } = {};
	await page.route('**/api/documents/*/publish', async (route) => {
		const response = await route.fetch();
		published = await response.json();
		await route.fulfill({ response });
	});

	await page.getByLabel('Title', { exact: true }).fill(TITLE);
	await page.getByLabel('Markdown').fill(MARKDOWN);
	await page.getByRole('button', { name: 'Save' }).click();
	await expect(page.getByText('Saved')).toBeVisible();

	await page.getByRole('button', { name: 'Publish' }).click();
	await expect(page.getByText('Published', { exact: true })).toBeVisible();
	await expect(page.locator('.badge')).toHaveText('published');

	docId = published.id ?? '';
	slug = published.slug ?? '';
	expect(docId).not.toBe('');
	expect(slug).not.toBe('');
	await page.context().close();
});

test('external readers can view the article and leave a comment', async ({ browser }) => {
	expect(slug).not.toBe('');
	const context = await browser.newContext();
	const page = await context.newPage();

	await page.goto(`/articles/${slug}`);
	await expect(page.locator('h1').first()).toHaveText(TITLE);
	await expect(page.locator('[data-block-id]').first()).toBeVisible();

	await page.getByLabel('Name', { exact: true }).fill('Reader');
	await page.getByLabel('Comment', { exact: true }).fill('Nice post!');
	await page.getByRole('button', { name: 'Post comment' }).click();
	await expect(page.getByText('Thanks! Your comment is awaiting moderation.')).toBeVisible();

	await context.close();
});

test('the owner approves the comment', async ({ browser }) => {
	const page = await adminPage(browser);
	await gotoDashboard(page);
	await expect(page.getByText('Reader')).toBeVisible();
	await page.getByRole('button', { name: 'Approve' }).click();
	await expect(page.getByText('Nothing awaiting moderation.')).toBeVisible();

	await page.goto(`/articles/${slug}`);
	await expect(page.getByText('Nice post!')).toBeVisible();
	await page.context().close();
});

test('analytics show the real tracker view events', async ({ browser }) => {
	expect(docId).not.toBe('');
	const page = await adminPage(browser);
	await page.goto(`/admin/stats/${docId}`);
	await expect(page.getByText('Views (estimated)')).toBeVisible();
	await expect(page.locator('.stat-value').first()).not.toHaveText('0');
	await page.context().close();
});

test('an experiment can be created, started, and concluded', async ({ browser }) => {
	expect(docId).not.toBe('');
	const page = await adminPage(browser);
	await page.goto(`/admin/stats/${docId}`);
	await expect(page.getByText('No experiments yet — create one below.')).toBeVisible();

	await page.locator('details.exp-card summary').click();
	await page.getByPlaceholder('e.g. New headline').fill('Headline test');
	await page.getByLabel('Variant 1 content').fill('A bolder headline');
	await page.getByRole('button', { name: 'Create experiment' }).click();
	await expect(page.getByText('Headline test')).toBeVisible();

	await page.getByRole('button', { name: 'Start experiment' }).click();
	await expect(page.getByText('running', { exact: true })).toBeVisible();

	await page.getByRole('button', { name: 'No improvement' }).click();
	await expect(page.getByText('Decision: no_improvement')).toBeVisible();
	await page.context().close();
});

test('the owner can log out and log back in', async ({ browser }) => {
	const page = await adminPage(browser);
	await gotoDashboard(page);

	await page.getByRole('button', { name: 'Log out' }).click();
	await page.waitForURL(/\/login$/);

	await page.getByLabel('Email', { exact: true }).fill(EMAIL);
	await page.getByLabel('Password', { exact: true }).fill(PASSWORD);
	await page.getByRole('button', { name: 'Log in' }).click();
	await expect(page).toHaveURL(/\/admin$/);
	await expect(page.getByText(TITLE)).toBeVisible();
	await page.context().close();
});
