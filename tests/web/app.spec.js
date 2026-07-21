import { expect, test } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { deflateSync } from "node:zlib";

const APP_ORIGIN = "http://127.0.0.1:4173";

test("loads Wasm and converts a selected image", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator("#status")).toHaveText(/Choose an image to begin/);

  await uploadImage(page);

  await expect(page.locator("#status")).toHaveText("ASCII art ready.");
  await expect(page.locator("#copy-button")).toBeEnabled();
  await expect(page.locator("#download-button")).toBeEnabled();
  const output = await page.locator("#ascii-output").textContent();
  expect(output).toContain("|");
  expect(output).toContain(".");
  expect(output).toMatch(/\n$/);
});

test("accepts an image dropped on the source area", async ({ page }) => {
  await page.goto("/");
  const png = makePng(16, 8);
  const transfer = await page.evaluateHandle(
    ({ bytes }) => {
      const data = Uint8Array.from(atob(bytes), (character) => character.charCodeAt(0));
      const dataTransfer = new DataTransfer();
      dataTransfer.items.add(new File([data], "dropped.png", { type: "image/png" }));
      return dataTransfer;
    },
    { bytes: png.toString("base64") },
  );

  await page.locator("#drop-zone").dispatchEvent("drop", { dataTransfer: transfer });

  await expect(page.locator("#source-name")).toHaveText("dropped.png");
  await expect(page.locator("#status")).toHaveText("ASCII art ready.");
});

test("validates settings and renders only the latest width", async ({ page }) => {
  await page.goto("/");
  await uploadImage(page, { width: 256, height: 128 });
  await expect(page.locator("#status")).toHaveText("ASCII art ready.");

  await page.locator("#ramp-input").fill("@");
  await expect(page.locator("#settings-error")).toContainText("2–256");
  await expect(page.locator("#copy-button")).toBeDisabled();

  await page.locator("#ramp-input").fill(".|");
  await page.locator("#columns-number").fill("400");
  await page.waitForTimeout(180);
  await page.locator("#columns-number").fill("20");
  await expect(page.locator("#copy-button")).toBeDisabled();

  await expect(page.locator("#status")).toHaveText("ASCII art ready.");
  await expect
    .poll(async () => {
      const output = await page.locator("#ascii-output").textContent();
      return output?.split("\n", 1)[0].length;
    })
    .toBe(20);
  await expect(page.locator("#columns-range")).toHaveValue("20");
});

test("applies and resets tone and colour controls", async ({ page }) => {
  await page.goto("/");
  await uploadImage(page, { width: 128, height: 64 });
  await expect(page.locator("#copy-button")).toBeEnabled();
  const original = await page.locator("#ascii-output").textContent();

  await page.locator("#brightness-input").fill("1");
  await expect(page.locator("#brightness-value")).toHaveText("1.00");
  await expect(page.locator("#copy-button")).toBeDisabled();
  await expect(page.locator("#copy-button")).toBeEnabled();
  const brightened = await page.locator("#ascii-output").textContent();
  expect(brightened).not.toBe(original);
  expect(brightened).toMatch(/^(\.+\n)+$/);

  await page.locator("#reset-tone-button").click();
  await expect(page.locator("#brightness-value")).toHaveText("0.00");
  await expect(page.locator("#copy-button")).toBeDisabled();
  await expect(page.locator("#copy-button")).toBeEnabled();
  await expect(page.locator("#ascii-output")).toHaveText(original);
});

test("reports image decode failures without enabling output actions", async ({ page }) => {
  await page.goto("/");
  await page.locator("#file-input").setInputFiles({
    name: "broken.png",
    mimeType: "image/png",
    buffer: Buffer.from("not an image"),
  });

  await expect(page.locator("#status")).toContainText("Could not decode that image");
  await expect(page.locator("#copy-button")).toBeDisabled();
  await expect(page.locator("#download-button")).toBeDisabled();
});

test("downloads UTF-8 plain text with a source-derived filename", async ({ page }) => {
  await page.addInitScript(() => {
    const createObjectUrl = URL.createObjectURL.bind(URL);
    URL.createObjectURL = (blob) => {
      window.__lastBlobType = blob.type;
      return createObjectUrl(blob);
    };
  });
  await page.goto("/");
  await uploadImage(page, { name: "portrait.test.png" });
  await expect(page.locator("#status")).toHaveText("ASCII art ready.");

  const downloadPromise = page.waitForEvent("download");
  await page.locator("#download-button").click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe("portrait.test_ascii.txt");
  expect(await page.evaluate(() => window.__lastBlobType)).toBe("text/plain;charset=utf-8");
  const downloadPath = await download.path();
  expect(downloadPath).not.toBeNull();
  const contents = await readFile(downloadPath, "utf8");
  expect(contents).toMatch(/\n$/);
  expect(contents).not.toContain("\r");
});

test("copies the exact generated text in a secure context", async ({ browserName, context, page }) => {
  test.skip(browserName !== "chromium", "Clipboard permissions are exercised once in Chromium.");
  await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: APP_ORIGIN });
  await page.goto("/");
  await uploadImage(page);
  await expect(page.locator("#status")).toHaveText("ASCII art ready.");

  const expected = await page.locator("#ascii-output").textContent();
  await page.locator("#copy-button").click();

  await expect(page.locator("#status")).toHaveText("Copied ASCII art to the clipboard.");
  const clipboard = await page.evaluate(() => navigator.clipboard.readText());
  expect(clipboard.replace(/\r\n/g, "\n")).toBe(expected);
});

test("uses a single-column layout and fitted preview on a narrow screen", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/");
  await uploadImage(page);
  await expect(page.locator("#status")).toHaveText("ASCII art ready.");

  const sourceBox = await page.locator(".source-panel").boundingBox();
  const outputBox = await page.locator(".output-panel").boundingBox();
  expect(sourceBox).not.toBeNull();
  expect(outputBox).not.toBeNull();
  expect(outputBox.y).toBeGreaterThan(sourceBox.y + sourceBox.height - 2);
  expect(outputBox.width).toBeLessThanOrEqual(390);
  const fontSize = await page
    .locator("#ascii-output")
    .evaluate((element) => Number.parseFloat(getComputedStyle(element).fontSize));
  expect(fontSize).toBeGreaterThanOrEqual(3);
  expect(fontSize).toBeLessThanOrEqual(12);
});

async function uploadImage(page, options = {}) {
  const { name = "gradient.png", width = 16, height = 8 } = options;
  await page.locator("#file-input").setInputFiles({
    name,
    mimeType: "image/png",
    buffer: makePng(width, height),
  });
}

function makePng(width, height) {
  const bytesPerRow = width * 4 + 1;
  const raw = Buffer.alloc(bytesPerRow * height);
  for (let y = 0; y < height; y += 1) {
    const rowStart = y * bytesPerRow;
    raw[rowStart] = 0;
    for (let x = 0; x < width; x += 1) {
      const pixelStart = rowStart + 1 + x * 4;
      const value = Math.round((x / Math.max(1, width - 1)) * 255);
      raw[pixelStart] = value;
      raw[pixelStart + 1] = value;
      raw[pixelStart + 2] = value;
      raw[pixelStart + 3] = 255;
    }
  }

  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8;
  header[9] = 6;
  return Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    pngChunk("IHDR", header),
    pngChunk("IDAT", deflateSync(raw)),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}

function pngChunk(type, data) {
  const typeBytes = Buffer.from(type, "ascii");
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length, 0);
  const checksum = Buffer.alloc(4);
  checksum.writeUInt32BE(crc32(Buffer.concat([typeBytes, data])), 0);
  return Buffer.concat([length, typeBytes, data, checksum]);
}

function crc32(data) {
  let crc = 0xffffffff;
  for (const byte of data) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}
