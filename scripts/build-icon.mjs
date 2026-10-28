// Draws assets/shaiwin.ico: a bed sheet with a face, three drawings for three
// size bands, nine PNGs, one .ico.
//
// The sheet is simple enough that one drawing serves 48 px and up, but a 2%
// outline lands on a third of a pixel at 16 px. So the small sizes get their
// own drawing: a thinner outline and a deeper hem band, which at that size
// reads as folded cloth instead of a picture frame.
//
// Rendering goes through Edge's headless screenshot mode, because it is the one
// SVG rasteriser that is already on every Windows box this builds on. The PNGs
// come back straight-alpha, so packing them into a DIB is a channel shuffle.
//
// Run with: node scripts/build-icon.mjs
// Env: EDGE=/path/to/msedge.exe to override the browser lookup.
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { deflateSync, inflateSync } from "node:zlib";

// --- geometry ----------------------------------------------------------------

// One path for the cloth: a rectangle whose bottom edge sags into four waves,
// because a bed sheet hanging over nothing still hangs.
const SHEET =
  "M158 244 C158 230 170 220 186 220 L838 220 C854 220 866 230 866 244 " +
  "C874 420 878 620 868 786 C812 820 748 838 682 826 C622 815 578 842 518 830 " +
  "C458 818 412 842 354 828 C294 814 212 816 156 786 C146 620 150 420 158 244 Z";

// The top fold: a band across the head of the sheet, the way a sheet is turned
// over at the top. This is what says "linen" rather than "square".
const HEM =
  "M158 244 C158 230 170 220 186 220 L838 220 C854 220 866 230 866 244 " +
  "C866 258 866 272 866 286 L158 286 C158 272 158 258 158 244 Z";

const INK = "#3A4A63";

function defs() {
  return `  <defs>
    <linearGradient id="cloth" x1="0.2" y1="0" x2="0.5" y2="1">
      <stop offset="0" stop-color="#FFFDF8"/>
      <stop offset="0.55" stop-color="#FCF7EC"/>
      <stop offset="1" stop-color="#F0E6D3"/>
    </linearGradient>
    <filter id="soft" x="-25%" y="-25%" width="150%" height="150%">
      <feDropShadow dx="0" dy="10" stdDeviation="14" flood-color="#2C3A52" flood-opacity="0.10"/>
    </filter>
  </defs>`;
}

function sheet(stroke, shadow = true) {
  const filt = shadow ? ' filter="url(#soft)"' : "";
  return (
    `<path d="${SHEET}" fill="url(#cloth)" stroke="${INK}" stroke-width="${stroke}" ` +
    `stroke-linejoin="round"${filt}/>`
  );
}

/// Two eyes, and optionally a blush and a smile. The small sizes pass zeros for
/// the optional parts: at 16 px a 3-unit blush is one muddy pixel.
function face(eyeR, eyeDx, eyeY, blushR, smileWidth, smile) {
  const out = [
    `<g fill="${INK}"><circle cx="${512 - eyeDx}" cy="${eyeY}" r="${eyeR}"/>` +
      `<circle cx="${512 + eyeDx}" cy="${eyeY}" r="${eyeR}"/></g>`,
  ];
  if (blushR) {
    out.push(
      `<g fill="#FFB3A7" opacity="0.85">` +
        `<circle cx="${512 - eyeDx - 54}" cy="${eyeY + 64}" r="${blushR}"/>` +
        `<circle cx="${512 + eyeDx + 54}" cy="${eyeY + 64}" r="${blushR}"/></g>`,
    );
  }
  if (smile) {
    out.push(
      `<path d="M${512 - 38} ${eyeY + 70} Q512 ${eyeY + 112} ${512 + 38} ${eyeY + 70}" ` +
        `fill="none" stroke="${INK}" stroke-width="${smileWidth}" stroke-linecap="round"/>`,
    );
  }
  return out.join("\n  ");
}

function svg(body, scale = 1) {
  if (scale !== 1) {
    body =
      `<g transform="translate(512 512) scale(${scale}) translate(-512 -512)">\n` +
      `${body}\n</g>`;
  }
  // A slight tilt: a sheet pinned up by nobody is never perfectly square.
  const g = `<g transform="rotate(-3 512 512)">\n${body}\n</g>`;
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" ` +
    `width="1024" height="1024">\n${defs()}\n${g}\n</svg>\n`
  );
}

// --- the three drawings ------------------------------------------------------

// 48 and up: everything, including the stitching along the hem.
const FULL = svg(
  sheet(22) +
    `\n  <path d="${HEM}" fill="#EFE2CC"/>` +
    `\n  <path d="M158 286 L866 286" stroke="${INK}" stroke-width="9" stroke-opacity="0.22"/>` +
    `\n  <path d="M176 300 L850 300" stroke="${INK}" stroke-width="7" stroke-opacity="0.14" ` +
    `stroke-dasharray="26 22" stroke-linecap="round"/>` +
    "\n  " +
    face(20, 72, 540, 26, 14, true),
);

// 24 / 32 / 40: no stitching, heavier outline, bigger face. The hem is darker
// than the 48+ one because by this point the band is only three pixels tall and
// a pale beige on a near-white body leaves the sheet looking like an empty
// frame. Contrast has to climb as the size drops.
const MID = svg(
  sheet(48) +
    `\n  <path d="${HEM}" fill="#DFC9A4"/>` +
    `\n  <path d="M158 286 L866 286" stroke="${INK}" stroke-width="16" stroke-opacity="0.22"/>` +
    "\n  " +
    face(38, 84, 548, 44, 24, true),
  1.04,
);

// 16 / 20: outline, hem band, two eyes. The stroke cannot go much thicker or
// the cloth turns into a frame, so the hem colour carries the "this is cloth"
// job instead of a divider line, which at this size just looks like a title bar.
// The band has to be darker than the big sizes' #EFE2CC: two pixels of pale
// beige on a near-white gradient is invisible, and the icon reads as an empty
// box. 1.06 rather than 1.12 keeps a pixel of margin at 16 px instead of
// running the antialiased edge off both sides.
const SMALL = svg(
  sheet(70, false) +
    `\n  <path d="${HEM}" fill="#D0B88E"/>` +
    "\n  " +
    face(58, 104, 566, 0, 0, false),
  1.06,
);

const VARIANTS = [
  { name: "full", text: FULL, sizes: [48, 64, 128, 256] },
  { name: "mid", text: MID, sizes: [24, 32, 40] },
  { name: "small", text: SMALL, sizes: [16, 20] },
];

// --- rendering ---------------------------------------------------------------

const EDGE_CANDIDATES = [
  process.env.EDGE,
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  "C:/Program Files/Microsoft/Edge/Application/msedge.exe",
].filter(Boolean);

const edge = EDGE_CANDIDATES.find((p) => existsSync(p));
if (!edge) {
  throw new Error(
    "找不到 Edge，用 EDGE=<msedge.exe 的路径> 指定一个。" +
      "Edge 只用来把 SVG 光栅化，任何 Chromium 内核的浏览器都能顶替。",
  );
}

const scratch = mkdtempSync(join(tmpdir(), "shaiwin-icon-"));

/// Rasterises one SVG at one size. The transparent background flag is what
/// keeps the icon's corners transparent instead of white.
///
/// `display:block` is not cosmetic. An inline <svg> is a replaced inline
/// element, so it sits on the text baseline and the line box reserves descender
/// space underneath it. When the viewport is exactly the icon size there is no
/// room for that space, and the browser pushes the drawing down by the
/// descender — at 16 px that is three pixels, which cuts the bottom off the
/// sheet. 20 px and up happen to be tall enough that the strut fits, so the bug
/// only ever showed on the smallest entry.
function render(text, size) {
  const page = join(scratch, `r${size}.html`);
  const png = join(scratch, `r${size}.png`);
  const sized = text
    .replace('width="1024" height="1024"', `width="${size}" height="${size}"`)
    .replace("<svg ", '<svg style="display:block" ');
  writeFileSync(
    page,
    `<!doctype html><meta charset="utf-8">` +
      `<body style="margin:0;background:transparent">${sized}</body>`,
    "utf8",
  );
  execFileSync(edge, [
    "--headless=new",
    "--disable-gpu",
    "--hide-scrollbars",
    "--default-background-color=00000000",
    "--force-device-scale-factor=1",
    `--screenshot=${png}`,
    `--window-size=${size},${size}`,
    `file:///${page.replace(/\\/g, "/")}`,
  ], { stdio: "ignore" });
  const img = decodePng(readFileSync(png));
  if (img.w !== size || img.h !== size) {
    throw new Error(`${size}px: browser returned ${img.w}x${img.h}`);
  }
  return img;
}

// --- PNG decoding ------------------------------------------------------------

/// Chunked, filtered scanlines back into straight RGBA. Only what a browser
/// screenshot produces is supported: 8 bit, non-interlaced, RGB or RGBA.
function decodePng(buf) {
  if (buf.toString("hex", 0, 8) !== "89504e470d0a1a0a") throw new Error("not a PNG");
  let ihdr = null;
  const idat = [];
  let off = 8;
  while (off + 12 <= buf.length) {
    const len = buf.readUInt32BE(off);
    const type = buf.toString("ascii", off + 4, off + 8);
    if (type === "IHDR") ihdr = buf.subarray(off + 8, off + 8 + len);
    else if (type === "IDAT") idat.push(buf.subarray(off + 8, off + 8 + len));
    off += 12 + len;
  }
  const w = ihdr.readUInt32BE(0);
  const h = ihdr.readUInt32BE(4);
  const [depth, color, compression, filter, interlace] = [
    ihdr[8], ihdr[9], ihdr[10], ihdr[11], ihdr[12],
  ];
  if (depth !== 8 || interlace !== 0 || compression !== 0 || filter !== 0) {
    throw new Error(`unsupported PNG: depth=${depth} interlace=${interlace}`);
  }
  if (color !== 6 && color !== 2) throw new Error(`unsupported PNG colour type ${color}`);
  const channels = color === 6 ? 4 : 3;
  const raw = inflateSync(Buffer.concat(idat));
  const stride = w * channels;
  const px = Buffer.alloc(h * stride);
  let pos = 0;
  for (let y = 0; y < h; y++) {
    const kind = raw[pos++];
    const line = raw.subarray(pos, pos + stride);
    pos += stride;
    const cur = px.subarray(y * stride, (y + 1) * stride);
    const up = y > 0 ? px.subarray((y - 1) * stride, y * stride) : null;
    unfilter(kind, cur, line, up, channels);
  }
  return { w, h, channels, px };
}

function unfilter(kind, cur, line, up, channels) {
  if (kind === 0) {
    line.copy(cur);
    return;
  }
  for (let i = 0; i < line.length; i++) {
    const left = i >= channels ? cur[i - channels] : 0;
    const above = up ? up[i] : 0;
    let pred;
    if (kind === 1) pred = left;
    else if (kind === 2) pred = above;
    else if (kind === 3) pred = (left + above) >> 1;
    else if (kind === 4) {
      const upperLeft = up && i >= channels ? up[i - channels] : 0;
      const p = left + above - upperLeft;
      const pa = Math.abs(p - left);
      const pb = Math.abs(p - above);
      const pc = Math.abs(p - upperLeft);
      pred = pa <= pb && pa <= pc ? left : pb <= pc ? above : upperLeft;
    } else throw new Error(`unknown filter type ${kind}`);
    // Buffer assignment wraps at 256, which is the arithmetic PNG wants.
    cur[i] = line[i] + pred;
  }
}

// --- the .ico container ------------------------------------------------------

/// One 32 bpp DIB entry: BITMAPINFOHEADER with the doubled height that tells a
/// reader an AND mask follows, then bottom-up BGRX rows, then that mask.
function dibEntry(img) {
  const { w, h, channels, px } = img;
  const header = Buffer.alloc(40);
  header.writeUInt32LE(40, 0);
  header.writeInt32LE(w, 4);
  header.writeInt32LE(h * 2, 8);
  header.writeUInt16LE(1, 12);
  header.writeUInt16LE(32, 14);
  header.writeUInt32LE(w * h * 4, 20);
  const xor = Buffer.alloc(w * h * 4);
  const maskStride = Math.ceil(w / 32) * 4;
  const and = Buffer.alloc(maskStride * h);
  for (let y = 0; y < h; y++) {
    const srcRow = h - 1 - y; // bottom-up
    for (let x = 0; x < w; x++) {
      const i = (srcRow * w + x) * channels;
      const o = (y * w + x) * 4;
      const a = channels === 4 ? px[i + 3] : 255;
      xor[o] = px[i + 2];
      xor[o + 1] = px[i + 1];
      xor[o + 2] = px[i];
      xor[o + 3] = a;
      if (a < 8) and[y * maskStride + (x >> 3)] |= 0x80 >> (x & 7);
    }
  }
  return Buffer.concat([header, xor, and]);
}

function writeIco(bySize, path) {
  const sizes = [...bySize.keys()].sort((a, b) => a - b);
  const images = sizes.map((size) => ({ size, data: dibEntry(bySize.get(size)) }));
  const dir = Buffer.alloc(6);
  dir.writeUInt16LE(0, 0);
  dir.writeUInt16LE(1, 2);
  dir.writeUInt16LE(images.length, 4);
  let offset = 6 + images.length * 16;
  const entries = images.map(({ size, data }) => {
    const e = Buffer.alloc(16);
    e[0] = size === 256 ? 0 : size; // 256 does not fit in a byte, so it is 0
    e[1] = size === 256 ? 0 : size;
    e.writeUInt16LE(1, 4);
    e.writeUInt16LE(32, 6);
    e.writeUInt32LE(data.length, 8);
    e.writeUInt32LE(offset, 12);
    offset += data.length;
    return e;
  });
  const ico = Buffer.concat([dir, ...entries, ...images.map((i) => i.data)]);
  writeFileSync(path, ico);
  return { bytes: ico.length, sizes };
}

// --- main --------------------------------------------------------------------

const bySize = new Map();
for (const variant of VARIANTS) {
  for (const size of variant.sizes) {
    bySize.set(size, render(variant.text, size));
    console.log(`rendered ${size}px from the ${variant.name} drawing`);
  }
}

const { bytes, sizes } = writeIco(bySize, "assets/shaiwin.ico");
console.log(`wrote assets/shaiwin.ico (${bytes} bytes, ${sizes.join("/")})`);

// The master art: the 1024 version of the full drawing, transparent, for
// store listings and anything that wants one big picture.
const big = render(FULL, 1024);
writeFileSync("assets/new_icon.png", encodePng(big));
console.log("wrote assets/new_icon.png (1024x1024)");

rmSync(scratch, { recursive: true, force: true });

// --- PNG encoding (only for the master art) ----------------------------------

function encodePng(img) {
  const { w, h, channels, px } = img;
  const stride = w * 4;
  const raw = Buffer.alloc((stride + 1) * h);
  for (let y = 0; y < h; y++) {
    raw[y * (stride + 1)] = 0; // filter: none
    for (let x = 0; x < w; x++) {
      const s = (y * w + x) * channels;
      const d = y * (stride + 1) + 1 + x * 4;
      raw[d] = px[s];
      raw[d + 1] = px[s + 1];
      raw[d + 2] = px[s + 2];
      raw[d + 3] = channels === 4 ? px[s + 3] : 255;
    }
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
