#!/usr/bin/env node
const { spawn } = require("child_process");
const path = require("path");
const fs = require("fs");

const mapping = {
  "darwin:arm64": "darwin-arm64",
  "darwin:x64": "darwin-x64",
  "linux:x64": "linux-x64",
  "win32:x64": "win32-x64",
};

const key = `${process.platform}:${process.arch}`;
const target = mapping[key];

if (!target) {
  console.error(`infraware: unsupported platform "${process.platform}-${process.arch}". Supported targets: ${Object.values(mapping).join(", ")}. This target is not yet supported/deferred.`);
  process.exit(1);
}

const isWin = target === "win32-x64";
const binName = isWin ? "infraware.exe" : "infraware";
const binPath = path.join(__dirname, "..", "binaries", target, binName);

if (!fs.existsSync(binPath)) {
  console.error(`infraware: binary not found for target "${target}" at ${binPath}`);
  process.exit(1);
}

const child = spawn(binPath, process.argv.slice(2), { stdio: "inherit" });
child.on("error", (err) => {
  console.error(`infraware: failed to spawn binary: ${err.message}`);
  process.exit(1);
});
child.on("close", (code, signal) => {
  if (signal) process.exit(1);
  process.exit(code ?? 1);
});
