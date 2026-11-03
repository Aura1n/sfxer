// Generates tools/icon-source.png: a 1024x1024 RGBA icon with an alpha channel.
// Pure Node (zlib only) so the build needs no image library.
import { deflateSync } from "node:zlib";
import { writeFileSync, mkdirSync } from "node:fs";

const S = 1024;
const SUPERSAMPLE = 3;

const TOP = [0x3b, 0x82, 0xf6];
const BOTTOM = [0x1d, 0x4e, 0xd9];

// rounded rect coverage: 1 = inside, 0 = outside, smooth via subpixel sampling
function roundedCoverage(x, y, rect) {
  const { x0, y0, x1, y1, r } = rect;
  let hit = 0;
  for (let sy = 0; sy < SUPERSAMPLE; sy++) {
    for (let sx = 0; sx < SUPERSAMPLE; sx++) {
      const px = x + (sx + 0.5) / SUPERSAMPLE;
      const py = y + (sy + 0.5) / SUPERSAMPLE;
      if (px < x0 || px > x1 || py < y0 || py > y1) continue;
      const cx = Math.min(Math.max(px, x0 + r), x1 - r);
      const cy = Math.min(Math.max(py, y0 + r), y1 - r);
      const dx = px - cx;
      const dy = py - cy;
      if (dx * dx + dy * dy > r * r) continue;
      hit++;
    }
  }
  return hit / (SUPERSAMPLE * SUPERSAMPLE);
}

const plate = { x0: 96, y0: 96, x1: 928, y1: 928, r: 190 };

// waveform bars: symmetric around the vertical centre
const bars = [
  { cx: 264, half: 96 },
  { cx: 368, half: 210 },
  { cx: 472, half: 330 },
  { cx: 576, half: 168 },
  { cx: 680, half: 264 },
  { cx: 784, half: 120 },
];
const barWidth = 56;
const midY = 512;

function barCoverage(x, y) {
  for (const b of bars) {
    const rect = {
      x0: b.cx - barWidth / 2,
      y0: midY - b.half,
      x1: b.cx + barWidth / 2,
      y1: midY + b.half,
      r: barWidth / 2,
    };
    const c = roundedCoverage(x, y, rect);
    if (c > 0) return c;
  }
  return 0;
}

const raw = new Uint8Array(S * 4 * S);
for (let y = 0; y < S; y++) {
  const rowOffset = y * S * 4;
  const t = y / (S - 1);
  const bg = [
    TOP[0] + (BOTTOM[0] - TOP[0]) * t,
    TOP[1] + (BOTTOM[1] - TOP[1]) * t,
    TOP[2] + (BOTTOM[2] - TOP[2]) * t,
  ];
  for (let x = 0; x < S; x++) {
    const plateA = roundedCoverage(x, y, plate);
    if (plateA === 0) continue;
    const barA = barCoverage(x, y);
    // bars are white, composited over the gradient plate
    const r = bg[0] + (255 - bg[0]) * barA;
    const g = bg[1] + (255 - bg[1]) * barA;
    const b = bg[2] + (255 - bg[2]) * barA;
    const i = rowOffset + x * 4;
    raw[i] = Math.round(r);
    raw[i + 1] = Math.round(g);
    raw[i + 2] = Math.round(b);
    raw[i + 3] = Math.round(255 * plateA);
  }
}

// PNG encoding
function crc32(buf) {
  let c, table = crc32.table;
  if (!table) {
    table = crc32.table = new Int32Array(256);
    for (let n = 0; n < 256; n++) {
      c = n;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      table[n] = c;
    }
  }
  c = ~0;
  for (let i = 0; i < buf.length; i++) c = table[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return ~c >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(S, 0);
ihdr.writeUInt32BE(S, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // RGBA
const filtered = Buffer.alloc(S * (S * 4 + 1));
for (let y = 0; y < S; y++) {
  filtered[y * (S * 4 + 1)] = 0;
  filtered.set(raw.subarray(y * S * 4, (y + 1) * S * 4), y * (S * 4 + 1) + 1);
}

const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(filtered, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);

mkdirSync(new URL(".", import.meta.url), { recursive: true });
writeFileSync(new URL("./icon-source.png", import.meta.url), png);
console.log("wrote tools/icon-source.png", png.length, "bytes");
