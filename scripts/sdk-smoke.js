#!/usr/bin/env node
// Simple smoke test for the browser SDK bundle.
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const bundlePath = path.join(__dirname, "..", "assets", "gui-sdk.js");
if (!fs.existsSync(bundlePath)) {
  console.error("Bundle missing; run npm run build-sdk first.");
  process.exit(1);
}

// Minimal DOM stubs
const fakeElement = { dataset: {} };
const context = {
  window: {},
  document: {
    querySelector: () => fakeElement,
  },
  console,
  fetch: async () => ({
    json: async () => ({}),
    ok: true,
  }),
  setTimeout,
};
context.window = context;

const code = fs.readFileSync(bundlePath, "utf8");
vm.runInNewContext(code, context);

const api = context.window && context.window.GreenticGUI;
if (!api) {
  console.error("GreenticGUI global missing");
  process.exit(1);
}

api.attachWorker({
  workerId: "worker.test",
  selector: "#fake",
  routes: ["/"],
});

console.log("SDK smoke test passed");
