import { expect, test } from "../lib/guard";

test("the app loads signed in", async ({ page, state }) => {
  await page.goto(`${state.app}/w/${state.ws}`);
  await expect(page.locator("body")).toContainText("PW-", { timeout: 20000 });
});
