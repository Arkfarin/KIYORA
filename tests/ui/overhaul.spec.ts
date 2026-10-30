import { test, expect, type Locator, type Page } from "@playwright/test";

async function reachable(locator: Locator, page: Page) {
  await expect(locator).toBeVisible();
  const box = await locator.boundingBox();
  const size = page.viewportSize()!;
  expect(box!.x).toBeGreaterThanOrEqual(0);
  expect(box!.y).toBeGreaterThanOrEqual(0);
  expect(box!.x + box!.width).toBeLessThanOrEqual(size.width + 1);
  expect(box!.y + box!.height).toBeLessThanOrEqual(size.height + 1);
  expect(
    await locator.evaluate((el) => {
      const rect = el.getBoundingClientRect();
      return el.contains(
        document.elementFromPoint(
          rect.x + rect.width / 2,
          rect.y + rect.height / 2,
        ),
      );
    }),
  ).toBe(true);
}
async function openPlayer(page: Page) {
  await page
    .getByRole("button", { name: "Открыть текущую композицию" })
    .click();
  return page.getByRole("dialog").last();
}
for (const width of [320, 360, 390, 430, 768, 1024, 1440, 1920]) {
  test(`player actions and lyrics stay reachable at ${width}px`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: width === 320 ? 568 : 800 });
    await page.goto("/tests/ui/index.html?interactive");
    const dialog = await openPlayer(page);
    for (const name of [
      "Слушать",
      "Случайный порядок",
      "Повтор: выключен",
      "Предыдущая композиция",
      "Следующая композиция",
      "Текст",
      "Очередь",
      "Свернуть плеер",
    ])
      await reachable(dialog.getByRole("button", { name, exact: true }), page);
    expect(
      await dialog.evaluate((el) => el.scrollHeight <= el.clientHeight + 1),
    ).toBe(true);
    await expect(page.getByRole("slider", { name: "Громкость" })).toHaveCount(
      0,
    );
    await dialog.getByRole("button", { name: "Текст", exact: true }).click();
    await expect(dialog.locator(".current-line")).toContainText(
      "Строка песни 9",
    );
    await reachable(
      dialog.getByRole("button", { name: "Обложка", exact: true }),
      page,
    );
    await reachable(
      dialog.getByRole("button", { name: "Слушать", exact: true }),
      page,
    );
    expect(
      await dialog.evaluate((el) => el.scrollHeight <= el.clientHeight + 1),
    ).toBe(true);
    await page.keyboard.press("Escape");
    await expect(dialog.locator(".now-artwork")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
  });
}

test("shuffle and all repeat states wait for confirmation; a rejection keeps the old state", async ({
  page,
}) => {
  await page.goto("/tests/ui/index.html?interactive");
  const dialog = await openPlayer(page);
  await page.evaluate(() => (window as any).testRuntime.delay(500));
  await dialog
    .getByRole("button", { name: "Случайный порядок", exact: true })
    .click();
  await expect(
    dialog.getByRole("button", { name: "Случайный порядок", exact: true }),
  ).toHaveAttribute("aria-pressed", "false");
  await expect(
    dialog.getByRole("button", { name: "Выключить случайный порядок" }),
  ).toHaveAttribute("aria-pressed", "true");
  for (const [before, after] of [
    ["выключен", "очереди"],
    ["очереди", "композиции"],
    ["композиции", "выключен"],
  ]) {
    await dialog
      .getByRole("button", { name: `Повтор: ${before}`, exact: true })
      .click();
    await expect(
      dialog.getByRole("button", { name: `Повтор: ${after}`, exact: true }),
    ).toBeEnabled();
  }
  await page.evaluate(() => (window as any).testRuntime.fail("edit_queue"));
  await dialog
    .getByRole("button", { name: "Выключить случайный порядок" })
    .click();
  await expect(
    dialog.getByRole("button", { name: "Выключить случайный порядок" }),
  ).toBeEnabled();
  expect(
    await page.evaluate(() =>
      (window as any).testRuntime.calls.some(
        (c: any) => c.command === "set_queue",
      ),
    ),
  ).toBe(false);
});

test("seek preview survives older backend ticks and commits one keyboard seek", async ({
  page,
}) => {
  await page.goto("/tests/ui/index.html?interactive");
  const dialog = await openPlayer(page);
  const slider = dialog.getByRole("slider");
  await page.evaluate(() => (window as any).testRuntime.delay(500));
  await slider.focus();
  await page.keyboard.press("End");
  await page.evaluate(() => (window as any).testRuntime.tick(41000));
  await expect(slider).toHaveValue("210000");
  await expect
    .poll(() =>
      page.evaluate(
        () => (window as any).testRuntime.model.playback.position_ms,
      ),
    )
    .toBe(210000);
  const calls = await page.evaluate(() =>
    (window as any).testRuntime.calls.filter(
      (c: any) => c.command === "control_playback",
    ),
  );
  expect(calls).toHaveLength(1);
  expect(calls[0].args.action).toEqual({ kind: "seek", value: 210000 });
});

test("manual lyrics scroll suspends following; return and line seeking work", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/tests/ui/index.html?interactive");
  const dialog = await openPlayer(page);
  await dialog.getByRole("button", { name: "Текст", exact: true }).click();
  const lines = dialog.locator(".lyrics-lines");
  await expect(lines.locator(".current-line")).toContainText("Строка песни 9");
  await lines.hover();
  await page.mouse.wheel(0, 220);
  await expect(
    dialog.getByRole("button", { name: "К текущей строке" }),
  ).toBeVisible();
  await page.waitForTimeout(400);
  const top = await lines.evaluate((el) => el.scrollTop);
  await page.evaluate(() => (window as any).testRuntime.tick(60000));
  await expect(lines.locator(".current-line")).toContainText("Строка песни 13");
  expect(await lines.evaluate((el) => el.scrollTop)).toBeCloseTo(top, 0);
  await dialog.getByRole("button", { name: "К текущей строке" }).click();
  await expect(
    dialog.getByRole("button", { name: "К текущей строке" }),
  ).toHaveCount(0);
  await lines
    .getByRole("button", { name: "Строка песни 14", exact: true })
    .click();
  await expect
    .poll(() =>
      page.evaluate(
        () => (window as any).testRuntime.model.playback.position_ms,
      ),
    )
    .toBe(65000);
  await page.screenshot({
    path: "test-results/lyrics-mobile.png",
    animations: "disabled",
  });
});

test("plain lyrics never gain timing or an active line", async ({ page }) => {
  await page.goto("/tests/ui/index.html?interactive&plain");
  const dialog = await openPlayer(page);
  await dialog.getByRole("button", { name: "Текст", exact: true }).click();
  await expect(dialog.locator(".lyrics-lines p")).toHaveCount(40);
  await page.evaluate(() => (window as any).testRuntime.tick(65000));
  await expect(dialog.locator(".current-line")).toHaveCount(0);
  await expect(dialog.locator(".lyrics-lines button")).toHaveCount(0);
});

test("queue edits are confirmed and a stale/rejected edit does not remove a row", async ({
  page,
}) => {
  await page.setViewportSize({ width: 360, height: 800 });
  await page.goto("/tests/ui/index.html?interactive");
  const dialog = await openPlayer(page);
  await dialog.getByRole("button", { name: "Очередь", exact: true }).click();
  const rows = dialog.locator(".queue-list li");
  await expect(rows).toHaveCount(3);
  await rows.nth(1).getByRole("button", { name: "Ниже", exact: true }).click();
  await expect(rows.nth(2)).toContainText("Тестовая композиция 2");
  await page.evaluate(() => (window as any).testRuntime.fail("edit_queue"));
  await rows.nth(1).getByRole("button", { name: "Убрать из очереди" }).click();
  await expect(rows).toHaveCount(3);
  await expect(dialog.getByRole("alert")).toContainText(
    "Изменение не подтверждено",
  );
  await expect(
    rows.nth(1).getByRole("button", { name: "Убрать из очереди" }),
  ).toBeEnabled();
  await rows.nth(1).getByRole("button", { name: "Убрать из очереди" }).click();
  await expect(rows).toHaveCount(2);
});

test("resuming the current track from the library preserves its queue", async ({
  page,
}) => {
  await page.goto("/tests/ui/index.html?interactive");
  await page
    .getByRole("complementary", { name: "Главная навигация" })
    .getByRole("button", { name: "Медиатека", exact: true })
    .click();
  await page.locator(".track-row").first().locator(".cover-play").click();
  const calls = await page.evaluate(() => (window as any).testRuntime.calls);
  expect(calls.filter((c: any) => c.command === "set_queue")).toHaveLength(0);
  expect(
    calls.some(
      (c: any) =>
        c.command === "control_playback" && c.args.action.kind === "resume",
    ),
  ).toBe(true);
});

test("large type and reduced motion keep the small-screen player usable", async ({
  page,
}) => {
  await page.setViewportSize({ width: 320, height: 568 });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/tests/ui/index.html?interactive");
  await page.addStyleTag({ content: ":root { font-size: 21px !important; }" });
  const dialog = await openPlayer(page);
  for (const name of [
    "Слушать",
    "Случайный порядок",
    "Повтор: выключен",
    "Очередь",
    "Текст",
  ]) {
    const button = dialog.getByRole("button", { name, exact: true });
    await button.scrollIntoViewIfNeeded();
    await reachable(button, page);
  }
  expect(
    await dialog.evaluate((el) => el.scrollWidth <= el.clientWidth + 1),
  ).toBe(true);
  expect(
    await dialog
      .locator(".now-artwork")
      .evaluate((el) => parseFloat(getComputedStyle(el).animationDuration)),
  ).toBeLessThan(0.01);
});

test("touch hold opens actions without playing, movement cancels hold, artwork swipe closes", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/tests/ui/index.html?interactive");
  await page
    .getByRole("navigation", { name: "Навигация телефона" })
    .getByRole("button", { name: "Медиатека", exact: true })
    .click();
  const row = page.locator(".track-row").first();
  await row.dispatchEvent("pointerdown", {
    pointerType: "touch",
    clientX: 60,
    clientY: 200,
  });
  await row.dispatchEvent("pointermove", {
    pointerType: "touch",
    clientX: 60,
    clientY: 240,
  });
  await page.waitForTimeout(600);
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await row.dispatchEvent("pointerdown", {
    pointerType: "touch",
    clientX: 60,
    clientY: 200,
  });
  await expect(page.getByRole("dialog")).toBeVisible();
  await row.dispatchEvent("pointerup", {
    pointerType: "touch",
    clientX: 60,
    clientY: 200,
  });
  await row.locator(".cover-play").dispatchEvent("click");
  expect(
    await page.evaluate(() =>
      (window as any).testRuntime.calls.some(
        (c: any) =>
          c.command === "control_playback" || c.command === "set_queue",
      ),
    ),
  ).toBe(false);
  await page.keyboard.press("Escape");
  const dialog = await openPlayer(page);
  await dialog.locator(".now-artwork").dispatchEvent("pointerdown", {
    pointerType: "touch",
    clientX: 100,
    clientY: 150,
  });
  await dialog.locator(".now-artwork").dispatchEvent("pointerup", {
    pointerType: "touch",
    clientX: 100,
    clientY: 260,
  });
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("three mobile destinations, persistent search, Back layers and theme", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/tests/ui/index.html?interactive");
  const nav = page.getByRole("navigation", { name: "Навигация телефона" });
  await expect(nav.getByRole("button")).toHaveCount(3);
  await nav.getByRole("button", { name: "Поиск", exact: true }).click();
  const search = page.getByRole("searchbox", { name: "Поиск по медиатеке" });
  await search.fill("композиция 2");
  await expect(page.locator("main .track-row")).toHaveCount(1);
  await nav.getByRole("button", { name: "Медиатека", exact: true }).click();
  await page.evaluate(() =>
    window.dispatchEvent(new Event("kiyora:back", { cancelable: true })),
  );
  await expect(search).toHaveValue("композиция 2");
  const dialog = await openPlayer(page);
  await dialog.getByRole("button", { name: "Действия с композицией" }).click();
  await expect(page.getByRole("dialog")).toHaveCount(2);
  await page.evaluate(() =>
    window.dispatchEvent(new Event("kiyora:back", { cancelable: true })),
  );
  await expect(page.getByRole("dialog")).toHaveCount(1);
  await page.evaluate(() =>
    window.dispatchEvent(new Event("kiyora:back", { cancelable: true })),
  );
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page.getByRole("button", { name: "Настройки и профиль" }).click();
  await page.getByRole("button", { name: "Светлая", exact: true }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await expect(page.getByRole("slider", { name: "Громкость" })).toHaveCount(0);
  await expect(page.getByRole("combobox", { name: "Повтор" })).toHaveCount(0);
});

test("an empty mobile library has no disabled mini player", async ({
  page,
}) => {
  await page.setViewportSize({ width: 320, height: 568 });
  await page.goto("/tests/ui/index.html?interactive&count=0");
  await expect(page.locator(".player")).toBeHidden();
  await expect(
    page.getByRole("navigation", { name: "Навигация телефона" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Добавить музыку", exact: true }).last(),
  ).toBeVisible();
});

test("desktop shortcuts work in the player and leave text editing alone", async ({
  page,
}) => {
  await page.goto("/tests/ui/index.html?interactive");
  const dialog = await openPlayer(page);
  await page.keyboard.press("Control+ArrowRight");
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).testRuntime.calls.filter(
            (c: any) => c.command === "control_playback",
          ).length,
      ),
    )
    .toBe(1);
  await page.keyboard.press("Escape");
  await page.locator("body").click({ position: { x: 700, y: 600 } });
  await page.keyboard.press("Control+k");
  const input = page.getByRole("searchbox", { name: "Поиск по медиатеке" });
  await expect(input).toBeFocused();
  await input.fill("Песня");
  await page.keyboard.press("Space");
  await page.keyboard.press("Control+ArrowRight");
  await expect(input).toHaveValue("Песня ");
  expect(
    await page.evaluate(
      () =>
        (window as any).testRuntime.calls.filter(
          (c: any) => c.command === "control_playback",
        ).length,
    ),
  ).toBe(1);
  await expect(dialog).toHaveCount(0);
});

test("real backend hook isolates progress ticks and rejects a foreign session", async ({
  page,
}) => {
  await page.goto("/tests/ui/index.html?backend&count=10001");
  await expect(page.locator(".track-row")).toHaveCount(50);
  const renders = await page.evaluate(() =>
    (window as any).testRuntime.backendRenders(),
  );
  for (const position of [50000, 51000, 52000]) {
    await page.evaluate(
      (value) => (window as any).testRuntime.backendEvent(value),
      position,
    );
    await expect(
      page.getByRole("slider", { name: "Позиция воспроизведения" }),
    ).toHaveValue(String(position));
  }
  expect(
    await page.evaluate(() => (window as any).testRuntime.backendRenders()),
  ).toBe(renders);
  await page.evaluate(() =>
    (window as any).testRuntime.backendEvent(99000, true),
  );
  await expect(
    page.getByRole("slider", { name: "Позиция воспроизведения" }),
  ).toHaveValue("52000");
});

test("import uses native file paths, retains preview choices, and navigation stays available", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/tests/ui/index.html?interactive");
  await page
    .getByRole("button", { name: "Добавить музыку", exact: true })
    .last()
    .click();
  await page
    .getByRole("button", { name: "Выбрать файлы", exact: true })
    .click();
  await expect(page.getByText("music.flac", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: /Отправить как есть/ }),
  ).toBeVisible();
  const calls = await page.evaluate(() => (window as any).testRuntime.calls);
  const preview = calls.find(
    (c: any) => c.args?.request?.kind === "preview_import",
  );
  expect(preview.args.request.payload.path).toBe("/test/music.flac");
  expect(preview.args.request.payload.bytes).toBeUndefined();
  await page
    .getByRole("navigation", { name: "Навигация телефона" })
    .getByRole("button", { name: "Главная", exact: true })
    .click();
  await expect(page.locator(".player")).toBeVisible();
});

test("missing lyrics and offline controls explain the available action", async ({
  page,
}) => {
  await page.goto("/tests/ui/index.html?offline");
  const dialog = await openPlayer(page);
  await dialog.getByRole("button", { name: "Текст", exact: true }).click();
  await expect(
    dialog.getByRole("heading", { name: "Текста пока нет" }),
  ).toBeVisible();
  await expect(
    dialog.getByRole("button", { name: "Добавить текст" }),
  ).toBeEnabled();
  await expect(
    dialog.getByRole("button", { name: "В любимые" }),
  ).toBeDisabled();
  await expect(
    dialog.getByRole("button", { name: "Слушать", exact: true }),
  ).toBeEnabled();
});
