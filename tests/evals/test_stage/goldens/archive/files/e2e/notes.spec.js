import { expect, test } from "@playwright/test";

test("adding a note shows it in the list", async ({ page }) => {
  await page.goto("/");
  await page.getByLabel("New note").fill("milk");
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page.getByRole("listitem")).toHaveText(["milk Archive"]);
});
