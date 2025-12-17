# Migration Status — greentic-gui (secrets public launch)

- What changed: pack/distributor secret requirements are now parsed and exposed via `/api/gui/config` (with a `pack_init_hint`), and worker missing-secret errors return structured responses plus `greentic-secrets init --pack <path>` remediation.
- What broke: nothing known; missing-secrets paths now return a 428-style payload instead of generic 500s.
- Next repos to update after this GUI work: greentic-distributor/runner/dev to emit structured missing-secrets errors and pack secret_requirements (handled by other Codex owners per program plan).
- Notes: OCI/distributor pulls now produce a cached `.gtpack` (or discovered gtpack) hint when possible so the remediation command is actionable locally.
