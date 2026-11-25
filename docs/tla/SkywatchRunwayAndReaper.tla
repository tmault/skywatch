---------------------- MODULE SkywatchRunwayAndReaper ----------------------
(*
 * TLA+ specification of the FIXED implicit state machines in Skywatch:
 *   1. Aircraft runway / ground / flight lifecycle & synchronous World::tick
 *      runway arbitration (server/src/world.rs)
 *   2. Scheduled-job Reaper drone lifecycle & retirement/restoration
 *      (server/src/world/reaper.rs, server/src/adapters/t3/scheduled.rs)
 *   3. Provider tanker spawn & tick state dispatch
 *      (server/src/world.rs, server/src/world/tanker.rs)
 *
 * Mirrors the fixed Rust guards verbatim so TLC verifies all safety invariants
 * hold across the complete reachable state space.
 *)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS
    Aircraft,          \* e.g. {"A1", "A2"}
    ReaperId           \* aircraft id acting as a Reaper drone

ASSUME ReaperId \in Aircraft

States == {
    "Parked", "Taxi", "Ready", "Takeoff",
    "Cruising", "Holding", "Refueling",
    "Landing", "Landed", "Departing", "Despawned"
}

Locations == {
    "GateOrRamp",
    "Taxiway",
    "Threshold",
    "RunwayMid",
    "Airborne"
}

JobTargets == {"Park", "HoldShort", "Fly"}

VARIABLES
    state,
    loc,
    altAboveZero,
    clearedForTakeoff,
    mustClearRunway,
    exitingRunway,
    hasArrivalSlot,
    taxiNext,
    departingForever,
    onFinal,
    shortFinal,
    runwayActive,
    depQueue,
    reaperJobPresent,
    reaperTarget,
    reaperPhase,
    tankerActive,
    tankerState,
    tankerTicked,
    tankerOnElliptical

vars == <<
    state, loc, altAboveZero, clearedForTakeoff, mustClearRunway,
    exitingRunway, hasArrivalSlot, taxiNext, departingForever,
    onFinal, shortFinal,
    runwayActive, depQueue,
    reaperJobPresent, reaperTarget, reaperPhase,
    tankerActive, tankerState, tankerTicked, tankerOnElliptical
>>

-----------------------------------------------------------------------------
InSeq(s, x) == \E i \in 1..Len(s): s[i] = x

InRunwaySequence(a) ==
    \/ state[a] \in {"Takeoff", "Landing", "Landed"}
    \/ (state[a] \in {"Taxi", "Ready"} /\ (clearedForTakeoff[a] \/ hasArrivalSlot[a]))

OccupiesRunway(a) ==
    CASE state[a] = "Takeoff"   -> ~altAboveZero[a]
      [] state[a] = "Landing"   -> onFinal[a] /\ shortFinal[a]
      [] state[a] = "Landed"    -> TRUE
      [] state[a] = "Departing" -> ~altAboveZero[a]
      [] state[a] = "Taxi"      ->
            /\ ~(taxiNext[a] = "Ready"
                 /\ clearedForTakeoff[a]
                 /\ ~mustClearRunway[a]
                 /\ ~hasArrivalSlot[a])
            /\ loc[a] \in {"Threshold", "RunwayMid"}
      [] OTHER -> FALSE

ActivelyUsingRunwaySurface(a) ==
    /\ state[a] # "Despawned"
    /\ ~altAboveZero[a]
    /\ \/ state[a] = "Takeoff"
       \/ state[a] = "Departing"
       \/ state[a] = "Landed"
       \/ (state[a] = "Taxi" /\ loc[a] = "RunwayMid")

LinedUpAtThreshold(a) ==
    /\ state[a] = "Ready"
    /\ clearedForTakeoff[a]
    /\ loc[a] = "Threshold"

RunwayUsers == {a \in Aircraft: state[a] # "Despawned" /\ OccupiesRunway(a)}
ArrivalOnFinal == \E a \in Aircraft: state[a] = "Landing" /\ onFinal[a]
AnyLinedUp == \E a \in Aircraft: LinedUpAtThreshold(a)

PrunedQueue == SelectSeq(depQueue, LAMBDA id:
    clearedForTakeoff[id] /\ state[id] \in {"Taxi", "Ready"})

NextDepQueue ==
    LET joining == {a \in Aircraft:
            clearedForTakeoff[a] /\ state[a] \in {"Taxi", "Ready"} /\ ~InSeq(PrunedQueue, a)}
    IN IF joining = {} THEN PrunedQueue
       ELSE LET pick == CHOOSE x \in joining: TRUE
            IN Append(PrunedQueue, pick)

NextRunwayActive ==
    LET stillGranted ==
            /\ runwayActive # "None"
            /\ ( (state[runwayActive] = "Ready" /\ clearedForTakeoff[runwayActive])
                 \/ runwayActive \in RunwayUsers )
        afterDrop == IF stillGranted THEN runwayActive ELSE "None"
    IN IF afterDrop = "None" /\ ~ArrivalOnFinal /\ RunwayUsers = {} THEN
           IF Len(NextDepQueue) > 0
              /\ state[Head(NextDepQueue)] = "Ready"
              /\ clearedForTakeoff[Head(NextDepQueue)]
           THEN Head(NextDepQueue)
           ELSE "None"
       ELSE IF ArrivalOnFinal /\ afterDrop # "None" THEN
           "None"
       ELSE
           afterDrop

RunwayHoldFor(a) ==
    IF state[a] = "Ready" THEN
        NextRunwayActive # a \/ ArrivalOnFinal \/ (\E x \in RunwayUsers: x # a)
    ELSE FALSE

RunwayOccupiedFor(a) ==
    state[a] = "Landing" /\ ((\E x \in RunwayUsers: x # a) \/ AnyLinedUp)

ReaperAirborne(a) ==
    \/ state[a] \in {"Cruising", "Holding"}
    \/ (state[a] = "Takeoff" /\ altAboveZero[a])

\* Fixed update_job_phase (reaper.rs)
ComputeReaperPhase(st, tNext, exiting, hasArr, tgt, airb) ==
    LET taxiingOut == st = "Taxi" /\ tNext = "Ready" /\ ~exiting /\ ~hasArr
        rollingForTakeoff == st = "Takeoff" /\ ~airb
    IN CASE tgt = "Fly" /\ airb               -> "OnStation"
         [] tgt = "Fly"                       -> "Launching"
         [] rollingForTakeoff                 -> "Launching"
         [] tgt = "HoldShort" /\ st = "Ready" -> "HoldingShort"
         [] tgt = "HoldShort" /\ st = "Parked"-> "Parked"
         [] tgt = "HoldShort" /\ taxiingOut   -> "TaxiOut"
         [] tgt = "HoldShort"                 -> "Returning"
         [] tgt = "Park" /\ st = "Parked"     -> "Parked"
         [] OTHER                             -> "Returning"

-----------------------------------------------------------------------------
Init ==
    /\ state              = [a \in Aircraft |-> "Parked"]
    /\ loc                = [a \in Aircraft |-> "GateOrRamp"]
    /\ altAboveZero       = [a \in Aircraft |-> FALSE]
    /\ clearedForTakeoff  = [a \in Aircraft |-> FALSE]
    /\ mustClearRunway    = [a \in Aircraft |-> FALSE]
    /\ exitingRunway      = [a \in Aircraft |-> FALSE]
    /\ hasArrivalSlot     = [a \in Aircraft |-> FALSE]
    /\ taxiNext           = [a \in Aircraft |-> "None"]
    /\ departingForever   = [a \in Aircraft |-> FALSE]
    /\ onFinal            = [a \in Aircraft |-> FALSE]
    /\ shortFinal         = [a \in Aircraft |-> FALSE]
    /\ runwayActive       = "None"
    /\ depQueue           = <<>>
    /\ reaperJobPresent   = TRUE
    /\ reaperTarget       = "Park"
    /\ reaperPhase        = "Parked"
    /\ tankerActive       = FALSE
    /\ tankerState        = "None"
    /\ tankerTicked       = FALSE
    /\ tankerOnElliptical = FALSE

-----------------------------------------------------------------------------
Hold(a) ==
    /\ a # ReaperId
    /\ state[a] = "Cruising"
    /\ state' = [state EXCEPT ![a] = "Holding"]
    /\ UNCHANGED <<loc, altAboveZero, clearedForTakeoff, mustClearRunway,
                   exitingRunway, hasArrivalSlot, taxiNext, departingForever,
                   onFinal, shortFinal, runwayActive, depQueue,
                   reaperJobPresent, reaperTarget, reaperPhase,
                   tankerActive, tankerState, tankerTicked, tankerOnElliptical>>

\* Fixed Aircraft::release() (world.rs):
\*   - Preserves mustClearRunway when releasing Landed aircraft from RunwayMid
\*   - Never sets clearedForTakeoff on airborne Holding/Refueling release
Release(a) ==
    /\ a # ReaperId
    /\ state[a] \in {"Parked", "Landed", "Taxi", "Ready", "Holding", "Refueling"}
    /\ LET ground == state[a] \in {"Parked", "Landed", "Taxi", "Ready"}
           preserve == state[a] \in {"Landed", "Taxi"} /\ mustClearRunway[a]
       IN
       /\ clearedForTakeoff' = [clearedForTakeoff EXCEPT ![a] = ground]
       /\ hasArrivalSlot'    = [hasArrivalSlot EXCEPT ![a] = FALSE]
       /\ exitingRunway'     = [exitingRunway EXCEPT ![a] = FALSE]
       /\ mustClearRunway'   = [mustClearRunway EXCEPT ![a] =
            IF ground THEN IF loc[a] = "Threshold" THEN FALSE ELSE preserve ELSE @]
       /\ state' = [state EXCEPT ![a] =
            IF ground THEN IF loc[a] = "Threshold" THEN "Ready" ELSE "Taxi"
            ELSE IF state[a] = "Refueling" THEN "Cruising"
            ELSE "Takeoff"]
       /\ taxiNext' = [taxiNext EXCEPT ![a] =
            IF ground /\ loc[a] # "Threshold" THEN "Ready" ELSE @]
    /\ UNCHANGED <<loc, altAboveZero, departingForever, onFinal, shortFinal,
                   runwayActive, depQueue,
                   reaperJobPresent, reaperTarget, reaperPhase,
                   tankerActive, tankerState, tankerTicked, tankerOnElliptical>>

BeginApproach(a) ==
    /\ a # ReaperId
    /\ state[a] \in {"Cruising", "Holding", "Refueling", "Takeoff"}
    /\ ~(state[a] = "Takeoff" /\ ~altAboveZero[a])
    /\ state'             = [state EXCEPT ![a] = "Landing"]
    /\ onFinal'           = [onFinal EXCEPT ![a] = FALSE]
    /\ shortFinal'        = [shortFinal EXCEPT ![a] = FALSE]
    /\ clearedForTakeoff' = [clearedForTakeoff EXCEPT ![a] = FALSE]
    /\ hasArrivalSlot'    = [hasArrivalSlot EXCEPT ![a] = TRUE]
    /\ UNCHANGED <<loc, altAboveZero, mustClearRunway, exitingRunway, taxiNext,
                   departingForever, runwayActive, depQueue,
                   reaperJobPresent, reaperTarget, reaperPhase,
                   tankerActive, tankerState, tankerOnElliptical, tankerTicked>>

GoAround(a) ==
    /\ a # ReaperId
    /\ state[a] \in {"Landing", "Landed", "Taxi", "Ready"}
    /\ hasArrivalSlot[a]
    /\ IF state[a] = "Landing" THEN
          /\ state'             = [state EXCEPT ![a] = "Cruising"]
          /\ hasArrivalSlot'    = [hasArrivalSlot EXCEPT ![a] = FALSE]
          /\ onFinal'           = [onFinal EXCEPT ![a] = FALSE]
          /\ shortFinal'        = [shortFinal EXCEPT ![a] = FALSE]
          /\ UNCHANGED <<clearedForTakeoff, mustClearRunway, exitingRunway, taxiNext>>
       ELSE
          LET preserve == state[a] \in {"Landed", "Taxi"} /\ mustClearRunway[a]
          IN
          /\ hasArrivalSlot'    = [hasArrivalSlot EXCEPT ![a] = FALSE]
          /\ clearedForTakeoff' = [clearedForTakeoff EXCEPT ![a] = TRUE]
          /\ exitingRunway'     = [exitingRunway EXCEPT ![a] = FALSE]
          /\ mustClearRunway'   = [mustClearRunway EXCEPT ![a] =
                IF loc[a] = "Threshold" THEN FALSE ELSE preserve]
          /\ state'             = [state EXCEPT ![a] =
                IF loc[a] = "Threshold" THEN "Ready" ELSE "Taxi"]
          /\ taxiNext'          = [taxiNext EXCEPT ![a] =
                IF loc[a] = "Threshold" THEN @ ELSE "Ready"]
          /\ UNCHANGED <<onFinal, shortFinal>>
    /\ UNCHANGED <<loc, altAboveZero, departingForever, runwayActive, depQueue,
                   reaperJobPresent, reaperTarget, reaperPhase,
                   tankerActive, tankerState, tankerTicked, tankerOnElliptical>>

\* Fixed Aircraft::depart() (world.rs):
\*   Ground farewell departures enter departure_queue as Ready/Taxi with
\*   clearedForTakeoff=TRUE and wait for runway arbitration before rolling!
Depart(a) ==
    /\ a # ReaperId
    /\ ~departingForever[a]
    /\ state[a] \notin {"Departing", "Despawned"}
    /\ departingForever'  = [departingForever EXCEPT ![a] = TRUE]
    /\ hasArrivalSlot'    = [hasArrivalSlot EXCEPT ![a] = FALSE]
    /\ exitingRunway'     = [exitingRunway EXCEPT ![a] = FALSE]
    /\ IF state[a] \in {"Cruising", "Holding", "Refueling", "Landing"}
          \/ (state[a] = "Takeoff" /\ altAboveZero[a]) THEN
          /\ state'             = [state EXCEPT ![a] = "Departing"]
          /\ altAboveZero'      = [altAboveZero EXCEPT ![a] = TRUE]
          /\ clearedForTakeoff' = [clearedForTakeoff EXCEPT ![a] = FALSE]
          /\ taxiNext'          = [taxiNext EXCEPT ![a] = "None"]
          /\ UNCHANGED mustClearRunway
       ELSE IF (state[a] = "Takeoff" /\ ~altAboveZero[a])
               \/ (state[a] = "Landed" /\ loc[a] = "RunwayMid") THEN
          /\ state'             = [state EXCEPT ![a] = "Departing"]
          /\ altAboveZero'      = [altAboveZero EXCEPT ![a] = FALSE]
          /\ clearedForTakeoff' = [clearedForTakeoff EXCEPT ![a] = FALSE]
          /\ mustClearRunway'   = [mustClearRunway EXCEPT ![a] = FALSE]
          /\ taxiNext'          = [taxiNext EXCEPT ![a] = "None"]
       ELSE
          LET preserve == state[a] \in {"Landed", "Taxi"} /\ mustClearRunway[a]
          IN
          /\ clearedForTakeoff' = [clearedForTakeoff EXCEPT ![a] = TRUE]
          /\ mustClearRunway'   = [mustClearRunway EXCEPT ![a] =
                IF loc[a] = "Threshold" THEN FALSE ELSE preserve]
          /\ state'             = [state EXCEPT ![a] =
                IF loc[a] = "Threshold" THEN "Ready" ELSE "Taxi"]
          /\ taxiNext'          = [taxiNext EXCEPT ![a] =
                IF loc[a] = "Threshold" THEN "None" ELSE "Ready"]
          /\ UNCHANGED altAboveZero
    /\ UNCHANGED <<loc, onFinal, shortFinal, runwayActive, depQueue,
                   reaperJobPresent, reaperTarget, reaperPhase,
                   tankerActive, tankerState, tankerTicked, tankerOnElliptical>>

-----------------------------------------------------------------------------
TaxiArrive(a) ==
    /\ state[a] = "Taxi"
    /\ depQueue'        = NextDepQueue
    /\ runwayActive'    = NextRunwayActive
    /\ mustClearRunway' = [mustClearRunway EXCEPT ![a] = FALSE]
    /\ IF taxiNext[a] = "Ready" THEN
          LET dest == IF clearedForTakeoff[a]
                         /\ (Len(NextDepQueue) = 0 \/ Head(NextDepQueue) = a)
                      THEN "Threshold"
                      ELSE "Taxiway"
          IN /\ state'         = [state EXCEPT ![a] = "Ready"]
             /\ loc'           = [loc EXCEPT ![a] = dest]
             /\ taxiNext'      = [taxiNext EXCEPT ![a] = "None"]
             /\ exitingRunway' = [exitingRunway EXCEPT ![a] = FALSE]
             /\ UNCHANGED altAboveZero
       ELSE IF taxiNext[a] = "Parked" THEN
          /\ state'         = [state EXCEPT ![a] = "Parked"]
          /\ loc'           = [loc EXCEPT ![a] = "GateOrRamp"]
          /\ taxiNext'      = [taxiNext EXCEPT ![a] = "None"]
          /\ exitingRunway' = [exitingRunway EXCEPT ![a] = FALSE]
          /\ UNCHANGED altAboveZero
       ELSE FALSE
    /\ reaperPhase' = IF a = ReaperId /\ reaperJobPresent
                      THEN ComputeReaperPhase(state'[a], taxiNext'[a], exitingRunway'[a],
                               hasArrivalSlot[a], reaperTarget,
                               state'[a] \in {"Cruising", "Holding"} \/ (state'[a] = "Takeoff" /\ altAboveZero'[a]))
                      ELSE reaperPhase
    /\ UNCHANGED <<clearedForTakeoff, hasArrivalSlot, departingForever,
                   onFinal, shortFinal, reaperJobPresent, reaperTarget,
                   tankerActive, tankerState, tankerTicked, tankerOnElliptical>>

\* Fixed Ready -> Takeoff or Departing (both governed by ~RunwayHoldFor(a)!)
ReadyToTakeoff(a) ==
    /\ state[a] = "Ready"
    /\ clearedForTakeoff[a]
    /\ ~RunwayHoldFor(a)
    /\ loc[a] = "Threshold"
    /\ depQueue'          = NextDepQueue
    /\ runwayActive'      = NextRunwayActive
    /\ clearedForTakeoff' = [clearedForTakeoff EXCEPT ![a] = FALSE]
    /\ state'             = [state EXCEPT ![a] =
            IF departingForever[a] THEN "Departing" ELSE "Takeoff"]
    /\ loc'               = [loc EXCEPT ![a] = "RunwayMid"]
    /\ altAboveZero'      = [altAboveZero EXCEPT ![a] = FALSE]
    /\ reaperPhase' = IF a = ReaperId /\ reaperJobPresent
                      THEN ComputeReaperPhase(state'[a], taxiNext[a], exitingRunway[a],
                               hasArrivalSlot[a], reaperTarget, FALSE)
                      ELSE reaperPhase
    /\ UNCHANGED <<mustClearRunway, exitingRunway, hasArrivalSlot, taxiNext,
                   departingForever, onFinal, shortFinal,
                   reaperJobPresent, reaperTarget,
                   tankerActive, tankerState, tankerTicked, tankerOnElliptical>>

TakeoffRotate(a) ==
    /\ state[a] = "Takeoff"
    /\ ~altAboveZero[a]
    /\ depQueue'     = NextDepQueue
    /\ runwayActive' = NextRunwayActive
    /\ altAboveZero' = [altAboveZero EXCEPT ![a] = TRUE]
    /\ loc'          = [loc EXCEPT ![a] = "Airborne"]
    /\ reaperPhase'  = IF a = ReaperId /\ reaperJobPresent
                       THEN ComputeReaperPhase("Takeoff", taxiNext[a], exitingRunway[a],
                                hasArrivalSlot[a], reaperTarget, TRUE)
                       ELSE reaperPhase
    /\ UNCHANGED <<state, clearedForTakeoff, mustClearRunway, exitingRunway,
                   hasArrivalSlot, taxiNext, departingForever, onFinal, shortFinal,
                   reaperJobPresent, reaperTarget,
                   tankerActive, tankerState, tankerTicked, tankerOnElliptical>>

TakeoffToCruise(a) ==
    /\ state[a] = "Takeoff"
    /\ altAboveZero[a]
    /\ depQueue'          = NextDepQueue
    /\ runwayActive'      = NextRunwayActive
    /\ clearedForTakeoff' = [clearedForTakeoff EXCEPT ![a] = FALSE]
    /\ state'             = [state EXCEPT ![a] = "Cruising"]
    /\ reaperPhase'       = IF a = ReaperId /\ reaperJobPresent
                            THEN ComputeReaperPhase("Cruising", taxiNext[a], exitingRunway[a],
                                     hasArrivalSlot[a], reaperTarget, TRUE)
                            ELSE reaperPhase
    /\ UNCHANGED <<loc, altAboveZero, mustClearRunway,
                   exitingRunway, hasArrivalSlot, taxiNext, departingForever,
                   onFinal, shortFinal, reaperJobPresent, reaperTarget,
                   tankerActive, tankerState, tankerTicked, tankerOnElliptical>>

LandingCaptureFinal(a) ==
    /\ state[a] = "Landing"
    /\ ~onFinal[a]
    /\ depQueue'     = NextDepQueue
    /\ runwayActive' = NextRunwayActive
    /\ onFinal'      = [onFinal EXCEPT ![a] = TRUE]
    /\ shortFinal'   = [shortFinal EXCEPT ![a] = FALSE]
    /\ UNCHANGED <<state, loc, altAboveZero, clearedForTakeoff, mustClearRunway,
                   exitingRunway, hasArrivalSlot, taxiNext, departingForever,
                   reaperJobPresent, reaperTarget, reaperPhase,
                   tankerActive, tankerState, tankerTicked, tankerOnElliptical>>

LandingEnterShortFinal(a) ==
    /\ state[a] = "Landing"
    /\ onFinal[a]
    /\ ~shortFinal[a]
    /\ depQueue'     = NextDepQueue
    /\ runwayActive' = NextRunwayActive
    /\ IF RunwayOccupiedFor(a) THEN
          /\ onFinal'    = [onFinal EXCEPT ![a] = FALSE]
          /\ shortFinal' = [shortFinal EXCEPT ![a] = FALSE]
       ELSE
          /\ shortFinal' = [shortFinal EXCEPT ![a] = TRUE]
          /\ UNCHANGED onFinal
    /\ UNCHANGED <<state, loc, altAboveZero, clearedForTakeoff, mustClearRunway,
                   exitingRunway, hasArrivalSlot, taxiNext, departingForever,
                   reaperJobPresent, reaperTarget, reaperPhase,
                   tankerActive, tankerState, tankerTicked, tankerOnElliptical>>

LandingTouchdown(a) ==
    /\ state[a] = "Landing"
    /\ onFinal[a]
    /\ shortFinal[a]
    /\ depQueue'     = NextDepQueue
    /\ runwayActive' = NextRunwayActive
    /\ IF ~RunwayOccupiedFor(a) THEN
          /\ state'           = [state EXCEPT ![a] = "Landed"]
          /\ loc'             = [loc EXCEPT ![a] = "RunwayMid"]
          /\ altAboveZero'    = [altAboveZero EXCEPT ![a] = FALSE]
          /\ mustClearRunway' = [mustClearRunway EXCEPT ![a] = TRUE]
          /\ onFinal'         = [onFinal EXCEPT ![a] = FALSE]
          /\ shortFinal'      = [shortFinal EXCEPT ![a] = FALSE]
       ELSE
          /\ onFinal'    = [onFinal EXCEPT ![a] = FALSE]
          /\ shortFinal' = [shortFinal EXCEPT ![a] = FALSE]
          /\ UNCHANGED <<state, loc, altAboveZero, mustClearRunway>>
    /\ UNCHANGED <<clearedForTakeoff, exitingRunway, hasArrivalSlot, taxiNext,
                   departingForever, reaperJobPresent, reaperTarget, reaperPhase,
                   tankerActive, tankerState, tankerTicked, tankerOnElliptical>>

LandedVacateRunway(a) ==
    /\ state[a] = "Landed"
    /\ mustClearRunway[a]
    /\ depQueue'      = NextDepQueue
    /\ runwayActive'  = NextRunwayActive
    /\ state'         = [state EXCEPT ![a] = "Taxi"]
    /\ exitingRunway' = [exitingRunway EXCEPT ![a] = TRUE]
    /\ loc'           = [loc EXCEPT ![a] = "Taxiway"]
    /\ taxiNext'      = [taxiNext EXCEPT ![a] =
            IF a = ReaperId /\ reaperJobPresent THEN "Parked"
            ELSE IF hasArrivalSlot[a] THEN "Ready"
            ELSE "Parked"]
    /\ UNCHANGED <<altAboveZero, clearedForTakeoff, mustClearRunway,
                   hasArrivalSlot, departingForever, onFinal, shortFinal,
                   reaperJobPresent, reaperTarget, reaperPhase,
                   tankerActive, tankerState, tankerTicked, tankerOnElliptical>>

DepartingRotate(a) ==
    /\ state[a] = "Departing"
    /\ ~altAboveZero[a]
    /\ loc[a] = "RunwayMid"
    /\ depQueue'     = NextDepQueue
    /\ runwayActive' = NextRunwayActive
    /\ altAboveZero' = [altAboveZero EXCEPT ![a] = TRUE]
    /\ loc'          = [loc EXCEPT ![a] = "Airborne"]
    /\ UNCHANGED <<state, clearedForTakeoff, mustClearRunway, exitingRunway,
                   hasArrivalSlot, taxiNext, departingForever, onFinal, shortFinal,
                   reaperJobPresent, reaperTarget, reaperPhase,
                   tankerActive, tankerState, tankerTicked, tankerOnElliptical>>

DepartingDespawn(a) ==
    /\ state[a] = "Departing"
    /\ altAboveZero[a]
    /\ depQueue'     = NextDepQueue
    /\ runwayActive' = NextRunwayActive
    /\ state'        = [state EXCEPT ![a] = "Despawned"]
    /\ UNCHANGED <<loc, altAboveZero, clearedForTakeoff, mustClearRunway,
                   exitingRunway, hasArrivalSlot, taxiNext, departingForever,
                   onFinal, shortFinal, reaperJobPresent, reaperTarget, reaperPhase,
                   tankerActive, tankerState, tankerTicked, tankerOnElliptical>>

-----------------------------------------------------------------------------
SteerReaper(tgt) ==
    LET a == ReaperId
    IN
    /\ reaperJobPresent
    /\ state[a] # "Despawned"
    /\ reaperTarget' = tgt
    /\ IF tgt = "Fly" THEN
          IF state[a] = "Parked"
             \/ (state[a] \in {"Ready", "Taxi"}
                 /\ ~clearedForTakeoff[a] /\ ~exitingRunway[a] /\ ~hasArrivalSlot[a])
             \/ (state[a] = "Taxi" /\ exitingRunway[a]) THEN
             LET preserve == state[a] \in {"Landed", "Taxi"} /\ mustClearRunway[a]
             IN
             /\ clearedForTakeoff' = [clearedForTakeoff EXCEPT ![a] = TRUE]
             /\ hasArrivalSlot'    = [hasArrivalSlot EXCEPT ![a] = FALSE]
             /\ exitingRunway'     = [exitingRunway EXCEPT ![a] = FALSE]
             /\ mustClearRunway'   = [mustClearRunway EXCEPT ![a] =
                    IF loc[a] = "Threshold" THEN FALSE ELSE preserve]
             /\ state'             = [state EXCEPT ![a] =
                    IF loc[a] = "Threshold" THEN "Ready" ELSE "Taxi"]
             /\ taxiNext'          = [taxiNext EXCEPT ![a] =
                    IF loc[a] = "Threshold" THEN @ ELSE "Ready"]
             /\ UNCHANGED <<onFinal, shortFinal>>
          ELSE IF state[a] \in {"Landing", "Landed"} THEN
             IF state[a] = "Landing" THEN
                /\ state'          = [state EXCEPT ![a] = "Cruising"]
                /\ hasArrivalSlot' = [hasArrivalSlot EXCEPT ![a] = FALSE]
                /\ onFinal'        = [onFinal EXCEPT ![a] = FALSE]
                /\ shortFinal'     = [shortFinal EXCEPT ![a] = FALSE]
                /\ UNCHANGED <<clearedForTakeoff, mustClearRunway, exitingRunway, taxiNext>>
             ELSE
                LET preserve == mustClearRunway[a]
                IN
                /\ hasArrivalSlot'    = [hasArrivalSlot EXCEPT ![a] = FALSE]
                /\ clearedForTakeoff' = [clearedForTakeoff EXCEPT ![a] = TRUE]
                /\ exitingRunway'     = [exitingRunway EXCEPT ![a] = FALSE]
                /\ mustClearRunway'   = [mustClearRunway EXCEPT ![a] =
                       IF loc[a] = "Threshold" THEN FALSE ELSE preserve]
                /\ state'             = [state EXCEPT ![a] =
                       IF loc[a] = "Threshold" THEN "Ready" ELSE "Taxi"]
                /\ taxiNext'          = [taxiNext EXCEPT ![a] =
                       IF loc[a] = "Threshold" THEN @ ELSE "Ready"]
                /\ UNCHANGED <<onFinal, shortFinal>>
          ELSE
             UNCHANGED <<state, clearedForTakeoff, mustClearRunway, exitingRunway,
                         hasArrivalSlot, taxiNext, onFinal, shortFinal>>
       ELSE IF tgt = "HoldShort" THEN
          IF state[a] = "Parked" THEN
             /\ exitingRunway'     = [exitingRunway EXCEPT ![a] = FALSE]
             /\ clearedForTakeoff' = [clearedForTakeoff EXCEPT ![a] = FALSE]
             /\ hasArrivalSlot'    = [hasArrivalSlot EXCEPT ![a] = FALSE]
             /\ state'             = [state EXCEPT ![a] = "Taxi"]
             /\ taxiNext'          = [taxiNext EXCEPT ![a] = "Ready"]
             /\ UNCHANGED <<mustClearRunway, onFinal, shortFinal>>
          ELSE
             UNCHANGED <<state, clearedForTakeoff, mustClearRunway, exitingRunway,
                         hasArrivalSlot, taxiNext, onFinal, shortFinal>>
       ELSE \* Fixed tgt = "Park": aborts clearedForTakeoff while Taxi/Ready!
          IF ReaperAirborne(a) THEN
             /\ state'             = [state EXCEPT ![a] = "Landing"]
             /\ onFinal'           = [onFinal EXCEPT ![a] = FALSE]
             /\ shortFinal'        = [shortFinal EXCEPT ![a] = FALSE]
             /\ clearedForTakeoff' = [clearedForTakeoff EXCEPT ![a] = FALSE]
             /\ hasArrivalSlot'    = [hasArrivalSlot EXCEPT ![a] = TRUE]
             /\ UNCHANGED <<mustClearRunway, exitingRunway, taxiNext>>
          ELSE IF state[a] \in {"Ready", "Taxi"}
                  /\ ~exitingRunway[a] /\ ~hasArrivalSlot[a] THEN
             /\ exitingRunway'     = [exitingRunway EXCEPT ![a] = FALSE]
             /\ clearedForTakeoff' = [clearedForTakeoff EXCEPT ![a] = FALSE]
             /\ hasArrivalSlot'    = [hasArrivalSlot EXCEPT ![a] = FALSE]
             /\ state'             = [state EXCEPT ![a] = "Taxi"]
             /\ taxiNext'          = [taxiNext EXCEPT ![a] = "Parked"]
             /\ UNCHANGED <<mustClearRunway, onFinal, shortFinal>>
          ELSE
             UNCHANGED <<state, clearedForTakeoff, mustClearRunway, exitingRunway,
                         hasArrivalSlot, taxiNext, onFinal, shortFinal>>
    /\ reaperPhase' = ComputeReaperPhase(state'[a], taxiNext'[a], exitingRunway'[a],
                          hasArrivalSlot'[a], tgt,
                          state'[a] \in {"Cruising", "Holding"} \/ (state'[a] = "Takeoff" /\ altAboveZero[a]))
    /\ UNCHANGED <<loc, altAboveZero, departingForever, runwayActive, depQueue,
                   reaperJobPresent, tankerActive, tankerState, tankerTicked, tankerOnElliptical>>

RetireReaper ==
    LET a == ReaperId
    IN
    /\ reaperJobPresent
    /\ state[a] # "Despawned"
    /\ (altAboveZero[a] \/ InRunwaySequence(a))
    /\ reaperJobPresent'  = FALSE
    /\ departingForever'  = [departingForever EXCEPT ![a] = TRUE]
    /\ hasArrivalSlot'    = [hasArrivalSlot EXCEPT ![a] = FALSE]
    /\ exitingRunway'     = [exitingRunway EXCEPT ![a] = FALSE]
    /\ IF state[a] \in {"Cruising", "Holding", "Refueling", "Landing"}
          \/ (state[a] = "Takeoff" /\ altAboveZero[a]) THEN
          /\ state'             = [state EXCEPT ![a] = "Departing"]
          /\ altAboveZero'      = [altAboveZero EXCEPT ![a] = TRUE]
          /\ clearedForTakeoff' = [clearedForTakeoff EXCEPT ![a] = FALSE]
          /\ taxiNext'          = [taxiNext EXCEPT ![a] = "None"]
          /\ UNCHANGED mustClearRunway
       ELSE IF (state[a] = "Takeoff" /\ ~altAboveZero[a])
               \/ (state[a] = "Landed" /\ loc[a] = "RunwayMid") THEN
          /\ state'             = [state EXCEPT ![a] = "Departing"]
          /\ altAboveZero'      = [altAboveZero EXCEPT ![a] = FALSE]
          /\ clearedForTakeoff' = [clearedForTakeoff EXCEPT ![a] = FALSE]
          /\ mustClearRunway'   = [mustClearRunway EXCEPT ![a] = FALSE]
          /\ taxiNext'          = [taxiNext EXCEPT ![a] = "None"]
       ELSE
          LET preserve == state[a] \in {"Landed", "Taxi"} /\ mustClearRunway[a]
          IN
          /\ clearedForTakeoff' = [clearedForTakeoff EXCEPT ![a] = TRUE]
          /\ mustClearRunway'   = [mustClearRunway EXCEPT ![a] =
                IF loc[a] = "Threshold" THEN FALSE ELSE preserve]
          /\ state'             = [state EXCEPT ![a] =
                IF loc[a] = "Threshold" THEN "Ready" ELSE "Taxi"]
          /\ taxiNext'          = [taxiNext EXCEPT ![a] =
                IF loc[a] = "Threshold" THEN "None" ELSE "Ready"]
          /\ UNCHANGED altAboveZero
    /\ UNCHANGED <<loc, onFinal, shortFinal, runwayActive, depQueue,
                   reaperTarget, reaperPhase,
                   tankerActive, tankerState, tankerTicked, tankerOnElliptical>>

\* Fixed restore_reaper (reaper.rs): aborts ground departure taxi/clearance!
RestoreReaper ==
    LET a == ReaperId
    IN
    /\ ~reaperJobPresent
    /\ state[a] # "Despawned"
    /\ reaperJobPresent' = TRUE
    /\ departingForever' = [departingForever EXCEPT ![a] = FALSE]
    /\ IF state[a] = "Departing" THEN
          /\ clearedForTakeoff' = [clearedForTakeoff EXCEPT ![a] = FALSE]
          /\ state'             = [state EXCEPT ![a] =
                IF altAboveZero[a] THEN "Cruising" ELSE "Takeoff"]
          /\ UNCHANGED <<exitingRunway, hasArrivalSlot, taxiNext>>
       ELSE IF state[a] \in {"Taxi", "Ready"}
               /\ (taxiNext[a] = "Departing" \/ clearedForTakeoff[a]) THEN
          /\ exitingRunway'     = [exitingRunway EXCEPT ![a] = FALSE]
          /\ clearedForTakeoff' = [clearedForTakeoff EXCEPT ![a] = FALSE]
          /\ hasArrivalSlot'    = [hasArrivalSlot EXCEPT ![a] = FALSE]
          /\ state'             = [state EXCEPT ![a] = "Taxi"]
          /\ taxiNext'          = [taxiNext EXCEPT ![a] = "Parked"]
       ELSE
          UNCHANGED <<state, clearedForTakeoff, exitingRunway, hasArrivalSlot, taxiNext>>
    /\ reaperPhase' = ComputeReaperPhase(state'[a], taxiNext'[a], exitingRunway'[a],
                          hasArrivalSlot'[a], reaperTarget,
                          state'[a] \in {"Cruising", "Holding"} \/ (state'[a] = "Takeoff" /\ altAboveZero[a]))
    /\ UNCHANGED <<loc, altAboveZero, mustClearRunway, onFinal, shortFinal,
                   runwayActive, depQueue, reaperTarget,
                   tankerActive, tankerState, tankerTicked, tankerOnElliptical>>

-----------------------------------------------------------------------------
\* Fixed TickTanker (world.rs:1451): matches Cruising | Refueling!
SpawnTanker ==
    /\ ~tankerActive
    /\ tankerActive'       = TRUE
    /\ tankerState'        = "Refueling"
    /\ tankerTicked'       = FALSE
    /\ tankerOnElliptical' = FALSE
    /\ UNCHANGED <<state, loc, altAboveZero, clearedForTakeoff, mustClearRunway,
                   exitingRunway, hasArrivalSlot, taxiNext, departingForever,
                   onFinal, shortFinal, runwayActive, depQueue,
                   reaperJobPresent, reaperTarget, reaperPhase>>

TickTanker ==
    /\ tankerActive
    /\ tankerTicked'       = TRUE
    /\ tankerOnElliptical' = (tankerState \in {"Cruising", "Refueling"})
    /\ UNCHANGED <<state, loc, altAboveZero, clearedForTakeoff, mustClearRunway,
                   exitingRunway, hasArrivalSlot, taxiNext, departingForever,
                   onFinal, shortFinal, runwayActive, depQueue,
                   reaperJobPresent, reaperTarget, reaperPhase,
                   tankerActive, tankerState>>

-----------------------------------------------------------------------------
Next ==
    \/ \E a \in Aircraft:
           \/ Hold(a)
           \/ Release(a)
           \/ BeginApproach(a)
           \/ GoAround(a)
           \/ Depart(a)
           \/ TaxiArrive(a)
           \/ ReadyToTakeoff(a)
           \/ TakeoffRotate(a)
           \/ TakeoffToCruise(a)
           \/ LandingCaptureFinal(a)
           \/ LandingEnterShortFinal(a)
           \/ LandingTouchdown(a)
           \/ LandedVacateRunway(a)
           \/ DepartingRotate(a)
           \/ DepartingDespawn(a)
    \/ \E tgt \in JobTargets: SteerReaper(tgt)
    \/ RetireReaper
    \/ RestoreReaper
    \/ SpawnTanker
    \/ TickTanker

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
InvRunwayMutualExclusion ==
    \A a, b \in Aircraft:
        (a # b /\ ActivelyUsingRunwaySurface(a)) => ~ActivelyUsingRunwaySurface(b)

InvBacktrackReportedAsOccupying ==
    \A a \in Aircraft:
        (state[a] = "Taxi" /\ loc[a] = "RunwayMid") => OccupiesRunway(a)

InvActiveReaperNeverDeparts ==
    reaperJobPresent => state[ReaperId] \notin {"Departing", "Despawned"}

InvReaperReturningNotTakingOff ==
    (reaperJobPresent /\ reaperPhase = "Returning") =>
        ~(state[ReaperId] = "Takeoff" /\ ~altAboveZero[ReaperId])

InvNoStaleTakeoffClearanceAloft ==
    \A a \in Aircraft:
        state[a] = "Cruising" => ~clearedForTakeoff[a]

InvTankerFliesEllipticalTrack ==
    tankerTicked => tankerOnElliptical

=============================================================================
