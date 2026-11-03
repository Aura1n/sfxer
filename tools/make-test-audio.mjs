// Writes a small throwaway library so the app can be exercised without touching
// real assets: PCM WAVs of assorted shapes, plus files that must not decode.
import { mkdirSync, writeFileSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const outDir = process.argv[2] || join(dirname(fileURLToPath(import.meta.url)), "..", "test-library");

function wav({ seconds, rate, channels, bits, tone, noise, gain = 0.8 }) {
  const frames = Math.round(seconds * rate);
  const bytesPer = bits / 8;
  const full = Math.pow(2, bits - 1) - 1;
  const data = Buffer.alloc(frames * channels * bytesPer);
  const write = {
    8: (o, v) => data.writeUInt8(v, o),
    16: (o, v) => data.writeInt16LE(v, o),
    24: (o, v) => {
      data[o] = v & 0xff;
      data[o + 1] = (v >> 8) & 0xff;
      data[o + 2] = (v >> 16) & 0xff;
    },
    32: (o, v) => data.writeInt32LE(v, o),
  }[bits];
  const clamp = {
    8: (v) => Math.max(0, Math.min(255, v)),
    16: (v) => Math.max(-32768, Math.min(32767, v)),
    24: (v) => Math.max(-full - 1, Math.min(full, v)),
    32: (v) => Math.max(-full - 1, Math.min(full, v)),
  }[bits];
  for (let i = 0; i < frames; i++) {
    for (let c = 0; c < channels; c++) {
      const t = i / rate;
      // Stereo tones are panned so the two channels visibly differ.
      const f = tone * (c === 1 && channels === 2 ? 1.5 : 1);
      const env = Math.min(1, (i + 1) / (rate * 0.005)) * Math.min(1, (frames - i) / (rate * 0.02));
      const s = (noise ? (Math.random() * 2 - 1) * 0.4 : Math.sin(2 * Math.PI * f * t) * gain) * env;
      // Saturating at full scale is what a real clipped recording looks like.
      const v = bits === 8 ? Math.round((s * 0.5 + 0.5) * 255) : Math.round(s * full);
      write(i * channels * bytesPer + c * bytesPer, clamp(v));
    }
  }
  const header = Buffer.alloc(44);
  header.write("RIFF", 0);
  header.writeUInt32LE(36 + data.length, 4);
  header.write("WAVE", 8);
  header.write("fmt ", 12);
  header.writeUInt32LE(16, 16);
  header.writeUInt16LE(1, 20);
  header.writeUInt16LE(channels, 22);
  header.writeUInt32LE(rate, 24);
  header.writeUInt32LE(rate * channels * bytesPer, 28);
  header.writeUInt16LE(channels * bytesPer, 32);
  header.writeUInt16LE(bits, 34);
  header.write("data", 36);
  header.writeUInt32LE(data.length, 40);
  return Buffer.concat([header, data]);
}

const files = [
  ["ui/click-soft.wav", wav({ seconds: 0.12, rate: 44100, channels: 1, bits: 16, tone: 1200 })],
  ["ui/click-hard.wav", wav({ seconds: 0.09, rate: 48000, channels: 1, bits: 24, tone: 2400 })],
  ["foley/footstep-gravel.wav", wav({ seconds: 0.6, rate: 44100, channels: 2, bits: 16, noise: true })],
  ["foley/door-close.wav", wav({ seconds: 1.4, rate: 44100, channels: 2, bits: 16, tone: 90 })],
  ["ambience/rain-loop.wav", wav({ seconds: 4, rate: 22050, channels: 2, bits: 16, noise: true })],
  ["tones/sine-440.wav", wav({ seconds: 2.5, rate: 48000, channels: 2, bits: 32, tone: 440 })],
  ["tones/silence.wav", wav({ seconds: 1, rate: 44100, channels: 1, bits: 16, tone: 100, gain: 0 })],
  ["tones/clipped-square.wav", wav({ seconds: 0.5, rate: 44100, channels: 1, bits: 16, tone: 220, gain: 1.6 })],
  ["bad/not-really-mp3.mp3", Buffer.from("this is not audio data at all".repeat(30))],
  ["bad/empty.wav", Buffer.alloc(0)],
  ["notes.txt", Buffer.from("not audio; the scanner must ignore this")],
];

rmSync(outDir, { recursive: true, force: true });
for (const [rel, buf] of files) {
  const p = join(outDir, rel);
  mkdirSync(dirname(p), { recursive: true });
  writeFileSync(p, buf);
}
console.log(`wrote ${files.length} files to ${outDir}`);
