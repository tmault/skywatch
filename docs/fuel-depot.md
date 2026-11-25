# Subscription fuel depot

The depot displays **authoritative subscription allowances**, independently of the
thread/aircraft driver. It does not estimate allowances from tokens, costs, context
occupancy, or rate-limit errors. The live default is explicitly unavailable.

## Current T3 transport limitation (2026-11-25)

Read-only inspection of the installed T3 archive confirmed:

- `packages/contracts/src/providerUsageLimits.ts` (in
  `ManagedEndpointRuntime-DBgt8khC.mjs`) defines `usedPercent` in [0,100],
  `checkedAt`, `resetsAt`, `windowDurationMins`, `kind`, `modelIds`, `providerIds`.
- `server.getConfig` is a read RPC with `{}` input. `subscribeServerConfig`
  supports `usageLimitSources: true`. Both require `orchestration:read`.
- The supported HTTP auth surface issues tickets through
  `POST /api/auth/websocket-ticket`, then the authenticated WebSocket RPC
  transport carries config messages. Config is broader than the desired quota DTO.
- Skywatch's existing paired MCP credential was tried against that ticket
  endpoint and received **HTTP 401**. No response bodies or credentials were
  logged. The MCP tool catalog / CLI exposes no narrow usage read operation.

Therefore **this branch does not provide a live T3 usage integration**. It does
not scrape desktop credentials, bypass auth, probe providers, call model refresh,
or poll private SQLite/session payloads. A supported independently authorized
read-only config consumer or a narrow upstream usage tool is still required.
`getConfig` was not called; its broader config payload is not exposed to Skywatch.

## Narrow input adapter

`UsageSource` in `server/src/usage.rs` is independent of the thread driver.
`FileUsageSource` is a working local transport for an authorized quota producer:
set `SKYWATCH_USAGE_FILE` to a dedicated JSON file, atomically replaced by that
producer. Polling is every 30 seconds, with a 256 KiB limit, at most 64 accounts
and 64 windows per account. Do not point it at a raw T3 config/credential file.
Absent/invalid input is unavailable; a failed read retains the same accounts and
readings visibly stale. A successful replacement replaces the whole account set.

Example **synthetic** producer payload (timestamps must be source timestamps):

```json
{
  "source_id": "t3-instance-opaque-id",
  "environment_id": "environment-opaque-id",
  "accounts": [{
    "id": "stable-account-opaque-id",
    "provider_id": "codex",
    "checked_at": "2026-11-25T10:00:00Z",
    "unavailable": false,
    "probe_failed": false,
    "windows": [{
      "id": "primary",
      "kind": "session",
      "used_percent": 37,
      "duration_minutes": 300,
      "resets_at": "2026-11-25T12:14:00Z",
      "model_ids": [],
      "provider_ids": []
    }]
  }]
}
```

Producer requirements:

- Use actual source/environment/provider-instance/account identities. Account `id`
  must change when the authenticated account changes. Never merge by email/name.
  For a T3 hub, include the hub source identity in `source_id`; for direct instances,
  include the provider `instanceId`. Do not substitute the instance alone for an
  account identity if it can switch accounts.
- Map T3 `usageLimits.checkedAt` to `checked_at`, never poll time. Map
  `windowDurationMins`, `usedPercent`, `resetsAt`, `modelIds`, `providerIds` directly
  to their snake-case equivalents. Preserve every actual window and scope.
  Map `unsupported` to `unavailable: true`, `probeFailed` to `probe_failed: true`.
  Missing usage remains an account with no windows, not zero/full.
- Use canonical UTC RFC3339 (`Z`, optional fractional seconds). Normalize offsets
  upstream. No timestamps more than 60 seconds in the future are accepted.
- Do not send emails, endpoint URLs, tokens, credential fingerprints, raw errors,
  account display names, or config payloads. The browser DTO uses fixed provider
  names, generated account labels, and scoped hashed keys. Keys prevent accidental
  identity mixing; their non-cryptographic hash is not a credential protection tool.
- Model/provider scope IDs must be actual public identifiers (max 100 ASCII chars,
  alphanumeric or `-_.:/`). Invalid scopes remain scoped as `unrecognized-scope`;
  they are never promoted to a provider-wide tank.

`remaining_percent = 100 - used_percent` only for validated authoritative values.
Missing/out-of-range values and invalid source time yield null. True empty/full
remain 0/100. Ten minutes is Skywatch's stale policy (`STALE_AFTER_SECS`), not a T3
freshness guarantee. A retained failed-probe reading is stale immediately.
An expired reset means **refresh pending**, never refill. The same contract can
be implemented by an OpenCode source when it has authoritative quota data.

## Scene and integration

`web/fuel-depot.js` owns meshes, textures, DOM, countdowns, theme listener and
cleanup. Main wiring is limited to import/create/update/pagehide disposal.
`WorldSnapshot.usage` is additive; the usage poll runs separately from T3 polling.
There are no changes to `adapters/t3.rs`, avoiding driver integration conflicts.

The center is `[160,155]`, pad radius 26 (base radius 28). Home terrain and satellite
islands are clear. Research sector center `[230,245]` is 114 units away with radius
86: the base is tangent to its conceptual circle, while the top pad has 2 units of
clearance. The original proposed radius 34 would overlap. Dynamic project slots
are clear. Recheck against future geometry/sector changes and the taxiway branch.

A maximum of four accounts occupy the island; the panel includes all accounts
and all windows. Up to two unscoped actual windows per account get tanks; scoped
windows stay in details, without averaging. Monthly-only plans get one Monthly
tank. Tank shell uses 16 segments, transparent opacity .25, no transmission,
no shadow casting, `depthWrite:false`. Countdown redraws at most once per minute
unless snapshot data changes. Replaced geometry/materials/textures are disposed.

Theme contract: initial `document.documentElement.dataset.theme`; subscribe to
`skywatch-theme-change` with `detail: {theme}`; listener removed on disposal.
`setTheme` changes the pad for alpine/tropical/winter. Optional
`decorate({theme, THREE, radius})` returns an owned group for vegetation/snow;
old decoration is disposed on theme change. No theme imports or config route
changes are required. Theme agent may supply this hook from its palette.

Keyboard: native Fuel depot button opens text details and focuses Close; Escape
closes and restores focus. All account/window scope, percentage, exact reset and
source freshness information is in DOM text. Fill/color is supplementary.

## Safe demo and verification

`/?fuel-demo=1` opts into explicitly marked synthetic fixtures in the main scene.
`/fuel-demo.html` is a focused synthetic-only screenshot fixture with a theme
selector. `web/fuel-fixtures.js` includes 63/28, empty/full, monthly, stale expired
reset, unknown, unavailable, two Codex accounts and a model-scoped extra window.
Neither route reads account data or emits provider requests.

Run `cargo test --manifest-path server/Cargo.toml`,
`cargo clippy --manifest-path server/Cargo.toml --all-targets`,
`node --test tests/*.test.mjs`, and `node --check` on the changed JS modules.
Strict Clippy (`-D warnings`) is currently blocked by pre-existing warnings in
main/world/T3/remote adapter code; ordinary Clippy passes with those warnings.

Verified in the browser: main-scene unavailable state; focused synthetic fixture;
Enter opens details and focuses Close, Escape closes and restores button focus;
all three theme events; decoration listener cleanup after disposal and exactly one
pad geometry disposal. T3 screenshot capture stopped producing frames, so the
focused captures used the connected Brave browser. Saved captures:

- `docs/screenshots/fuel-depot-demo.png`
- `docs/screenshots/fuel-depot-details-winter.png`
- `docs/screenshots/fuel-depot-unavailable.png`

Final automated evidence: 93 Rust tests and 15 Node tests pass; JS syntax checks
pass; ordinary all-target Clippy passes with existing warnings; Gitleaks reports
no leaks in the usage module, renderer, fixtures and integration document.
