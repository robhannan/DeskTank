// Exercise the same project subpath that GitHub Pages serves.
import { expect, test } from "@playwright/test";
import { framesDrawn, open, watchErrors } from "./support.ts";

test("installs under /DeskTank/ and runs a tank after an offline reload", async ({ page, context }) => {
  const errors = watchErrors(page);
  // Cache Storage is shared by all Pages projects on this origin.
  await page.goto("manifest.webmanifest");
  const keys = await page.evaluate(async () => {
    const key = (scope: string) => `desktank:${encodeURIComponent(new URL(scope, location.href).href)}:old`;
    const old = key("./");
    const sibling = key("../another-project/");
    await Promise.all([caches.open(old), caches.open(sibling)]);
    return { old, sibling };
  });
  await open(page, "?backend=webgl2");
  const manifest = await page.locator('link[rel="manifest"]').evaluate(async (link: HTMLLinkElement) => {
    const response = await fetch(link.href);
    return response.json();
  });
  expect(manifest).toMatchObject({ name: "DeskTank", start_url: "./", scope: "./", display: "standalone" });
  for (const size of [192, 512]) {
    const icon = manifest.icons.find((entry: { sizes: string }) => entry.sizes === `${size}x${size}`);
    expect(icon.type).toBe("image/png");
    const dimensions = await page.evaluate(async (src: string) => {
      const image = await createImageBitmap(await (await fetch(src)).blob());
      return [image.width, image.height];
    }, icon.src);
    expect(dimensions).toEqual([size, size]);
  }
  const scope = await page.evaluate(async () => {
    const worker = await navigator.serviceWorker.ready;
    return new URL(worker.scope).pathname;
  });
  expect(scope).toBe("/DeskTank/");
  await expect.poll(() => page.evaluate(() => navigator.serviceWorker.controller?.state)).toBe("activated");

  const cachesAfterActivation = await page.evaluate(() => caches.keys());
  expect(cachesAfterActivation).not.toContain(keys.old);
  expect(cachesAfterActivation).toContain(keys.sibling);

  await context.setOffline(true);
  const response = await page.reload();
  expect(response?.fromServiceWorker()).toBe(true);
  await expect(page.locator("body[data-ready]")).toBeAttached();
  const ticks = await page.evaluate(() => window.desktank.tank.tickCount());
  await expect.poll(() => page.evaluate(() => window.desktank.tank.tickCount())).toBeGreaterThan(ticks);
  expect(await framesDrawn(page)).toBeGreaterThan(0);
  expect(errors).toEqual([]);
});
