import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

export const ALGORITHM = "oklab-kmeans-v1";
const FRAME_WIDTH = 96;
const FRAME_HEIGHT = 96;

function sha256File(filePath) {
  const hash = crypto.createHash("sha256");
  hash.update(fs.readFileSync(filePath));
  return hash.digest("hex");
}

function safeSourcePath(root, relativeFile) {
  if (typeof relativeFile !== "string" || path.isAbsolute(relativeFile)) {
    throw new Error("idle palette source must be a relative path");
  }
  const resolved = path.resolve(root, relativeFile);
  const relative = path.relative(root, resolved);
  if (!relative || relative.startsWith("..") || path.isAbsolute(relative)) {
    throw new Error("idle palette source escapes VideoSource/V1");
  }
  return resolved;
}

export function canonicalIdleFile(config) {
  const clips = config?.states?.idle?.clips;
  if (!Array.isArray(clips)) {
    throw new Error("video-config.json is missing idle clips");
  }
  const selected = clips.find((clip) => clip?.enabled !== false && typeof clip?.file === "string");
  if (!selected) {
    throw new Error("video-config.json has no enabled idle clip");
  }
  return selected.file.replaceAll("\\", "/");
}

function srgbToLinear(value) {
  const channel = value / 255;
  return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
}

function linearToSrgb(value) {
  const channel = Math.max(0, Math.min(1, value));
  const encoded = channel <= 0.0031308 ? channel * 12.92 : 1.055 * channel ** (1 / 2.4) - 0.055;
  return Math.round(encoded * 255);
}

function rgbToOklab(red, green, blue) {
  const r = srgbToLinear(red);
  const g = srgbToLinear(green);
  const b = srgbToLinear(blue);
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  return [
    0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  ];
}

function oklabToRgb([L, a, b]) {
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  return [
    linearToSrgb(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
    linearToSrgb(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
    linearToSrgb(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s),
  ];
}

function distanceSquared(left, right) {
  const d0 = left[0] - right[0];
  const d1 = left[1] - right[1];
  const d2 = left[2] - right[2];
  return d0 * d0 + d1 * d1 + d2 * d2;
}

function randomGenerator(seed) {
  let value = seed >>> 0;
  return () => {
    value += 0x6d2b79f5;
    let next = value;
    next = Math.imul(next ^ (next >>> 15), next | 1);
    next ^= next + Math.imul(next ^ (next >>> 7), next | 61);
    return ((next ^ (next >>> 14)) >>> 0) / 4294967296;
  };
}

function chooseCentroids(points, count, seed) {
  const random = randomGenerator(seed);
  const centroids = [points[Math.floor(random() * points.length)].slice()];
  while (centroids.length < count) {
    const weights = points.map((point) =>
      Math.min(...centroids.map((centroid) => distanceSquared(point, centroid))),
    );
    const total = weights.reduce((sum, value) => sum + value, 0);
    if (total <= Number.EPSILON) break;
    let cursor = random() * total;
    let selected = points.length - 1;
    for (let index = 0; index < weights.length; index += 1) {
      cursor -= weights[index];
      if (cursor <= 0) {
        selected = index;
        break;
      }
    }
    centroids.push(points[selected].slice());
  }
  return centroids;
}

function clusterPixels(points, clusterCount, seed) {
  const centroids = chooseCentroids(points, clusterCount, seed);
  let assignments = new Uint8Array(points.length);
  for (let iteration = 0; iteration < 10; iteration += 1) {
    const sums = centroids.map(() => [0, 0, 0]);
    const counts = new Uint32Array(centroids.length);
    for (let index = 0; index < points.length; index += 1) {
      let selected = 0;
      let closest = Number.POSITIVE_INFINITY;
      for (let candidate = 0; candidate < centroids.length; candidate += 1) {
        const distance = distanceSquared(points[index], centroids[candidate]);
        if (distance < closest) {
          selected = candidate;
          closest = distance;
        }
      }
      assignments[index] = selected;
      counts[selected] += 1;
      sums[selected][0] += points[index][0];
      sums[selected][1] += points[index][1];
      sums[selected][2] += points[index][2];
    }
    let movement = 0;
    for (let index = 0; index < centroids.length; index += 1) {
      if (counts[index] === 0) continue;
      const next = sums[index].map((value) => value / counts[index]);
      movement += distanceSquared(centroids[index], next);
      centroids[index] = next;
    }
    if (movement < 0.000001) break;
  }
  const counts = new Uint32Array(centroids.length);
  assignments.forEach((assignment) => {
    counts[assignment] += 1;
  });
  return centroids.map((lab, index) => ({
    lab,
    rgb: oklabToRgb(lab),
    coverage: counts[index] / points.length,
  }));
}

function toHex(rgb) {
  return `#${rgb.map((value) => value.toString(16).padStart(2, "0")).join("")}`.toUpperCase();
}

export function extractThemeFromRgb(rgb, seed = 1) {
  if (!Buffer.isBuffer(rgb) || rgb.length === 0 || rgb.length % 3 !== 0) {
    throw new Error("RGB frame buffer is invalid");
  }
  const points = [];
  for (let index = 0; index < rgb.length; index += 3) {
    points.push(rgbToOklab(rgb[index], rgb[index + 1], rgb[index + 2]));
  }
  const clusters = clusterPixels(points, 7, seed).filter((cluster) => cluster.coverage >= 0.005);
  const darkCandidates = clusters.filter(
    (cluster) => cluster.lab[0] >= 0.02 && cluster.lab[0] <= 0.42 && cluster.coverage >= 0.01,
  );
  const basePool = darkCandidates.length > 0 ? darkCandidates : clusters;
  const base = basePool.reduce((best, cluster) => (cluster.lab[0] < best.lab[0] ? cluster : best));
  const accentPool = clusters.filter((cluster) => cluster !== base && cluster.lab[0] >= 0.18 && cluster.lab[0] <= 0.88);
  const accent = (accentPool.length > 0 ? accentPool : clusters).reduce((best, cluster) => {
    const chroma = Math.hypot(cluster.lab[1], cluster.lab[2]);
    const bestChroma = Math.hypot(best.lab[1], best.lab[2]);
    const score = chroma + Math.sqrt(distanceSquared(cluster.lab, base.lab)) * 0.04 + Math.min(cluster.coverage, 0.1) * 0.1;
    const bestScore = bestChroma + Math.sqrt(distanceSquared(best.lab, base.lab)) * 0.04 + Math.min(best.coverage, 0.1) * 0.1;
    return score > bestScore ? cluster : best;
  });
  const light = clusters.reduce((best, cluster) => (cluster.lab[0] > best.lab[0] ? cluster : best));
  const accentChroma = Math.hypot(accent.lab[1], accent.lab[2]);
  const targetChroma = accentChroma > 0 && accentChroma < 0.14
    ? Math.min(0.14, accentChroma * 1.45)
    : accentChroma;
  const accentScale = accentChroma > 0 ? targetChroma / accentChroma : 1;
  const accentRgb = oklabToRgb([
    accent.lab[0],
    accent.lab[1] * accentScale,
    accent.lab[2] * accentScale,
  ]);
  const baseRgb = Math.max(...base.rgb) < 8
    ? accentRgb.map((channel) => Math.round(channel * 0.08))
    : base.rgb;
  return {
    base: toHex(baseRgb),
    accent: toHex(accentRgb),
    light: toHex(light.rgb),
  };
}

function readExisting(outputPath) {
  try {
    return JSON.parse(fs.readFileSync(outputPath, "utf8"));
  } catch {
    return null;
  }
}

export function generatePalette(projectRoot, { force = false } = {}) {
  const videoRoot = path.join(projectRoot, "VideoSource", "V1");
  const configPath = path.join(videoRoot, "video-config.json");
  const outputPath = path.join(videoRoot, "ui-palette.json");
  const config = JSON.parse(fs.readFileSync(configPath, "utf8").replace(/^\uFEFF/, ""));
  const sourceFile = canonicalIdleFile(config);
  const sourcePath = safeSourcePath(videoRoot, sourceFile);
  if (!fs.statSync(sourcePath).isFile()) throw new Error(`idle palette source is missing: ${sourceFile}`);
  const sourceHash = sha256File(sourcePath);
  const current = readExisting(outputPath);
  if (
    !force
    && current?.schema_version === 1
    && current?.algorithm === ALGORITHM
    && current?.source?.file === sourceFile
    && current?.source?.sha256 === sourceHash
    && current?.source?.frame_index === 0
  ) {
    return { changed: false, outputPath, palette: current };
  }

  const ffmpeg = process.env.CODEX_PET_FFMPEG || "ffmpeg";
  const result = spawnSync(ffmpeg, [
    "-hide_banner", "-loglevel", "error", "-i", sourcePath,
    "-vf", `crop=trunc(iw*0.9/2)*2:trunc(ih*0.9/2)*2:(iw-ow)/2:(ih-oh)/2,scale=${FRAME_WIDTH}:${FRAME_HEIGHT}:flags=area,format=rgb24`,
    "-frames:v", "1", "-an", "-sn", "-dn", "-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1",
  ], { encoding: null, maxBuffer: 1024 * 1024 });
  if (result.error) throw new Error(`unable to start ffmpeg: ${result.error.message}`);
  if (result.status !== 0) throw new Error(`ffmpeg first-frame extraction failed with exit code ${result.status}`);
  const expectedBytes = FRAME_WIDTH * FRAME_HEIGHT * 3;
  if (!Buffer.isBuffer(result.stdout) || result.stdout.length !== expectedBytes) {
    throw new Error(`ffmpeg returned ${result.stdout?.length ?? 0} RGB bytes; expected ${expectedBytes}`);
  }
  const seed = Number.parseInt(sourceHash.slice(0, 8), 16);
  const palette = {
    schema_version: 1,
    algorithm: ALGORITHM,
    source: { file: sourceFile, sha256: sourceHash, frame_index: 0 },
    colors: extractThemeFromRgb(result.stdout, seed),
  };
  const serialized = `${JSON.stringify(palette, null, 2)}\n`;
  const tempPath = `${outputPath}.${process.pid}.tmp`;
  fs.writeFileSync(tempPath, serialized, "utf8");
  fs.renameSync(tempPath, outputPath);
  return { changed: true, outputPath, palette };
}

function main() {
  const scriptPath = fileURLToPath(import.meta.url);
  const projectRoot = path.resolve(path.dirname(scriptPath), "..");
  const result = generatePalette(projectRoot, { force: process.argv.includes("--force") });
  process.stdout.write(`${result.changed ? "generated" : "current"}: ${result.outputPath}\n`);
  process.stdout.write(`source=${result.palette.source.file} base=${result.palette.colors.base} accent=${result.palette.colors.accent} light=${result.palette.colors.light}\n`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  }
}
