import { expect, test } from "@playwright/test";
import { boxOf, decodePng, framesDrawn, open, watchErrors } from "./support.ts";

declare global {
  interface Window {
    gridPipelines: string[];
  }
}

const softwareGpu = process.platform === "linux";
const renderTimeout = softwareGpu ? 30_000 : 15_000;

// Use SwiftShader for ANGLE and Vulkan on Linux so Chromium can back the
// WebGPU canvas swapchain on GPU-less runners (Chromium issue 575926500).
test.use({
  channel: "chromium",
  launchOptions: {
    args: [
      "--enable-unsafe-swiftshader",
      "--enable-unsafe-webgpu",
      ...(softwareGpu ? ["--use-angle=swiftshader", "--use-vulkan=swiftshader", "--enable-features=Vulkan"] : []),
    ],
  },
});

test("WebGPU starts with the split grid and presents the tank", async ({ page }, testInfo) => {
  const errors = watchErrors(page);
  // Chromium reports swapchain failures as device-loss warnings.
  page.on("console", (message) => {
    if (message.type() === "warning" && message.text().includes("desktank: GPU device lost")) {
      errors.push(message.text());
    }
  });
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
  try {
    await open(page, "?backend=webgpu");
    expect(await page.evaluate(() => window.desktank.backend)).toBe("webgpu");
    await expect.poll(() => framesDrawn(page), { timeout: renderTimeout }).toBeGreaterThan(0);
    expect(await page.evaluate(() => window.gridPipelines)).toEqual(["fs_ground"]);
    // framesDrawn counts submissions. SwiftShader still has to compile and
    // execute that work before Chromium can present the blue water.
    const box = await boxOf(page.locator("#tank"));
    const clip = { x: Math.floor(box.x + box.width / 2), y: Math.floor(box.y + box.height / 2), width: 1, height: 1 };
    await expect(async () => {
      const { rgba } = await decodePng(page, await page.screenshot({ clip }));
      expect(rgba[2] - rgba[0], `tank centre should show blue water (RGBA ${rgba.join(", ")})`).toBeGreaterThan(15);
    }).toPass({ timeout: renderTimeout });
    expect(errors).toEqual([]);
  } catch (error) {
    if (errors.length > 0) {
      await testInfo.attach("browser errors", { body: errors.join("\n"), contentType: "text/plain" });
    }
    throw error;
  }
});
