# Scheduled jobs

Skywatch visualises T3 scheduled tasks read-only on Windows, macOS and Linux.
The local adapter reads an explicit summary-column allowlist from T3's SQLite
projection with bundled SQLite in read-only mode. Linked environments use the
existing Python MCP bridge: enumerate projects, then `list_scheduled_tasks` for
each project with its owning `environmentId`. The bridge strips prompts and
webhook metadata before returning summaries. A failed or incomplete listing
keeps the previous list; a successful empty list removes jobs. Remote fields
not supplied by T3 (such as last-run time or count) are shown as unknown/omitted.
No task prompts, webhook tokens, signatures, filters, errors or workspace
settings enter snapshots. Titles are displayed locally; screenshots must use demo mode.

Environment and task ID identify each job; its REAP callsign survives polling,
removal/reappearance and reconnects for the life of the server. Home has four
ramp slots and satellites two each. Running jobs take priority, then existing
visible jobs, then enabled jobs with the soonest run; overflow is counted in the
HUD. Callsign allocations are held in memory, not persisted across server restarts.

Idle/disabled jobs park away from terminal gates. Within two minutes of a run,
one job per airport taxis to hold short. Only a reported running task launches,
using the ordinary runway queue and occupancy rules. It loiters at drone height
in its project sector, then uses the ordinary approach, far-end exit and ramp
return route. Jobs already airborne stay on station between closely spaced runs.
Failed results raise a FAILED alert without publishing private error details.
Radio shows schedule, countdown, last result/time and count when available;
Open in T3 targets the bound thread on its owning device. Radio messages and
approvals cannot execute or modify scheduled jobs.

`JobSource` and `OsJob` define a future optional read-only OS scheduler adapter.
The current opt-in stub returns no jobs. Linux could implement crontab/systemd
timers, macOS launchd, and Windows Task Scheduler (`schtasks` or native APIs).
Such adapters must expose only job names, timing and result: never command
lines, arguments or environment variables. They must not assume a POSIX shell
on the common code path. No OS scheduler is queried today.
