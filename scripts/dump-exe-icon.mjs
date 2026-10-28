// Pulls RT_ICON images straight out of a built exe and writes them as PNGs,
// so the picture can be looked at without trusting the .ico file on disk.
//
// Run with: node scripts/dump-exe-icon.mjs <exe> <out-dir>
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { deflateSync } from "node:zlib";

const [exePath, outDir] = process.argv.slice(2);
if (!exePath || !outDir) {
  console.error("usage: node scripts/dump-exe-icon.mjs <exe> <out-dir>");
  process.exit(2);
}

const exe = readFileSync(exePath);
const u16 = (b, o) => b.readUInt16LE(o);
const u32 = (b, o) => b.readUInt32LE(o);

const peOff = u32(exe, 0x3c);
const numSections = u16(exe, peOff + 6);
const optSize = u16(exe, peOff + 20);
const secOff = peOff + 24 + optSize;
let sec = null;
for (let i = 0; i < numSections; i++) {
  const o = secOff + i * 40;
  if (exe.subarray(o, o + 8).toString("ascii").replace(/\0/g, "") === ".rsrc") sec = o;
}
if (sec === null) throw new Error("no .rsrc section");
const va = u32(exe, sec + 12);
const raw = u32(exe, sec + 20);

function walk(dirOff, depth, typeId, nameId) {
  const named = u16(exe, dirOff + 12);
  const ids = u16(exe, dirOff + 14);
  const out = [];
  let e = dirOff + 16 + named * 8;
  for (let i = 0; i < ids; i++, e += 8) {
    const id = u32(exe, e) & 0x7fffffff;
    const child = u32(exe, e + 4) & 0x7fffffff;
    if (depth === 0) out.push(...walk(child + raw, 1, id, 0));
    else if (depth === 1) out.push(...walk(child + raw, 2, typeId, id));
    else {
      const dataEntry = child + raw;
      out.push({
        typeId,
        nameId,
        fileOff: u32(exe, dataEntry) - va + raw,
        size: u32(exe, dataEntry + 4),
      });
    }
  }
  return out;
}

/// A 32 bpp DIB entry: BITMAPINFOHEADER, bottom-up BGRA rows, then an AND mask
/// whose length is only implied by the header's doubled height.
function decodeDib(blob) {
  const w = blob.readInt32LE(4);
  const h = Math.abs(blob.readInt32LE(8)) / 2;
  const bpp = blob.readUInt16LE(14);
  if (bpp !== 32) throw new Error(`expected 32 bpp, got ${bpp}`);
  const stride = w * 4;
  const px = Buffer.alloc(w * h * 4);
  for (let y = 0; y < h; y++) {
    const src = 40 + (h - 1 - y) * stride;
    for (let x = 0; x < w; x++) {
      const s = src + x * 4;
      const d = (y * w + x) * 4;
      px[d] = blob[s + 2];
      px[d + 1] = blob[s + 1];
      px[d + 2] = blob[s];
      px[d + 3] = blob[s + 3];
    }
  }
  return { w, h, px };
}

function encodePng({ w, h, px }) {
  const stride = w * 4;
  const raw = Buffer.alloc((stride + 1) * h);
  for (let y = 0; y < h; y++) {
    raw[y * (stride + 1)] = 0;
    px.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride);
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
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

/// Nearest-neighbour zoom, so a 16 px icon can be inspected pixel by pixel.
function zoom(img, factor) {
  const { w, h, px } = img;
  const out = Buffer.alloc(w * factor * h * factor * 4);
  for (let y = 0; y < h * factor; y++) {
    for (let x = 0; x < w * factor; x++) {
      const s = (Math.floor(y / factor) * w + Math.floor(x / factor)) * 4;
      const d = (y * (w * factor) + x) * 4;
      px.copy(out, d, s, s + 4);
    }
  }
  return { w: w * factor, h: h * factor, px: out };
}

mkdirSync(outDir, { recursive: true });
const icons = walk(raw, 0, 0, 0)
  .filter((x) => x.typeId === 3)
  .sort((a, b) => a.nameId - b.nameId);

for (const entry of icons) {
  const blob = exe.subarray(entry.fileOff, entry.fileOff + entry.size);
  const img = decodeDib(blob);
  const name = `rt-icon-${entry.nameId}-${img.w}px`;
  writeFileSync(join(outDir, `${name}.png`), encodePng(img));
  if (img.w <= 32) {
    writeFileSync(join(outDir, `${name}-zoom8.png`), encodePng(zoom(img, 8)));
  }
  console.log(`rt_icon ${entry.nameId}: ${img.w}x${img.h}`);
}
console.log(`wrote ${icons.length} PNGs to ${outDir}`);
