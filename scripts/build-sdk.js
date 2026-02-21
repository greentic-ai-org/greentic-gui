#!/usr/bin/env node
"use strict";

const { existsSync } = require("fs");
const { join } = require("path");
const projectRoot = join(__dirname, "..");
const bundlePath = join(projectRoot, "assets", "gui-sdk.js");

function canFallback(err) {
  const msg = String((err && err.message) || "");
  return (
    msg.includes("another platform") ||
    msg.includes("Exec format error") ||
    msg.includes("spawn") ||
    msg.includes("ENOENT")
  );
}

async function main() {
  try {
    const esbuild = require("esbuild");
    await esbuild.build({
      entryPoints: ["src/gui-sdk/index.ts"],
      bundle: true,
      format: "iife",
      outfile: "assets/gui-sdk.js",
      absWorkingDir: projectRoot,
      logLevel: "info",
    });
    return;
  } catch (err) {
    if (canFallback(err) && existsSync(bundlePath)) {
      console.warn(
        "esbuild is not runnable on this machine; reusing existing assets/gui-sdk.js"
      );
      return;
    }
    throw err;
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
