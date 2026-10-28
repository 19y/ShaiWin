// Scans tracked-ish source files for encoding damage and optionally repairs
// files that PowerShell wrote back in the ANSI code page.
//
// Usage: node scripts/fix-encoding.mjs [--fix]
import { readFileSync, writeFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const fix = process.argv.includes("--fix");
const roots = ["crates", "docs", "platform", "scripts", "include"];
const extra = ["README.md", "Cargo.toml", ".gitignore"];
const exts = new Set([".rs", ".toml", ".md", ".mjs", ".swift", ".h", ".modulemap", ".txt"]);

function walk(dir, out) {
  for (const name of readdirSync(dir)) {
    if (name === "target" || name === "node_modules" || name === ".git") continue;
    const path = join(dir, name);
    const st = statSync(path);
    if (st.isDirectory()) walk(path, out);
    else if (exts.has(name.slice(name.lastIndexOf("."))) || name === "module.modulemap") out.push(path);
  }
}

const files = [];
for (const root of roots) {
  try { walk(root, files); } catch { /* missing dir is fine */ }
}
for (const name of extra) {
  try { if (statSync(name).isFile()) files.push(name); } catch {}
}

const decoder = new TextDecoder("utf-8", { fatal: true });
let bad = 0;
for (const path of files) {
  let bytes;
  try { bytes = readFileSync(path); } catch { continue; }
  let text = null;
  try { text = decoder.decode(bytes); } catch { text = null; }

  const hasReplacement = text !== null && text.includes("\uFFFD");
  // Heuristic for GBK-mangled UTF-8: many high bytes and CJK-looking runs.
  const looksGbk = text === null;
  if (!looksGbk && !hasReplacement) continue;

  bad++;
  console.log(`${relative(process.cwd(), path)}: ${looksGbk ? "invalid UTF-8" : "U+FFFD"} (${bytes.length} bytes)`);
  if (!fix) continue;

  // The file was written back by PowerShell in the ANSI code page (936 here).
  const gbk = new TextDecoder("gbk", { fatal: false });
  const repaired = gbk.decode(bytes);
  writeFileSync(path, repaired, "utf8");
  const leftovers = (repaired.match(/\uFFFD/g) || []).length;
  console.log(`  repaired -> utf8, ${leftovers} unrecoverable char(s)`);
}

console.log(bad === 0 ? "all files are valid UTF-8" : `${bad} file(s) need attention`);
