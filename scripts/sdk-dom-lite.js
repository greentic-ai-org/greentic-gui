/* Minimal DOM-lite assertions for GreenticGUI without external deps. */
const assert = require("assert");
const fs = require("fs");
const path = require("path");
const vm = require("vm");

// Fake DOM elements
class Element {
  constructor(selector) {
    this.selector = selector;
    this.dataset = {};
  }
}

const dom = {
  elements: {},
  querySelector(selector) {
    if (!this.elements[selector]) {
      this.elements[selector] = new Element(selector);
    }
    return this.elements[selector];
  },
};

// Sandbox with fake window/document/fetch.
const events = [];
const sandbox = {
  window: { location: { host: "localhost", pathname: "/" }, document: dom },
  document: dom,
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
const bundlePath = path.join(__dirname, "..", "assets", "gui-sdk.js");
vm.runInContext(fs.readFileSync(bundlePath, "utf8"), sandbox);

assert(sandbox.window.GreenticGUI, "GreenticGUI global is defined");
const target = sandbox.window.GreenticGUI.attachWorker({
  workerId: "worker.test",
  selector: "#slot",
  routes: ["/"],
});
assert(target, "attachWorker returns element");
assert.strictEqual(target.dataset.greenticWorker, "worker.test");
assert.strictEqual(target.dataset.greenticRoutes, "/");

console.log("sdk-dom-lite.js passed");
