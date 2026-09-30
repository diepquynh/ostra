---
name: e2e-test
description: Write a Playwright spec under e2e/ that drives the page in a browser.
---

# Browser tests

1. Put the spec in `e2e/<area>.spec.js`. Import `test` and `expect` from `@playwright/test`.
2. `playwright.config.js` starts the server and sets `baseURL`, so start each test with `await page.goto("/")`.
3. Find elements by role and label (`page.getByRole`, `page.getByLabel`), never by CSS class.
4. Assert what the user sees with `expect(locator).toHaveText(...)` or `toHaveCount(...)`.
5. Run with `npx playwright test e2e/<area>.spec.js` after `npm install` and `npx playwright install chromium`.
