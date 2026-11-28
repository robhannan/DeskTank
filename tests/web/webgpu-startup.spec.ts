import { expect, test } from "@playwright/test";
import { boxOf, decodePng, framesDrawn, open, watchErrors } from "./support.ts";

declare global {
  interface Window {
    gridPipelines: string[];
  }
}

// Use Chromium's full browser and enable SwiftShader WebGPU on Linux CI.
test.use({
  channel: "chromium",
  launchOptions: {
    args: [
      "--enable-unsafe-swiftshader",
      "--enable-unsafe-webgpu",
      ...(process.platform === "linux" ? ["--use-webgpu-adapter=swiftshader"] : []),
    ],
  },
});

test("WebGPU starts with the split grid and presents the tank", async ({ page }) => {
  const errors = watchErrors(page);
  await page.addInitScript(() => {
    window.gridPipelines = [];
    // Record the entry points while letting the browser compile the pipelines.
    type Descriptor = { fragment?: { entryPoint?: string } };
    const device = Reflect.get(globalThis, "GPUDevice") as {
      prototype: { createRenderPipeline(descriptor: Descriptor): unknown };
    } | undefined;
    if (!device) return;
    const createPipeline = device.prototype.createRenderPipeline;
    device.prototype.createRenderPipeline = function (descriptor) {
      const entry = descriptor.fragment?.entryPoint;
      if (entry === "fs_main" || entry === "fs_ground") window.gridPipelines.push(entry);
      return createPipeline.call(this, descriptor);
    };
  });
  await open(page, "?backend=webgpu");
  expect(await page.evaluate(() => window.desktank.backend)).toBe("webgpu");
  expect(await page.evaluate(() => window.gridPipelines)).toEqual(["fs_ground"]);
  await expect.poll(() => framesDrawn(page)).toBeGreaterThan(0);
  // framesDrawn counts submissions. Check that the canvas presents water.
  const box = await boxOf(page.locator("#tank"));
  const clip = { x: Math.floor(box.x + box.width / 2), y: Math.floor(box.y + box.height / 2), width: 1, height: 1 };
  await expect.poll(async () => {
    const { rgba } = await decodePng(page, await page.screenshot({ clip }));
    return rgba[2] - rgba[0];
  }, { message: "the middle of the tank shows blue water", timeout: 15_000 }).toBeGreaterThan(15);
  expect(errors).toEqual([]);
});
