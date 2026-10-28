// Check that every RT_ICON resource in a built exe is byte-identical to the
// matching image in assets/shaiwin.ico. Usage:
//   node gen/check-exe-icon.mjs <exe> <ico>
import fs from 'node:fs';

const exePath = process.argv[2];
const icoPath = process.argv[3];
if (!exePath || !icoPath) {
  console.error('usage: node gen/check-exe-icon.mjs <exe> <ico>');
  process.exit(2);
}

const exe = fs.readFileSync(exePath);
const ico = fs.readFileSync(icoPath);

function u16(b, o) { return b.readUInt16LE(o); }
function u32(b, o) { return b.readUInt32LE(o); }

const peOff = u32(exe, 0x3c);
const numSections = u16(exe, peOff + 6);
const optSize = u16(exe, peOff + 20);
const secOff = peOff + 24 + optSize;
let sec = null;
for (let i = 0; i < numSections; i++) {
  const o = secOff + i * 40;
  if (exe.subarray(o, o + 8).toString('ascii').replace(/\0/g, '') === '.rsrc') sec = o;
}
if (sec === null) {
  console.error('no .rsrc section');
  process.exit(1);
}
const va = u32(exe, sec + 12);
const raw = u32(exe, sec + 20);

// The directory offsets and data RVAs in this binary are relative to the start
// of the resource section, not to the image base — that is how the linker lays
// them out here, so the conversion adds the section's file pointer and ignores
// its virtual address.
function offFromVa(address) {
  return address + raw;
}

const RT_ICON = 3;
const RT_GROUP_ICON = 14;

// Each directory header is: characteristics(4), timestamp(4), version(4),
// NumberOfNamedEntries(u16), NumberOfIdEntries(u16). The entries are 8 bytes
// each — an identifier and an RVA — and the named ones come first, so the ID
// entries start after them.
function walk(dirOff, depth, typeId, nameId) {
  const named = u16(exe, dirOff + 12);
  const ids = u16(exe, dirOff + 14);
  const out = [];
  let e = dirOff + 16 + named * 8;
  for (let i = 0; i < ids; i++, e += 8) {
    const id = u32(exe, e) & 0x7fffffff;
    // The high bit of the second field says "this points at another directory";
    // the rest is the offset.
    const child = offFromVa(u32(exe, e + 4) & 0x7fffffff);
    if (depth === 0) out.push(...walk(child, 1, id, 0));
    else if (depth === 1) out.push(...walk(child, 2, typeId, id));
    else {
      // The third level's entries point straight at an
      // IMAGE_RESOURCE_DATA_ENTRY. Its OffsetToData is a real image RVA (unlike
      // the directory offsets above, which are section-relative), so it converts
      // through the section's virtual address.
      out.push({
        typeId,
        nameId,
        fileOff: u32(exe, child) - va + raw,
        size: u32(exe, child + 4),
      });
    }
  }
  return out;
}

const entries = walk(raw, 0, 0, 0);
const icons = entries.filter((x) => x.typeId === RT_ICON).sort((a, b) => a.nameId - b.nameId);
const groups = entries.filter((x) => x.typeId === RT_GROUP_ICON);

const count = u16(ico, 4);
console.log(`ico: ${count} images; exe: ${icons.length} RT_ICON, ${groups.length} RT_GROUP_ICON`);

const sizes = [];
for (let i = 0; i < count; i++) {
  const d = 6 + i * 16;
  const w = ico[d] || 256;
  const h = ico[d + 1] || 256;
  sizes.push(`${w}x${h}`);
}
console.log('ico sizes:', sizes.join(' '));

let bad = 0;
for (let i = 0; i < count; i++) {
  const entry = icons[i];
  if (!entry) {
    console.log(`image ${i}: MISSING in exe`);
    bad++;
    continue;
  }
  const imageLen = u32(ico, 6 + i * 16 + 8);
  const imageOff = u32(ico, 6 + i * 16 + 12);
  const fromIco = ico.subarray(imageOff, imageOff + imageLen);
  const fromExe = exe.subarray(entry.fileOff, entry.fileOff + entry.size);
  const same = fromIco.length === fromExe.length && fromIco.equals(fromExe);
  if (!same) bad++;
  console.log(`image ${i} ${sizes[i]}: ${same ? 'identical' : `DIFFERS (exe ${fromExe.length} vs ico ${fromIco.length})`}`);
}
if (groups.length !== 1) {
  console.log(`FAIL: expected exactly one RT_GROUP_ICON, found ${groups.length}`);
  bad++;
}
console.log(bad === 0 ? 'PASS: exe icon matches assets/shaiwin.ico' : `FAIL: ${bad} problem(s)`);
process.exit(bad === 0 ? 0 : 1);
