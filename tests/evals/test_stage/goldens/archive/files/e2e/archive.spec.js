import { expect, test } from "@playwright/test";

test("archiving a note takes it off the list", async ({ page }) => {
  await page.goto("/");
  await page.getByLabel("New note").fill("milk");
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page.getByRole("listitem")).toHaveCount(1);
  await page.getByRole("button", { name: "Archive" }).click();
  await expect(page.getByRole("listitem")).toHaveCount(0);
});

test("showing archived notes lists the archived note", async ({ page }) => {
  await page.goto("/");
  await page.getByLabel("New note").fill("milk");
  await page.getByRole("button", { name: "Add" }).click();
  await page.getByRole("button", { name: "Archive" }).click();
  await page.getByLabel("Show archived notes").check();
  await expect(page.getByRole("listitem")).toHaveText(["milk"]);
});
