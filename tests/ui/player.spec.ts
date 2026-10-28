import { test, expect } from "@playwright/test";

for (const width of [320, 390, 768, 1440]) {
  test(`library and full player fit ${width}px`, async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.setViewportSize({ width, height: 900 });
    await page.goto("/tests/ui/index.html?count=10001");
    await expect(page.locator(".track-row")).toHaveCount(50);
    await expect(
      page.getByRole("heading", { name: "Медиатека", exact: true }),
    ).toBeVisible();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBeTruthy();
    if (width <= 640) {
      await expect(page.locator(".sidebar")).toBeHidden();
      const player = await page.locator(".player").boundingBox();
      const nav = await page.locator(".mobile-nav").boundingBox();
      expect(player!.y + player!.height).toBeLessThanOrEqual(nav!.y + 1);
    }
    await page.screenshot({
      path: `test-results/library-${width}.png`,
      animations: "disabled",
    });
    await page
      .getByRole("button", { name: "Открыть текущую композицию" })
      .click();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();
    expect(
      await dialog.evaluate((node) => node.scrollWidth <= node.clientWidth + 1),
    ).toBeTruthy();
    await page.screenshot({
      path: `test-results/player-${width}.png`,
      animations: "disabled",
    });
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    expect(errors).toEqual([]);
  });
}

test("10,001 tracks are paged; a late track is searchable", async ({
  page,
}) => {
  await page.goto("/tests/ui/index.html?count=10001");
  await expect(page.locator(".track-row")).toHaveCount(50);
  await page
    .getByRole("textbox", { name: "Поиск в коллекции" })
    .fill("композиция 10001");
  await expect(page.locator(".track-row")).toHaveCount(1);
  await expect(page.locator(".track-row")).toContainText("10001");
});

test("offline edit retains text and explicit tag-copy choice across reopening", async ({
  page,
}) => {
  await page.goto("/tests/ui/index.html?offline");
  await page
    .getByRole("button", { name: "Редактировать тестовый трек" })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog
    .getByRole("textbox", { name: "Название", exact: true })
    .fill("Черновик без подключения");
  await dialog
    .getByRole("radio", { name: /Изменить встроенные теги копии/ })
    .check();
  await expect(
    dialog.getByRole("button", { name: "Сохранить", exact: true }),
  ).toBeDisabled();
  await page.keyboard.press("Escape");
  await page
    .getByRole("button", { name: "Редактировать тестовый трек" })
    .click();
  await expect(
    dialog.getByRole("textbox", { name: "Название", exact: true }),
  ).toHaveValue("Черновик без подключения");
  await expect(
    dialog.getByRole("radio", { name: /Изменить встроенные теги копии/ }),
  ).toBeChecked();
  expect(
    await page.evaluate(() => (window as any).testRuntime.calls.length),
  ).toBe(0);
});

test("resuming an ended occurrence never replaces the engine queue", async ({
  page,
}) => {
  await page.goto("/tests/ui/index.html");
  await page.evaluate(async () => {
    const { model, resumePlayback } = (window as any).testRuntime;
    try {
      await resumePlayback(
        model.status.context.session_id,
        { state: "ended" },
        { ...model.queue, position: 2 },
      );
    } catch {
      /* the test transport rejects mutations */
    }
  });
  expect(
    await page.evaluate(() =>
      (window as any).testRuntime.calls.map((c: any) => [
        c.command,
        c.args.action,
      ]),
    ),
  ).toEqual([["edit_queue", { kind: "jump", index: 2 }]]);
});

test("draft deletion wins over concurrent autosaves", async ({ page }) => {
  await page.goto("/tests/ui/index.html");
  const saved = await page.evaluate(async () => {
    const { writeDraft, readDraft } = (window as any).testRuntime;
    const pending = [
      writeDraft("concurrency", { savedAt: 1 }),
      writeDraft("concurrency", { savedAt: 2 }),
      writeDraft("concurrency"),
    ];
    await Promise.all(pending);
    return (await readDraft("concurrency")) ?? null;
  });
  expect(saved).toBeNull();
});

test("a lost acknowledgement keeps exact edit payload and operation ID on retry", async ({
  page,
}) => {
  await page.goto("/tests/ui/index.html");
  await page
    .getByRole("button", { name: "Редактировать тестовый трек" })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog
    .getByRole("textbox", { name: "Название", exact: true })
    .fill("Сохранённое намерение");
  await dialog.getByRole("button", { name: "Сохранить", exact: true }).click();
  await expect(
    dialog.getByRole("button", { name: "Проверить сохранение" }),
  ).toBeEnabled();
  await page.keyboard.press("Escape");
  await page
    .getByRole("button", { name: "Редактировать тестовый трек" })
    .click();
  await expect(
    dialog.getByRole("textbox", { name: "Название", exact: true }),
  ).toBeDisabled();
  await dialog.getByRole("button", { name: "Проверить сохранение" }).click();
  await expect
    .poll(() => page.evaluate(() => (window as any).testRuntime.calls.length))
    .toBe(2);
  const calls = await page.evaluate(() => (window as any).testRuntime.calls);
  expect(calls).toHaveLength(2);
  expect(calls[0]).toEqual(calls[1]);
});

test("definitive preflight rejection unlocks the draft without losing text", async ({
  page,
}) => {
  await page.goto("/tests/ui/index.html?preflight");
  await page
    .getByRole("button", { name: "Редактировать тестовый трек" })
    .click();
  const dialog = page.getByRole("dialog");
  const title = dialog.getByRole("textbox", { name: "Название", exact: true });
  await title.fill("Черновик после конфликта версий");
  await dialog.getByRole("button", { name: "Сохранить", exact: true }).click();
  await expect(dialog.getByRole("alert")).toContainText(
    "Изменение не отправлено",
  );
  await expect(title).toBeEnabled();
  await expect(title).toHaveValue("Черновик после конфликта версий");
  await page.keyboard.press("Escape");
  await page
    .getByRole("button", { name: "Редактировать тестовый трек" })
    .click();
  await expect(title).toBeEnabled();
  await expect(title).toHaveValue("Черновик после конфликта версий");
});
