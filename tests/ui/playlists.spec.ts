import { test, expect } from "@playwright/test";

for (const creating of [false, true]) {
  test(`${creating ? "creation" : "playlist edit"} preserves exact intent after a lost acknowledgement and reload`, async ({
    page,
  }) => {
    await page.goto("/tests/ui/index.html");
    const label = creating
      ? "Создать тестовый плейлист"
      : "Редактировать тестовый плейлист";
    await page.getByRole("button", { name: label }).click();
    let dialog = page.getByRole("dialog");
    await dialog
      .getByRole("textbox", { name: "Название", exact: true })
      .fill("Сохранить этот плейлист");
    await dialog
      .getByRole("button", { name: "Сохранить", exact: true })
      .click();
    await expect(
      dialog.getByRole("button", { name: "Проверить сохранение" }),
    ).toBeEnabled();
    const original = await page.evaluate(
      () => (window as any).testRuntime.calls[0],
    );
    await page.reload();
    await page.getByRole("button", { name: label }).click();
    dialog = page.getByRole("dialog");
    await expect(
      dialog.getByRole("textbox", { name: "Название", exact: true }),
    ).toBeDisabled();
    await expect(
      dialog.getByRole("textbox", { name: "Название", exact: true }),
    ).toHaveValue("Сохранить этот плейлист");
    await dialog.getByRole("button", { name: "Проверить сохранение" }).click();
    await expect
      .poll(() => page.evaluate(() => (window as any).testRuntime.calls.length))
      .toBe(1);
    const retry = await page.evaluate(
      () => (window as any).testRuntime.calls[0],
    );
    expect(retry).toEqual(original);
    expect(retry.args.request.kind).toBe(creating ? "mutate" : "edit_playlist");
    if (!creating)
      expect(retry.args.request.payload.edit.expected_order).toBe("9");
  });
}
for (const width of [320, 390, 768, 1440]) {
  test(`playlist editor fits ${width}px and retains offline draft`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.goto("/tests/ui/index.html?offline&playlist-editor");
    const dialog = page.getByRole("dialog");
    await dialog
      .getByRole("textbox", { name: "Название", exact: true })
      .fill("Музыка в дороге");
    await expect(
      dialog.getByRole("button", { name: "Сохранить", exact: true }),
    ).toBeDisabled();
    expect(
      await dialog.evaluate((node) => node.scrollWidth <= node.clientWidth + 1),
    ).toBeTruthy();
    await page.screenshot({
      path: `test-results/playlist-editor-${width}.png`,
      animations: "disabled",
    });
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    await page
      .getByRole("button", { name: "Редактировать тестовый плейлист" })
      .click();
    await expect(
      dialog.getByRole("textbox", { name: "Название", exact: true }),
    ).toHaveValue("Музыка в дороге");
    expect(
      await page.evaluate(() => (window as any).testRuntime.calls.length),
    ).toBe(0);
  });
}
test("playlist preflight rejection releases draft for correction", async ({
  page,
}) => {
  await page.goto("/tests/ui/index.html?preflight&playlist-editor");
  const dialog = page.getByRole("dialog");
  await dialog
    .getByRole("textbox", { name: "Название", exact: true })
    .fill("Мой изменённый плейлист");
  await dialog.getByRole("button", { name: "Сохранить", exact: true }).click();
  await expect(dialog.getByRole("alert")).toContainText(
    "Изменение не применено",
  );
  await expect(
    dialog.getByRole("textbox", { name: "Название", exact: true }),
  ).toBeEnabled();
  await expect(
    dialog.getByRole("textbox", { name: "Название", exact: true }),
  ).toHaveValue("Мой изменённый плейлист");
});
