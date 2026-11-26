# OpenCode v2 driver

Skywatch supports Anomaly OpenCode **2.4.1**, using its pinned `/api/...` HTTP
protocol. Select a backend at server startup; changing it requires a restart.

```sh
SKYWATCH_DRIVER=opencode
SKYWATCH_OPENCODE_URL=http://127.0.0.1:49374
SKYWATCH_OPENCODE_PASSWORD_FILE=/run/secrets/opencode-password
# Optional links: configure both, using addresses reachable by the browser.
SKYWATCH_OPENCODE_WEB_URL=https://opencode.example.com
SKYWATCH_OPENCODE_PUBLIC_SERVER_URL=https://opencode-api.example.com
```

The transport URL is required and has no assumed port. The password file holds
only the OpenCode server password, optionally followed by a newline. HTTP Basic
uses username `opencode`. Mount it read-only; do not copy provider credentials,
`service.json`, pairing invitations or tokens into Skywatch configuration. An
unauthenticated local server can omit the password file. No password is returned
to the browser or included in diagnostics. Skywatch does not modify OpenCode's
configuration or start its service.

URLs must be origins without credentials, paths, query strings or fragments.
Remote origins require HTTPS; HTTP is allowed only for localhost, 127.0.0.1 and
::1. There is no insecure remote exception. Redirects are rejected, preventing
credential forwarding. Requests have connection/response deadlines and bounded
response sizes. Public origins are independent of the internal transport origin.
Web links use OpenCode's URL-safe base64 server key and existing-session route.

`SKYWATCH_DRIVER=t3` explicitly selects T3. An unset driver preserves the legacy
behavior: T3 is enabled unless `SKYWATCH_T3=0` or `false`. With T3 disabled,
`SKYWATCH_DEMO` still selects the demo or empty world. An explicit driver overrides
`SKYWATCH_T3`; any value other than `t3` or `opencode` is a startup error.

## Supported behavior

The normalized session/device/job seam feeds the existing fleet reconciliation:
sticky callsigns, model classification/747s, 24-hour parking, taxiway, runway
operations, parent escorts, provider tankers and read-only scheduled Reapers.
OpenCode acquisition does not use T3's SQLite database, CLI or bridge.

- Session pages are followed to completion, including a separate archived list.
  Active state comes from `/api/session/active`; absence means idle, not completed.
  Failed and interrupted outcomes remain distinct. Projects name sectors.
- Only `parentID` creates escorts. Fork metadata does not. Ancestors are followed
  with cycle/missing-parent checks. Completed children linger for the configured
  `SKYWATCH_SUBAGENT_LINGER_SECS` interval while their parent is visible.
- Radio messages use the session prompt endpoint. Approval/rejection rechecks the
  current permission's session ownership and request ID. Approval sends `once`,
  never `always` or an approval text prompt. Pending forms require OpenCode's UI;
  no approval button is offered for a form alone. Command acceptance/failure is
  reported in radio history; HTTP errors never release an aircraft optimistically.
- An active session's latest assistant retry with a future deadline and structured
  HTTP 429 error can use a tanker, isolated per session. Message prose is not used
  to detect quotas. A deadline is a retry time, **not a provider quota reset**;
  the quota-reset field remains empty. Terminal quota failures do not imply
  automatic resumption. Provider quotas without a structured 429 are not inferred.
- Native automations expose allowlisted name, project, schedule, timing, enabled
  state and outcome/count metadata. Actions, prompts, code, shell commands, event
  watch arguments, state and output are discarded. `fired` remains `fired`;
  it does not become running or successful. No automation can be run or edited
  from Skywatch.
- Incomplete session/active/project/permission/form/message refreshes retain the
  last world, mark it stale, and disable commands/links. Failed automation reads
  retain Reapers marked stale. Failed machine reads retain airports as offline.
  A successful empty list is authoritative. Session refreshes have a 25-second bound, followed by at most 8 seconds for jobs;
  individual requests time out after 8 seconds. Pagination is capped at 100 pages
  of 100 sessions per archived/live list; exceeding it preserves stale state.

## Precise parity limits

- Experimental machine discovery displays paired machine reachability only.
  The pinned contract does not prove remote session enumeration/action forwarding;
  all displayed sessions belong to the configured server, never a machine guessed
  from a directory. Unavailable experimental machine APIs are tolerated.
- Existing-session desktop links are unverified and hidden. Web links need both
  public origins and browser access/authentication to that OpenCode server.
- OpenCode has no T3-equivalent PR watcher/background task projection here. Forms
  and transcript browsing stay in OpenCode. T3 retains its existing text-based
  approval routing; its adapter is otherwise unchanged.
- Automation API metadata has no running flag or exact dispatched-run attribution.
  Reapers visualize the schedule and recorded outcomes; they do not assert that a
  dispatched agent is running. OS schedulers remain the existing future seam.
- Child fighters retain the shared escort formation behavior. A child's own
  permission remains actionable through its session ID; independent child retry
  tanker flight is not represented while it is an escort.
- Polling is bounded and serialized with radio processing; a refresh can delay a
  command up to 33 seconds for a refresh. No durable SSE replay or remote forwarding
  is assumed. Commands are not automatically retried after uncertain responses.

## Verification

Read-only connectivity was checked against the installed `opencode v2.4.1` on
2026-11-25: authenticated session, active-session, project, permission, form,
message and automation GETs succeeded. The experimental machine endpoint was
unavailable. No real prompt, permission reply or automation was launched.
Fixture HTTP tests cover commands, 204 replies, redirects, errors, pagination,
partial polling, hierarchy, URL/config validation, ownership and redaction.

Pinned protocol: https://github.com/anomalyco/opencode/blob/v2.4.1/packages/protocol/openapi.json
Route/encoding: `packages/app/src/shell/routes/session.ts`, `packages/util/src/encode.ts`
in the same tag. Upgrading OpenCode requires rechecking these contracts.
