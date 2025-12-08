# Repository Overview

## 1. High-Level Purpose
- Rust 2024 binary crate that scaffolds the Greentic GUI runtime.
- Axum-based multi-tenant GUI server that loads GUI packs, enforces auth, injects WIT/file fragments, exposes worker/telemetry/session APIs, and serves a browser SDK; OAuth bearer validation uses greentic-oauth-sdk with a minimal login/logout UX for broker-driven flows.

## 2. Main Components and Functionality
- **Path:** src/main.rs
  - **Role:** Entry-point; initializes telemetry/tracing, loads env config, builds shared state, starts server.
  - **Key functionality:** Wires FsPackProvider or DistributorPackProvider (FilePath/OCI/internal handles), composite fragment renderer (WIT via Wasmtime + file fallback), greentic-session InMemory/Redis manager, greentic-telemetry sink, worker host stub, app shutdown hooks.
- **Path:** src/config.rs
  - **Role:** Runtime configuration.
  - **Key functionality:** Reads bind addr, pack root, default tenant, tenant map, pack cache TTL, env/team/platform defaults, distributor settings, OAuth broker URL, CORS toggle, SDK serving root.
- **Path:** src/server.rs
  - **Role:** Server bootstrap and routing.
  - **Key functionality:** Routes `/api/gui/config`, `/api/gui/worker/message`, `/api/gui/events`, `/api/gui/session`, `/api/gui/cache/clear`, auth start/callback/logout, `/greentic/gui-sdk.js`, catch-all HTML with `/login`/`/logout` static fallbacks; session cookie extraction; fragment injection; graceful shutdown; tenant cache with TTL and invalidation; request span tagging with tenant/path.
- **Path:** src/packs.rs
  - **Role:** Pack models/providers.
  - **Key functionality:** Manifest structs for layout/auth/feature/skin/telemetry; fragment/worker bindings; route normalization; FsPackProvider loads `PACK_ROOT/<tenant>/<pack>/gui/manifest.json`; DistributorPackProvider resolves packs via greentic-distributor-client using JSON-configured refs, supports FilePath/OCI (download + extract) and internal (local path) artifacts with in-memory cache and clear hook; OCI pulls can use `GREENTIC_OCI_BEARER` or basic auth env vars, with warnings on partial creds.
- **Path:** src/tenant.rs
  - **Role:** Tenant GUI configuration and route resolution.
  - **Key functionality:** Aggregates packs into `TenantGuiConfig`; resolves routes with wildcard support; associates fragments with pack asset roots; unit test for feature routing; stores route origin (layout/auth/feature).
- **Path:** src/routing.rs
  - **Role:** Request evaluation.
  - **Key functionality:** Resolves path via TenantGuiConfig, enforces auth via SessionManager, redirects to login on unauthenticated protected routes, loads HTML for serving.
- **Path:** src/fragments.rs
  - **Role:** Fragment renderer integration.
  - **Key functionality:** Composite renderer tries WIT gui-fragment via greentic-interfaces-wasmtime (`fragments/{component}.wasm`) then falls back to file fragments (`fragments/{id}.html`); injects a fragment-error placeholder on render failures with richer logging; DOM injection via kuchiki; unit test covers injection.
- **Path:** src/integration.rs
  - **Role:** Greentic services abstraction.
  - **Key functionality:** SessionManager (greentic-session InMemory/Redis), TelemetrySink (greentic-telemetry), TenantCtx helper; hooks for wiring real storage/telemetry backends.
- **Path:** src/worker.rs
  - **Role:** Worker invocation adapter.
  - **Key functionality:** WorkerBackend trait; WorkerHost delegates to backend; default StubWorkerBackend echoes payloads using HostWorkerRequest/Response (greentic-interfaces-host 0.4.54); HTTP backend scaffold (env-driven via WORKER_GATEWAY_URL/TOKEN/TIMEOUT) builds requests to `/workers/invoke`; WorkerHost is built from env (`worker_backend_from_env`).
- **Path:** src/auth.rs
  - **Role:** OAuth start/callback flow.
  - **Key functionality:** Uses greentic-oauth-client to request auth start URL, redirects to provider; callback expects `id_token`, validates bearer via greentic-oauth-sdk (JWKS/issuer/audience/scopes), issues session via greentic-session with optional cookie Max-Age, logout clears cookie, redirects home.
- **Path:** src/api.rs
  - **Role:** API handlers.
  - **Key functionality:** Returns GUI config (routes/workers/skin), builds TenantCtx from env/team, issues sessions, forwards worker messages via WorkerHost (echo stub), records telemetry events, clears tenant cache, serves SDK script.
- **Path:** src/sdk.rs & assets/gui-sdk.js
  - **Role:** Browser SDK.
  - **Key functionality:** Serves built bundle `assets/gui-sdk.js` (esbuild entry at `src/gui-sdk/index.ts` + typings `src/gui-sdk/index.d.ts`); global `GreenticGUI` with `init`, `attachWorker`, `sendWorkerMessage`, `sendEvent`, `startSession`; Node smoke + assertions via `npm run test-sdk`.
- **Path:** assets/sdk-harness.html
  - **Role:** SDK browser harness.
  - **Key functionality:** Simple page loading `/greentic/gui-sdk.js` and attaching a test worker slot; served at `/tests/sdk-harness` for Playwright tests.
- **Path:** assets/login.html
  - **Role:** Minimal login UX.
  - **Key functionality:** Static login page with required selectors/buttons calling `/auth/{provider}/start`; used as fallback when no auth pack serves `/login`.
- **Path:** assets/logout.html
  - **Role:** Minimal logout UX.
  - **Key functionality:** Static logout page with a button that triggers `/auth/logout`; `/logout` routes there when not served by a pack.
- **Path:** package.json
  - **Role:** SDK build tooling.
  - **Key functionality:** `npm run build-sdk` bundles SDK via esbuild to `assets/gui-sdk.js`; exposes types at `src/gui-sdk/index.d.ts`.
- **Path:** ci/local_check.sh
  - **Role:** Local CI helper.
  - **Key functionality:** Runs `cargo fmt`, `cargo clippy --all-targets --all-features -D warnings`, and `cargo test`.
- **Path:** src/server.rs (pack ops)
  - **Role:** Server bootstrap and routing.
  - **Key functionality:** Adds `/api/gui/packs/reload` (POST JSON `{tenant}`) to clear and re-warm pack cache (logs cache hit/miss counters); `/api/gui/cache/clear` clears cache only.

## 3. Work In Progress, TODOs, and Stubs
- **Fragment rendering:** WIT path uses greentic-interfaces-wasmtime over `fragments/{component}.wasm`; needs real component artifacts and richer error handling; compiled component caching is in place but still minimal.
- **Auth flow:** Callback still expects `id_token` query from broker; basic static login page exists but pack-driven UI is still expected; provider routing remains minimal.
- **Pack provider:** Distributor internal artifacts treated as local paths; OCI auth supports bearer or basic via env vars but still lacks hot-reload/watchers and richer auth flows.
- **Workers/telemetry:** WorkerHost delegates to a pluggable WorkerBackend (env-driven HTTP backend to `/workers/invoke` when `WORKER_GATEWAY_URL` is set, otherwise stub echo) with retries/backoff; no local Wasmtime/runner execution; no response streaming; telemetry sets TelemetryCtx but remains basic.
- **SDK:** Bundle is plain JS with typings and Node tests (`scripts/sdk-smoke.js` + `scripts/sdk-tests.js`, run via `npm run test-sdk`); build/test wired into `ci/local_check.sh`; no browser-based tests yet.
  Browser: Playwright harness/script exists (`npm run test:browser`) targeting `/tests/sdk-harness` but requires a running server.
- **Sessions/storage:** Session store supports Redis via `REDIS_URL` with in-memory fallback; cookie Max-Age configurable via `SESSION_TTL_SECS`, but store-level expiry/cleanup is unchanged.

## 4. Broken, Failing, or Conflicting Areas
- None observed; `cargo test` passes (2 tests). Current warnings stem from unused placeholder code/fields.

## 5. Notes for Future Work
- Integrate real OAuth broker token exchange + ID token verification and login UI; align session issuance with auth pack settings.
- Produce/ship WIT fragment components, add Wasmtime cache/pooling, and surface fragment render errors to telemetry.
- Enhance distributor internal/OCI handling (auth, caching, hot-reload) and multi-pack routing; add persistent session store.
- Add SDK typings/tests and wire `npm run build-sdk` into CI; document build prerequisites.
- Expand telemetry context propagation (TelemetryCtx), and implement a real remote WorkerBackend (HTTP/NATS) now that host worker types are exposed.
