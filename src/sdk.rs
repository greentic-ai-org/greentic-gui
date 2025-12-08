pub fn sdk_script() -> String {
    r#"// Greentic GUI SDK (lightweight stub until full build pipeline is added)
(function(global) {
  const version = "0.2.0";
  let config = null;

  async function init(opts = {}) {
    config = {
      tenantDomain: opts.tenantDomain || window.location.host,
      configUrl: opts.configUrl || "/api/gui/config",
      eventsUrl: opts.eventsUrl || "/api/gui/events",
      workerMessageUrl: opts.workerMessageUrl || "/api/gui/worker/message",
    };
    try {
      const res = await fetch(config.configUrl);
      config.guiConfig = await res.json();
    } catch (err) {
      console.warn("GreenticGUI: failed to load GUI config", err);
    }
    return config;
  }

  function attachWorker({ workerId, selector, routes = [] }) {
    const el = document.querySelector(selector);
    if (!el) {
      console.warn("GreenticGUI: worker target not found for", selector);
      return null;
    }
    el.dataset.greenticWorker = workerId;
    el.dataset.greenticRoutes = routes.join(",");
    return el;
  }

  async function sendWorkerMessage({ workerId, payload = {}, context = {} }) {
    if (!config) await init();
    const body = {
      worker_id: workerId,
      payload,
      context: Object.assign({ path: window.location.pathname }, context),
    };
    const res = await fetch(config.workerMessageUrl, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    return res.json();
  }

  async function sendEvent({ eventType, metadata = {} }) {
    if (!config) await init();
    try {
      await fetch(config.eventsUrl, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          event_type: eventType,
          path: window.location.pathname,
          timestamp: Date.now(),
          metadata,
        }),
      });
    } catch (err) {
      console.warn("GreenticGUI: failed to send event", err);
    }
  }

  async function startSession({ userId, team }) {
    if (!config) await init();
    const res = await fetch("/api/gui/session", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ user_id: userId, team }),
    });
    if (!res.ok) {
      throw new Error("Failed to start session");
    }
    return res.json();
  }

  global.GreenticGUI = { version, init, attachWorker, sendWorkerMessage, sendEvent, startSession };
})(window);
"#
    .to_string()
}
