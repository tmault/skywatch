# Rounded taxiway evidence

The two top-down PNGs and 16-second MP4 show a Boeing 747 executing a
heading-aware nose-in gate departure reversal (`747 gate 4 departure`) and a
Reaper arrival turn into the home ramp (`Reaper ramp 17 arrival`). Both panels
render the **actual production movement-area meshes** extracted verbatim from
`web/main.js` (including the 12-unit route ribbons and 16-unit Taxiway Alpha
slab—no widened evidence corridor) with synthetic deterministic 20 Hz telemetry
exported by the Rust simulation (`production_ground_trajectories`).

- `approaching-turns-top-down.png`: early reversal / ramp-turn entry (t = 2.0 s).
- `moving-turns-top-down.png`: mid-maneuver translation through both bends (t = 6.0 s).
- `moving-turns.mp4`: playable H.264 recording (8 fps, 16 s) of the moving turns.
- `production-trajectories.json`: deterministic server trajectory fixtures for
  all 52 production routes (10 home gates, 6 Taxiway Alpha queue slots, 4 home
  Reaper ramp stands, 4 satellite gates, 2 satellite Reaper ramp stands; arrival
  and departure each).
- `production-scene.js`: executes `buildHomeAirport` / `buildSatelliteAirport`
  verbatim from `web/main.js`, extracts paved top-surface triangles, and
  computes rotated 747 tyre contact-patch corners (all 18 tyres at production
  `makeJumboMesh` scale `0.345`).
- `assess-clearance.mjs` / `clearance-assessment.json`: planar swept-vertex
  envelope assessment for the production 747 mesh across all home and satellite
  747 routes, including explicit practical limits.
- `replay.html` / `capture.mjs`: deterministic replay and headless capture
  harness; serve the repository root over HTTP to use relative imports of the
  production web modules.

## Regenerating evidence

From the repository root:

```sh
SKYWATCH_PRODUCTION_EVIDENCE="$PWD/docs/evidence/rounded-taxiways/production-trajectories.json" \
  cargo test --manifest-path server/Cargo.toml production_ground_trajectories
node docs/evidence/rounded-taxiways/assess-clearance.mjs
```

To re-render the PNGs and MP4 (requires `python3`, `chromium`, and `ffmpeg`):

```sh
python3 -m http.server 8765 --bind 127.0.0.1 &
node docs/evidence/rounded-taxiways/capture.mjs
ffmpeg -y -framerate 8 -i /tmp/skywatch-turn-frames/%04d.png \
  -c:v libx264 -pix_fmt yuv420p -movflags +faststart \
  docs/evidence/rounded-taxiways/moving-turns.mp4
```

## Clearance summary & practical limits

- **Rotated 747 tyre contact patches:** Every 20 Hz frame across all 20 home
  and satellite 747 routes (gates, Taxiway Alpha queue slots, arrivals,
  departures, reversals, and runway connectors) keeps all 72 tyre contact-patch
  corners (4×4 main bogies + twin nose gear at production scale `0.345`) inside
  both boundaries of the actual production pavement (`tests/production-clearance.test.mjs`).
  Taxiway Alpha was widened from 14 to 16 units (symmetric edge lights at ±8.3)
  and the satellite south threshold received a 28×16 turn pad so rotated nose
  gear stays on pavement during queue and threshold blends.
- **Route / paint agreement:** All 52 driven production routes (including
  heading-aware gate reversals and the fixed 10-unit runway arrival lead-in)
  stay within `< 0.03` units of painted production centerlines.
- **Swept wing & static object assessment (`clearance-assessment.json`):**
  - Production 747 wingspan at scale `0.345`: `22.28` units.
  - Minimum planar terminal gap (sampled every 4th 20 Hz frame): `+0.57` units
    at home gates (against terminal glass `x = 133.5`), `+3.12` units at
    satellite gates (against terminal glass `x = 40.05`).
  - Adjacent parked 747 wing envelopes overlap at both home (`19`-unit gate
    spacing → `+3.28` u overlap) and satellite (`16`-unit gate spacing → `+6.28` u
    overlap) stands.
  - Broad-phase jetbridge wing-envelope candidate frames: `572` across home
    gate departures (decorative bridges are not 3D collision-resolved).
  - Practical limits: sampled planar envelope checks are not a continuous 3D
    swept collision solver, tyres use the rigid airframe pose without steering
    off-tracking dynamics, and wheel-on-pavement proof does not imply wing,
    jetbridge, or adjacent-traffic clearance.
