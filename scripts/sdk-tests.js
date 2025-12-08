/* Lightweight SDK assertions (Node context). */
const assert = require("assert");
const path = require("path");
const { readFileSync } = require("fs");
const vm = require("vm");

// Load the built SDK bundle.
const bundlePath = path.join(__dirname, "..", "assets", "gui-sdk.js");
const code = readFileSync(bundlePath, "utf8");

// Create a sandboxed context with a fake window/fetch.
const events = [];
const sandbox = {
  window: {},
  fetch: async (url, opts) => {
    events.push({ url, opts });
    return {
      ok: true,
      json: async () => ({ status: "ok", url }),
    };
  },
  console,
};
vm.createContext(sandbox);
vm.runInContext(code, sandbox);

assert(sandbox.window.GreenticGUI, "GreenticGUI global is defined");
sandbox.window.location = { host: "localhost", pathname: "/" };

(async () => {
  await sandbox.window.GreenticGUI.sendEvent({ eventType: "test", metadata: { ok: true } });
  const eventCalls = events.filter((e) => e.url.includes("/api/gui/events"));
  assert.strictEqual(eventCalls.length, 1, "sendEvent should POST once");

  // Ensure init sets defaults and does not throw when fetch fails (we override fetch).
  await sandbox.window.GreenticGUI.init({ configUrl: "/api/gui/config" });
  await sandbox.window.GreenticGUI.sendWorkerMessage({ workerId: "w", payload: { a: 1 } });
  assert(events.some((e) => e.url.includes("/api/gui/worker/message")), "worker message should POST");
  console.log("sdk-tests.js passed");
})();
