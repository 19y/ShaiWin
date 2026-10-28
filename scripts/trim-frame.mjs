// Turns a SHAIWIN_DUMP_FRAME png into the README picture.
//
//   node scripts/trim-frame.mjs <in.png> <out.png> [--pad 220] [--width 1280]
//
// The dump is the whole panel: a rope across the full screen width and a small
// cluster of cards in the middle. This keeps the cards plus a length of rope on
// each side, and lays the result on a dark backdrop so the same file reads the
// same on a light and a dark GitHub page.
import fs from "node:fs";
import zlib from "node:zlib";

const [inPath, outPath, ...flags] = process.argv.slice(2);
if (!inPath || !outPath) {
  console.error("usage: node scripts/trim-frame.mjs <in.png> <out.png> [--pad N] [--width N]");
  process.exit(2);
}
const flag = (name, dflt) => {
  const i = flags.indexOf(`--${name}`);
  return i >= 0 ? Number(flags[i + 1]) : dflt;
};
const pad = flag("pad", 220);
const targetWidth = flag("width", 0);

// --- decode ------------------------------------------------------------------

function readPng(path) {
  const buf = fs.readFileSync(path);
  if (buf.readUInt32BE(0) !== 0x89504e47) throw new Error(`${path}: not a png`);
  let off = 8;
  let w = 0, h = 0, color = 0, depth = 0;
  const idat = [];
  while (off < buf.length) {
    const len = buf.readUInt32BE(off);
    const type = buf.toString("ascii", off + 4, off + 8);
    if (type === "IHDR") {
      w = buf.readUInt32BE(off + 8);
      h = buf.readUInt32BE(off + 12);
      depth = buf[off + 16];
      color = buf[off + 17];
    }
    if (type === "IDAT") idat.push(buf.subarray(off + 8, off + 8 + len));
    off += 12 + len;
  }
  if (depth !== 8) throw new Error(`${path}: bit depth ${depth} not supported`);
  const bpp = color === 6 ? 4 : color === 2 ? 3 : color === 4 ? 2 : 1;
  if (![1, 2, 3, 4, 6].includes(color)) throw new Error(`${path}: colour type ${color}`);
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = w * bpp;
  const px = Buffer.alloc(h * stride);
  let p = 0;
  for (let y = 0; y < h; y++) {
    const kind = raw[p++];
    const cur = px.subarray(y * stride, (y + 1) * stride);
    const up = y ? px.subarray((y - 1) * stride, y * stride) : Buffer.alloc(stride);
    for (let x = 0; x < stride; x++) {
      const a = x >= bpp ? cur[x - bpp] : 0;
      const b = up[x];
      const c = x >= bpp ? up[x - bpp] : 0;
      let v = raw[p++];
      if (kind === 1) v += a;
      else if (kind === 2) v += b;
      else if (kind === 3) v += (a + b) >> 1;
      else if (kind === 4) {
        const pp = a + b - c;
        const pa = Math.abs(pp - a), pb = Math.abs(pp - b), pc = Math.abs(pp - c);
        v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      }
      cur[x] = v & 255;
    }
  }
  const rgba = Buffer.alloc(w * h * 4);
  for (let i = 0; i < w * h; i++) {
    for (let k = 0; k < 3; k++) rgba[i * 4 + k] = px[i * bpp + k];
    rgba[i * 4 + 3] = bpp === 4 ? px[i * bpp + 3] : bpp === 2 ? px[i * bpp + 1] : 255;
  }
  return { w, h, rgba };
}

// --- encode ------------------------------------------------------------------

const crcTable = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
function crc32(buf) {
  let c = 0xffffffff;
  for (const byte of buf) c = crcTable[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const head = Buffer.alloc(8);
  head.writeUInt32BE(data.length, 0);
  head.write(type, 4, "ascii");
  const tail = Buffer.alloc(4);
  tail.writeUInt32BE(crc32(Buffer.concat([head.subarray(4), data])), 0);
  return Buffer.concat([head, data, tail]);
}
function writePng(path, w, h, rgba) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // truecolour + alpha
  const stride = w * 4;
  const raw = Buffer.alloc((stride + 1) * h);
  for (let y = 0; y < h; y++) {
    raw[y * (stride + 1)] = 0;
    rgba.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride);
  }
  const png = Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", zlib.deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
  fs.writeFileSync(path, png);
}

// --- the crop ----------------------------------------------------------------

const { w, h, rgba } = readPng(inPath);
const alphaAt = (x, y) => rgba[(y * w + x) * 4 + 3];

// A column with cards in it carries far more ink than a column with only the
// rope, so the cards are found by weight rather than by colour.
const weight = new Array(w).fill(0);
for (let x = 0; x < w; x++) {
  let sum = 0;
  for (let y = 0; y < h; y++) sum += alphaAt(x, y);
  weight[x] = sum;
}
const sorted = [...weight].sort((a, b) => a - b);
const median = sorted[Math.floor(sorted.length / 2)] || 1;
let left = w, right = -1;
for (let x = 0; x < w; x++) {
  if (weight[x] > median * 4) {
    if (x < left) left = x;
    if (x > right) right = x;
  }
}
if (right < left) throw new Error("no cards found in the frame");

left = Math.max(0, left - pad);
right = Math.min(w - 1, right + pad);
let top = h, bottom = -1;
for (let y = 0; y < h; y++) {
  for (let x = left; x <= right; x++) {
    if (alphaAt(x, y) > 0) {
      if (y < top) top = y;
      if (y > bottom) bottom = y;
      break;
    }
  }
}
if (bottom < top) throw new Error("the cropped band is empty");
top = Math.max(0, top - 24);
bottom = Math.min(h - 1, bottom + 24);

const cw = right - left + 1;
const ch = bottom - top + 1;
const scale = targetWidth > 0 && targetWidth < cw ? targetWidth / cw : 1;
const ow = Math.round(cw * scale);
const oh = Math.round(ch * scale);

// Backdrop: the panel's own dark navy, lightest behind the rope.
const out = Buffer.alloc(ow * oh * 4);
for (let y = 0; y < oh; y++) {
  const t = y / (oh - 1);
  const bg = [
    Math.round(26 + 12 * t),
    Math.round(32 + 14 * t),
    Math.round(48 + 18 * t),
  ];
  for (let x = 0; x < ow; x++) {
    // Area-average the source so a downscaled rope does not turn into dashes.
    const sx0 = left + Math.floor(x / scale);
    const sx1 = Math.max(sx0 + 1, left + Math.ceil((x + 1) / scale));
    const sy0 = top + Math.floor(y / scale);
    const sy1 = Math.max(sy0 + 1, top + Math.ceil((y + 1) / scale));
    let r = 0, g = 0, b = 0, a = 0, n = 0;
    for (let sy = sy0; sy < Math.min(sy1, h); sy++) {
      for (let sx = sx0; sx < Math.min(sx1, w); sx++) {
        const i = (sy * w + sx) * 4;
        // tiny-skia hands back premultiplied pixels, which is exactly what an
        // "over" blend against the backdrop needs.
        r += rgba[i];
        g += rgba[i + 1];
        b += rgba[i + 2];
        a += rgba[i + 3];
        n++;
      }
    }
    const o = (y * ow + x) * 4;
    if (n === 0) {
      out[o] = bg[0]; out[o + 1] = bg[1]; out[o + 2] = bg[2]; out[o + 3] = 255;
      continue;
    }
    out[o] = Math.min(255, Math.round(r / n + bg[0] * (1 - a / n / 255)));
    out[o + 1] = Math.min(255, Math.round(g / n + bg[1] * (1 - a / n / 255)));
    out[o + 2] = Math.min(255, Math.round(b / n + bg[2] * (1 - a / n / 255)));
    out[o + 3] = 255;
  }
}
writePng(outPath, ow, oh, out);
console.log(`${inPath}: ${w}x${h}, cards at ${left + pad}..${right - pad}`);
console.log(`${outPath}: ${ow}x${oh}`);
