# Environment themes

Approved scope: Alpine (default), Tropical and Swiss winter. Set `SKYWATCH_THEME`
for the server default. Invalid values stop boot. `/config.json` exposes only the
validated theme string, never environment variables or traffic metadata. Older
servers without the endpoint fall back to Alpine. Browser choice in
`localStorage['skywatch.theme']` overrides the server; denied storage still allows
session switching.

Click SKYWATCH for a native modal dialog. Native radio keys, Tab focus containment,
Escape and Done work without touching flight controls or the radio. Terrain
positions, airport identities, runways, taxiways and aircraft are retained. Only
owned environment geometry/materials are rebuilt/disposed. No animation loop is
created by the theme system.

## Extension hook (fuel depot and other scenery)

`document.documentElement.dataset.theme` is set before dispatching the window
`skywatch-theme-change` CustomEvent, whose `detail` is `{ theme }`. Read the dataset
when mounting and subscribe to this event; remove the subscription on disposal.
`THEME_PALETTES` in `web/environment.js` provides matching colors. Modules own and
must dispose their replaced meshes/materials/textures. `createEnvironment().island`
registers a terrain spec with automatic theme rebuilding and returns `heightAt`
and `dispose`. It never owns the parent group's airport structures or flags.

Winter uses a continuous snow plain between the islands and four deterministic
farm clusters outside the airport envelopes. Tropical retains terrain geometry,
using sand, palms and lower green backdrop ridges. Alpine retains Swiss scenery.

Verification: 94 Rust tests, 19 Node tests, JavaScript syntax checks. Normal Clippy
passes with existing warnings; `-D warnings` fails on pre-existing adapter/world
and boot boolean style warnings. Demo verification uses `SKYWATCH_T3=0`, never
live thread titles.
