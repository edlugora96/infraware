#!/usr/bin/env node
const fs = require("fs");
const path = require("path");
const pkgDir = path.resolve(__dirname, "..");
const targets = [
  { dir: "darwin-arm64", file: "infraware", magic: Buffer.from([0xcf, 0xfa, 0xed, 0xfe]), altMagic: Buffer.from([0xfe, 0xed, 0xfa, 0xcf]), name: "Mach-O" },
  { dir: "darwin-x64", file: "infraware", magic: Buffer.from([0xcf, 0xfa, 0xed, 0xfe]), altMagic: Buffer.from([0xfe, 0xed, 0xfa, 0xcf]), name: "Mach-O" },
  { dir: "linux-x64", file: "infraware", magic: Buffer.from([0x7f, 0x45, 0x4c, 0x46]), name: "ELF" },
  { dir: "win32-x64", file: "infraware.exe", magic: Buffer.from([0x4d, 0x5a]), name: "PE/MZ" },
];
let failed = false;
for (const t of targets) {
  const p = path.join(pkgDir, "binaries", t.dir, t.file);
  if (!fs.existsSync(p)) { console.error(`Missing ${p}`); failed=true; continue; }
  const buf = fs.readFileSync(p);
  if (buf.length < 10) { console.error(`${p} too small`); failed=true; continue; }
  // reject placeholder/text
  const text = buf.slice(0, 1024).toString("utf8");
  if (/placeholder/i.test(text) && buf.length < 1000) { console.error(`${p} appears to be placeholder`); failed=true; continue; }
  // check not text file (has null bytes for binaries)
  // magic check
  const isMachO = t.magic && (buf.slice(0,4).equals(t.magic) || (t.altMagic && buf.slice(0,4).equals(t.altMagic)) || buf.slice(0,4).equals(Buffer.from([0xca,0xfe,0xba,0xbe])) );
  const isELF = t.name==="ELF" && buf.slice(0,4).equals(t.magic);
  const isPE = t.name==="PE/MZ" && buf.slice(0,2).equals(t.magic);
  let ok = false;
  if (t.name==="Mach-O") ok=isMachO;
  else if (t.name==="ELF") ok=isELF;
  else if (t.name==="PE/MZ") ok=isPE;
  if (!ok) { console.error(`${p} invalid magic, expected ${t.name} got ${buf.slice(0,4).toString("hex")}`); failed=true; continue; }
  if (t.dir !== "win32-x64") {
    try { fs.accessSync(p, fs.constants.X_OK); } catch { console.error(`${p} not executable`); failed=true; }
  }
}
if (failed) { console.error("verify-binaries: FAILED"); process.exit(1); }
console.log("verify-binaries: OK");
