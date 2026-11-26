# Skywatch TLA+ State Machine Models

Formal TLA+ specifications of the implicit state machines in `server/src/world.rs`, `server/src/world/reaper.rs`, `server/src/world/tanker.rs`, `server/src/adapters/t3.rs`, and `server/src/adapters/t3/scheduled.rs`.

## Modules

- [SkywatchRunwayAndReaper.tla](SkywatchRunwayAndReaper.tla) / [SkywatchRunwayAndReaper.cfg](SkywatchRunwayAndReaper.cfg):
  Models synchronous `World::tick` runway arbitration (`runway_active`, `departure_queue`, `occupies_runway`, `lined_up_by_ap`), aircraft lifecycle transitions (`release`, `begin_approach`, `go_around`, `depart`, `hold`), Reaper scheduled-job steering/retirement/restoration, and tanker state dispatch.
- [SkywatchAdapterHandoff.tla](SkywatchAdapterHandoff.tla) / [SkywatchAdapterHandoff.cfg](SkywatchAdapterHandoff.cfg):
  Models T3 adapter pool reconciliation (`callsigns`, `runway.arrivals`, `taxiway_cs`, `gates`) and `Aircraft.orbit_center` persistence across `Holding -> Cruising -> Holding`.

## Running TLC

With `tla2tools.jar` and Java 21 (or `eclipse-temurin:21-jre-alpine` via Docker):

```bash
java -XX:+UseParallelGC -cp tla2tools.jar tlc2.TLC \
  -workers 2 -deadlock -config SkywatchRunwayAndReaper.cfg SkywatchRunwayAndReaper.tla

java -XX:+UseParallelGC -cp tla2tools.jar tlc2.TLC \
  -workers 2 -deadlock -config SkywatchAdapterHandoff.cfg SkywatchAdapterHandoff.tla
```
