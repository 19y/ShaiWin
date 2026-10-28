// Renders assets/icon-preview.png: every icon size on a light and a dark band,
// small sizes blown up so the tray rendering can be judged at a glance.
//
// Run with: node scripts/preview-icon.mjs
import { deflateSync } from "node:zlib";
import { readFileSync, writeFileSync } from "node:fs";

const ico = readFileSync("assets/shaiwin.ico");
const count = ico.readUInt16LE(4);

function decode(base) {
  const w = ico[base] === 0 ? 256 : ico[base];
  const len = ico.readUInt32LE(base + 8);
  const off = ico.readUInt32LE(base + 12);
  const dib = ico.subarray(off, off + len);
  const header = dib.readUInt32LE(0);
  const h = dib.readInt32LE(8) / 2;
  if (dib.readUInt16LE(14) !== 32) throw new Error("expected a 32 bpp entry");
  const px = Buffer.alloc(w * h * 4);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const src = header + ((h - 1 - y) * w + x) * 4;
      const dst = (y * w + x) * 4;
      px[dst] = dib[src + 2]; // R
      px[dst + 1] = dib[src + 1]; // G
      px[dst + 2] = dib[src]; // B
      px[dst + 3] = dib[src + 3];
    }
  }
  return { size: w, px };
}

const entries = [];
for (let i = 0; i < count; i++) entries.push(decode(6 + i * 16));

// Big enough to see, small enough to read: the tray sizes get the most help.
const SHOW = [
  [16, 8],
  [20, 8],
  [24, 8],
  [32, 4],
  [48, 4],
  [64, 4],
  [128, 2],
  [256, 1],
];

const pad = 20;
const gap = 24;
const cellH = Math.max(...SHOW.map(([size, scale]) => size * scale));
const bandW = pad * 2 + SHOW.reduce((sum, [size, scale]) => sum + size * scale, 0) + gap * (SHOW.length - 1);
const bandH = cellH + pad * 2;
const W = bandW;
const H = bandH * 2;

const out = Buffer.alloc(W * H * 4);
for (let y = 0; y < H; y++) {
  const light = y < bandH;
  for (let x = 0; x < W; x++) {
    const i = (y * W + x) * 4;
    // A taskbar-ish light band and a taskbar-ish dark band.
    out[i] = light ? 243 : 32;
    out[i + 1] = light ? 243 : 32;
    out[i + 2] = light ? 246 : 36;
    out[i + 3] = 255;
  }
}

for (const baseY of [0, bandH]) {
  let x = pad;
  for (const [size, scale] of SHOW) {
    const found = entries.find((e) => e.size === size);
    if (!found) continue;
    const top = baseY + pad + Math.floor((cellH - size * scale) / 2);
    for (let dy = 0; dy < size * scale; dy++) {
      for (let dx = 0; dx < size * scale; dx++) {
        const sx = Math.floor(dx / scale);
        const sy = Math.floor(dy / scale);
        const si = (sy * size + sx) * 4;
        const a = found.px[si + 3] / 255;
        if (a <= 0) continue;
        const o = ((top + dy) * W + (x + dx)) * 4;
        out[o] = found.px[si] * a + out[o] * (1 - a);
        out[o + 1] = found.px[si + 1] * a + out[o + 1] * (1 - a);
        out[o + 2] = found.px[si + 2] * a + out[o + 2] * (1 - a);
      }
    }
    x += size * scale + gap;
  }
}

// PNG encode (filter 0).
const raw = Buffer.alloc((W * 4 + 1) * H);
for (let y = 0; y < H; y++) {
  raw[y * (W * 4 + 1)] = 0;
  out.copy(raw, y * (W * 4 + 1) + 1, y * W * 4, (y + 1) * W * 4);
}
const crcTable = (() => {
  const t = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c;
  }
  return t;
})();
const crc = (buf) => {
  let c = 0xffffffff;
  for (const byte of buf) c = crcTable[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};
const chunk = (type, data) => {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const c = Buffer.alloc(4);
  c.writeUInt32BE(crc(body));
  return Buffer.concat([len, body, c]);
};
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(W, 0);
ihdr.writeUInt32BE(H, 4);
ihdr[8] = 8;
ihdr[9] = 6;
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);
writeFileSync("assets/icon-preview.png", png);
console.log(`wrote assets/icon-preview.png (${W}x${H})`);
