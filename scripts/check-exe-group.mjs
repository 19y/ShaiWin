// Reads the RT_GROUP_ICON out of a built exe and prints what Windows will
// actually pick from it, then compares that against assets/shaiwin.ico.
//
// check-exe-icon.mjs proves the RT_ICON blobs are byte-identical, but a stale
// RT_GROUP_ICON would still make Explorer draw the wrong entry — the group is
// the table that says which image is the 16 px one.
//
// Run with: node scripts/check-exe-group.mjs <exe> <ico>
import fs from "node:fs";

const [exePath, icoPath] = process.argv.slice(2);
if (!exePath || !icoPath) {
  console.error("usage: node scripts/check-exe-group.mjs <exe> <ico>");
  process.exit(2);
}

const exe = fs.readFileSync(exePath);
const ico = fs.readFileSync(icoPath);
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
      // Third level: the entry points at an IMAGE_RESOURCE_DATA_ENTRY, whose
      // OffsetToData is a real image RVA, so it converts through the section's
      // virtual address rather than through its file pointer.
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

const entries = walk(raw, 0, 0, 0);
const group = entries.find((x) => x.typeId === 14);
if (!group) throw new Error("no RT_GROUP_ICON");

// GRPICONDIR: reserved(2), type(2), count(2), then count × GRPICONDIRENTRY.
const g = exe.subarray(group.fileOff, group.fileOff + group.size);
const count = u16(g, 4);
const fromExe = [];
for (let i = 0; i < count; i++) {
  const o = 6 + i * 14;
  fromExe.push({
    w: g[o] || 256,
    h: g[o + 1] || 256,
    bpp: u16(g, o + 6),
    bytes: u32(g, o + 8),
    id: u16(g, o + 12),
  });
}

const n = u16(ico, 4);
const fromIco = [];
for (let i = 0; i < n; i++) {
  const o = 6 + i * 16;
  fromIco.push({ w: ico[o] || 256, h: ico[o + 1] || 256, bytes: u32(ico, o + 8) });
}

console.log(`${exePath}: RT_GROUP_ICON with ${count} entries`);
console.log(
  "  exe group:",
  fromExe.map((x) => `${x.w}px/${x.bpp}bpp/id${x.id}`).join(" "),
);
console.log("  ico dir  :", fromIco.map((x) => `${x.w}px`).join(" "));

let bad = 0;
if (count !== n) {
  console.log(`FAIL: group has ${count} entries, ico has ${n}`);
  bad++;
}
for (let i = 0; i < Math.min(count, n); i++) {
  if (fromExe[i].w !== fromIco[i].w || fromExe[i].bytes !== fromIco[i].bytes) {
    console.log(`FAIL: entry ${i}: exe ${fromExe[i].w}px/${fromExe[i].bytes}B vs ico ${fromIco[i].w}px/${fromIco[i].bytes}B`);
    bad++;
  }
  if (fromExe[i].bpp !== 32) {
    console.log(`FAIL: entry ${i} is ${fromExe[i].bpp}bpp, expected 32`);
    bad++;
  }
}
console.log(bad === 0 ? "PASS: group icon matches the .ico directory" : `FAIL: ${bad} problem(s)`);
process.exit(bad === 0 ? 0 : 1);
