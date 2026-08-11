import assert from "node:assert/strict";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { canonicalIdleFile, extractThemeFromRgb, generatePalette } from "./generate-ui-palette.mjs";

const pixels = Buffer.alloc(1000 * 3);
for (let index = 0; index < 1000; index += 1) {
  const color = index < 650 ? [9, 9, 17] : index < 900 ? [110, 72, 221] : [240, 247, 251];
  pixels.set(color, index * 3);
}

const first = extractThemeFromRgb(pixels, 42);
const second = extractThemeFromRgb(pixels, 42);
assert.deepEqual(first, second);
assert.equal(first.base, "#090911");
assert.equal(first.accent, "#6E48DD");
assert.equal(first.light, "#F0F7FB");
assert.equal(
  canonicalIdleFile({ states: { idle: { clips: [{ file: "idle.mp4", enabled: true }] } } }),
  "idle.mp4",
);
assert.throws(() => canonicalIdleFile({ states: { idle: { clips: [] } } }), /enabled idle clip/);
const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
assert.equal(generatePalette(projectRoot).changed, false);

process.stdout.write("ui palette tests passed\n");
