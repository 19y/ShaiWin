// Reads a SHAIWIN_DUMP_FRAME png and reports where the drawn pixels stop.
import fs from "node:fs";
import zlib from "node:zlib";
const buf = fs.readFileSync(process.argv[2]);
let off = 8, w = 0, h = 0, idat = [];
while (off < buf.length) {
  const len = buf.readUInt32BE(off), type = buf.toString("ascii", off + 4, off + 8);
  if (type === "IHDR") { w = buf.readUInt32BE(off + 8); h = buf.readUInt32BE(off + 12); }
  if (type === "IDAT") idat.push(buf.subarray(off + 8, off + 8 + len));
  off += 12 + len;
}
const raw = zlib.inflateSync(Buffer.concat(idat));
const bpp = 4, stride = w * bpp;
const px = Buffer.alloc(h * stride);
let p = 0;
for (let y = 0; y < h; y++) {
  const kind = raw[p++];
  const cur = px.subarray(y * stride, (y + 1) * stride);
  const up = y ? px.subarray((y - 1) * stride, y * stride) : Buffer.alloc(stride);
  for (let x = 0; x < stride; x++) {
    const a = x >= bpp ? cur[x - bpp] : 0, b = up[x], c = x >= bpp ? up[x - bpp] : 0;
    let v = raw[p++];
    if (kind === 1) v += a; else if (kind === 2) v += b; else if (kind === 3) v += (a + b) >> 1;
    else if (kind === 4) { const pp = a + b - c, pa = Math.abs(pp - a), pb = Math.abs(pp - b), pc = Math.abs(pp - c); v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c; }
    cur[x] = v & 255;
  }
}
const alpha = (x, y) => px[(y * w + x) * bpp + 3];
const TH = +(process.argv[3] || 40);
let lowest = -1, lowestX = -1;
for (let y = h - 1; y >= 0; y--) {
  let found = -1;
  for (let x = 0; x < w; x++) if (alpha(x, y) >= TH) { found = x; break; }
  if (found >= 0) { lowest = y; lowestX = found; break; }
}
let ropeMid = -1;
for (let y = 0; y < h; y++) if (alpha(Math.floor(w / 2), y) >= TH) { ropeMid = y; break; }
let ropeEnd = -1;
for (let y = 0; y < h; y++) if (alpha(6, y) >= TH) { ropeEnd = y; break; }
console.log(`size ${w}x${h}  alpha>=${TH}: rope first row at centre=${ropeMid} at x=6=${ropeEnd}  lowest drawn pixel=${lowest} (x=${lowestX})`);
for (const t of [8, 40, 120, 200]) {
  let low = -1;
  for (let y = h - 1; y >= 0; y--) { for (let x = 0; x < w; x++) if (alpha(x, y) >= t) { low = y; break; } if (low >= 0) break; }
  console.log(`  alpha>=${String(t).padStart(3)} lowest row = ${low}`);
}
