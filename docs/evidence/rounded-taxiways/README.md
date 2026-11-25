# Rounded taxiway evidence

The two top-down PNGs and 14-second MP4 show a Boeing 747 apron turn and a
Reaper return loop. The replay uses production aircraft meshes and painted
route geometry with positions, heading and speed exported by deterministic
Rust trajectory tests at 20 Hz. All labels and aircraft are synthetic. The
browser also loaded the ordinary standalone demo client without console errors.

- `moving-turns-top-down.png`: both aircraft translating through bends.
- `approaching-turns-top-down.png`: aircraft approaching the bends.
- `moving-turns.mp4`: playable H.264 recording of moving turns.
- `trajectories.json`: deterministic server trajectory fixtures.
- `replay.html`: replay source; serve the repository root over HTTP to use its
  relative imports of the production web modules.

Regenerate the trajectory fixtures from the repository root:

```sh
SKYWATCH_TRAJECTORY_EVIDENCE="$PWD/docs/evidence/rounded-taxiways/trajectories.json" \
  cargo test --manifest-path server/Cargo.toml rounded_routes_cover -- --nocapture
```

Validation: 93 Rust tests, 14 Node tests, all web JavaScript syntax checks,
ground-route module import, and `git diff --check` pass. `cargo clippy
--all-targets` completes with 11 existing warnings (six shared with the binary);
strict `-D warnings` is blocked by those existing adapter/test/style warnings.

TDD failures observed before implementing the fixes included a 1.5707964 radian
departure heading snap, missing runway end alignment, a 0.19308054 radian yaw on
a 10 cm route, a stationary heading snap on an empty route, and stopping at the
arrival exit instead of rolling into its curve.

The 747 test checks the scaled main gear envelope on the broad apron. The
simulation does not provide a full swept-wing collision solver. Tight nose-in
stand reversals use slow moving alignment curves, so taxi duration can exceed
old stop-and-pivot timing; demo timing assertions allow for these turns.
