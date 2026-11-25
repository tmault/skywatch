---------------------- MODULE SkywatchAdapterHandoff ----------------------
(*
 * TLA+ specification of the FIXED T3 adapter reconciliation & callsign handoff
 * state machine (server/src/adapters/t3.rs, server/src/world.rs).
 *
 * Fixes verified:
 *   1. `runway.departing` tracks departing callsigns so reopening/unsettling a
 *      thread reclaims its existing airframe instead of spawning a duplicate.
 *   2. `data.stale` shares arrival handoff with `data.taxiway`, so an arriving
 *      SKY aircraft pushed into `stale` finishes landing and renames to GT
 *      instead of spawning a duplicate GT alongside SKY.
 *   3. `apply_poll` calls `a.hold(...)` on entry into Holding and `Cruising`
 *      clears `orbit_center`, eliminating stale-orbit teleports.
 *)
EXTENDS Naturals, FiniteSets

VARIABLES
    t3Pool,
    inCallsigns,
    inArrivals,
    inDeparting,
    inTaxiwayCs,
    inGates,
    primed,
    skyPresent,
    skyState,
    skyHasArrivalSlot,
    skyPosition,
    skyOrbitCenter,
    skyTeleported,
    txPresent,
    gtPresent

vars == <<
    t3Pool, inCallsigns, inArrivals, inDeparting, inTaxiwayCs, inGates, primed,
    skyPresent, skyState, skyHasArrivalSlot, skyPosition, skyOrbitCenter, skyTeleported,
    txPresent, gtPresent
>>

Init ==
    /\ t3Pool            = "FlyingCruise"
    /\ inCallsigns       = TRUE
    /\ inArrivals        = FALSE
    /\ inDeparting       = FALSE
    /\ inTaxiwayCs       = FALSE
    /\ inGates           = FALSE
    /\ primed            = TRUE
    /\ skyPresent        = TRUE
    /\ skyState          = "Cruising"
    /\ skyHasArrivalSlot = FALSE
    /\ skyPosition       = "SectorA"
    /\ skyOrbitCenter    = "None"
    /\ skyTeleported     = FALSE
    /\ txPresent         = FALSE
    /\ gtPresent         = FALSE

T3StateChange(nextPool) ==
    /\ nextPool \in {"FlyingCruise", "FlyingHold", "Taxiway", "Stale", "Done"}
    /\ nextPool # t3Pool
    /\ t3Pool' = nextPool
    /\ UNCHANGED <<inCallsigns, inArrivals, inDeparting, inTaxiwayCs, inGates, primed,
                   skyPresent, skyState, skyHasArrivalSlot, skyPosition,
                   skyOrbitCenter, skyTeleported, txPresent, gtPresent>>

\* World::tick Cruising now clears orbit_center (world.rs)
TickCruiseMove ==
    /\ skyPresent
    /\ skyState = "Cruising"
    /\ skyPosition = "SectorA"
    /\ skyPosition'    = "SectorB"
    /\ skyOrbitCenter' = "None"
    /\ UNCHANGED <<t3Pool, inCallsigns, inArrivals, inDeparting, inTaxiwayCs, inGates, primed,
                   skyPresent, skyState, skyHasArrivalSlot, skyTeleported,
                   txPresent, gtPresent>>

TickHolding ==
    /\ skyPresent
    /\ skyState = "Holding"
    /\ IF skyOrbitCenter = "None" THEN
          /\ skyOrbitCenter' = skyPosition
          /\ UNCHANGED skyTeleported
       ELSE IF skyOrbitCenter # skyPosition THEN
          /\ skyTeleported' = TRUE
          /\ UNCHANGED skyOrbitCenter
       ELSE
          UNCHANGED <<skyOrbitCenter, skyTeleported>>
    /\ UNCHANGED <<t3Pool, inCallsigns, inArrivals, inDeparting, inTaxiwayCs, inGates, primed,
                   skyPresent, skyState, skyHasArrivalSlot, skyPosition,
                   txPresent, gtPresent>>

TickFinishArrival ==
    /\ skyPresent
    /\ skyState = "Landing"
    /\ skyState' = "Ready"
    /\ UNCHANGED <<t3Pool, inCallsigns, inArrivals, inDeparting, inTaxiwayCs, inGates, primed,
                   skyPresent, skyHasArrivalSlot, skyPosition, skyOrbitCenter,
                   skyTeleported, txPresent, gtPresent>>

\* Departing aircraft eventually despawns past horizon
TickDespawnDeparting ==
    /\ skyPresent
    /\ skyState = "Departing"
    /\ skyPresent' = FALSE
    /\ skyState'   = "None"
    /\ UNCHANGED <<t3Pool, inCallsigns, inArrivals, inDeparting, inTaxiwayCs, inGates, primed,
                   skyHasArrivalSlot, skyPosition, skyOrbitCenter, skyTeleported,
                   txPresent, gtPresent>>

\* Fixed apply_poll() (server/src/adapters/t3.rs)
ApplyPoll ==
    LET
        keepFlying     == t3Pool \in {"FlyingCruise", "FlyingHold"}
        targetFlyState == IF t3Pool = "FlyingHold" THEN "Holding" ELSE "Cruising"
        inTaxiPool     == (t3Pool = "Taxiway")
        inStalePool    == (t3Pool = "Stale")
        isDone         == (t3Pool = "Done")

        \* Prune despawned departing callsigns
        depAlive == inDeparting /\ (skyPresent \/ txPresent \/ gtPresent)

        \* 1. Resume mid-arrival
        resumedArr     == inArrivals /\ keepFlying
        arr1           == IF resumedArr THEN FALSE ELSE inArrivals
        cs1            == IF resumedArr /\ skyPresent THEN TRUE ELSE inCallsigns
        skySt1         == IF resumedArr /\ skyPresent /\ skyState = "Landing" THEN "Cruising" ELSE skyState
        skySlot1       == IF resumedArr /\ skyPresent THEN FALSE ELSE skyHasArrivalSlot

        \* 2. Reclaim departing aircraft if reopened into flying
        revivedFly     == depAlive /\ keepFlying
        dep1           == IF revivedFly THEN FALSE ELSE depAlive
        cs2            == IF revivedFly THEN TRUE ELSE cs1
        skyPres1       == IF revivedFly THEN TRUE ELSE skyPresent
        txPres1        == IF revivedFly THEN FALSE ELSE txPresent
        gtPres1        == IF revivedFly THEN FALSE ELSE gtPresent
        skySt2         == IF revivedFly THEN "Cruising" ELSE skySt1

        \* 3. Drop from callsigns if !keepFlying
        droppedSky     == cs2 /\ ~keepFlying
        cs3            == IF droppedSky THEN FALSE ELSE cs2
        beginsApproach == droppedSky /\ primed /\ inTaxiPool /\ skyPres1
                          /\ skySt2 \in {"Cruising", "Holding", "Landing"}
        departsSky     == droppedSky /\ isDone /\ skyPres1
        removedSky     == droppedSky /\ ~departsSky /\ ~beginsApproach

        arr2           == IF beginsApproach THEN TRUE ELSE arr1
        dep2           == IF departsSky THEN TRUE ELSE dep1
        skyPres2       == IF removedSky THEN FALSE ELSE skyPres1
        skySt3         == IF departsSky THEN "Departing"
                          ELSE IF beginsApproach THEN "Landing"
                          ELSE IF removedSky THEN "None"
                          ELSE skySt2
        skySlot2       == IF beginsApproach THEN TRUE
                          ELSE IF departsSky \/ removedSky THEN FALSE
                          ELSE skySlot1

        \* 4. Update existing SKY (or wake from TX/GT, or spawn new SKY)
        inRunwaySeq    == skySt3 = "Landing" \/ (skySt3 = "Ready" /\ skySlot2)
        needsSpawnSky  == keepFlying /\ ~skyPres2
        wokeFromGround == needsSpawnSky /\ (txPres1 \/ gtPres1)
        skyPres3       == IF keepFlying THEN TRUE ELSE skyPres2
        csFinal        == IF keepFlying THEN TRUE ELSE cs3
        txPres2        == IF wokeFromGround THEN FALSE ELSE txPres1
        gtPres2        == IF wokeFromGround THEN FALSE ELSE gtPres1
        skySt4         == IF keepFlying /\ ~inRunwaySeq THEN targetFlyState ELSE skySt3
        \* Fixed: entering Holding via a.hold() resets orbit_center if state changed!
        nextOrbitCenter ==
            IF keepFlying /\ ~inRunwaySeq /\ targetFlyState = "Holding" /\ skySt3 # "Holding"
            THEN "None"
            ELSE skyOrbitCenter

        \* 5. taxi_drop
        txDropped      == inTaxiwayCs /\ ~inTaxiPool
        txCs1          == IF txDropped THEN FALSE ELSE inTaxiwayCs
        txDeparting    == txDropped /\ isDone /\ txPres2
        dep3           == IF txDeparting THEN TRUE ELSE dep2
        txPres3        == IF txDropped /\ ~txDeparting THEN FALSE ELSE txPres2

        \* 6. data.taxiway loop (reclaim departing, hand off arrival, or spawn TX)
        revivedTaxi    == inTaxiPool /\ dep3
        dep4           == IF revivedTaxi THEN FALSE ELSE dep3
        revivedTaxiAir == revivedTaxi /\ skyPres3
        arr3           == IF revivedTaxiAir THEN TRUE ELSE arr2
        skySt5         == IF revivedTaxiAir THEN "Landing" ELSE skySt4
        skySlot3       == IF revivedTaxiAir THEN TRUE ELSE skySlot2
        txPres4        == IF revivedTaxi /\ ~revivedTaxiAir THEN TRUE ELSE txPres3
        gtPres3        == IF revivedTaxi /\ ~revivedTaxiAir THEN FALSE ELSE gtPres2

        arrDoneTaxi    == inTaxiPool /\ arr3 /\ skyPres3 /\ skySt5 = "Ready" /\ skySlot3
        arrInFlyTaxi   == inTaxiPool /\ arr3 /\ skyPres3 /\ ~arrDoneTaxi
        arr4           == IF arrDoneTaxi THEN FALSE ELSE arr3
        txCsFinal      == IF inTaxiPool THEN TRUE ELSE txCs1
        skyPres4       == IF arrDoneTaxi THEN FALSE ELSE skyPres3
        txPresFinal    == IF arrDoneTaxi THEN TRUE
                          ELSE IF inTaxiPool /\ ~arrInFlyTaxi THEN TRUE
                          ELSE txPres4

        \* 7. Orphaned arrivals (only if left BOTH taxiway AND stale!)
        orphaned       == arr4 /\ ~inTaxiPool /\ ~inStalePool
        orphanDepart   == orphaned /\ isDone /\ skyPres4
        orphanRemove   == orphaned /\ ~isDone /\ (~skyPres4 \/ skySt5 = "Ready")
        arr5           == IF orphaned /\ (isDone \/ orphanRemove) THEN FALSE ELSE arr4
        dep5           == IF orphanDepart THEN TRUE ELSE dep4
        skyPres5       == IF orphanRemove THEN FALSE ELSE skyPres4
        skySt6         == IF orphanDepart THEN "Departing"
                          ELSE IF orphanRemove THEN "None"
                          ELSE skySt5
        skySlot4       == IF orphanRemove THEN FALSE ELSE skySlot3

        \* 8. gate_drop
        gtDropped      == inGates /\ ~inStalePool
        gtCs1          == IF gtDropped THEN FALSE ELSE inGates
        gtDeparting    == gtDropped /\ isDone /\ gtPres3
        dep6           == IF gtDeparting THEN TRUE ELSE dep5
        gtPres4        == IF gtDropped /\ ~gtDeparting THEN FALSE ELSE gtPres3

        \* 9. data.stale loop (FIXED: reclaims departing AND hands off arrivals!)
        revivedStale    == inStalePool /\ dep6
        depFinal        == IF revivedStale THEN FALSE ELSE dep6
        revivedStaleAir == revivedStale /\ skyPres5
        arr6            == IF revivedStaleAir THEN TRUE ELSE arr5
        skyStFinal      == IF revivedStaleAir THEN "Landing" ELSE skySt6
        skySlotFinal    == IF revivedStaleAir THEN TRUE ELSE skySlot4
        gtPres5         == IF revivedStale /\ ~revivedStaleAir THEN TRUE ELSE gtPres4
        \* Renaming a departing ground TX into GT clears txPresent
        txPresAfterStale == IF revivedStale /\ ~revivedStaleAir THEN FALSE ELSE txPresFinal

        arrDoneStale    == inStalePool /\ arr6 /\ skyPres5 /\ skyStFinal = "Ready" /\ skySlotFinal
        arrInFlyStale   == inStalePool /\ arr6 /\ skyPres5 /\ ~arrDoneStale
        arrFinal        == IF arrDoneStale THEN FALSE ELSE arr6
        gtCsFinal       == IF inStalePool THEN TRUE ELSE gtCs1
        skyPresFinal    == IF arrDoneStale THEN FALSE ELSE skyPres5
        gtPresFinal     == IF arrDoneStale THEN TRUE
                           ELSE IF inStalePool /\ ~arrInFlyStale THEN TRUE
                           ELSE gtPres5
    IN
        /\ inCallsigns'       = csFinal
        /\ inArrivals'        = arrFinal
        /\ inDeparting'       = depFinal
        /\ inTaxiwayCs'       = txCsFinal
        /\ inGates'           = gtCsFinal
        /\ skyPresent'        = skyPresFinal
        /\ skyState'          = skyStFinal
        /\ skyHasArrivalSlot' = IF arrDoneTaxi \/ arrDoneStale THEN FALSE ELSE skySlotFinal
        /\ skyOrbitCenter'    = nextOrbitCenter
        /\ txPresent'         = txPresAfterStale
        /\ gtPresent'         = gtPresFinal
        /\ UNCHANGED <<t3Pool, primed, skyPosition, skyTeleported>>

Next ==
    \/ \E p \in {"FlyingCruise", "FlyingHold", "Taxiway", "Stale", "Done"}: T3StateChange(p)
    \/ TickCruiseMove
    \/ TickHolding
    \/ TickFinishArrival
    \/ TickDespawnDeparting
    \/ ApplyPoll

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
InvUniqueAircraftPerThread ==
    LET count == (IF skyPresent THEN 1 ELSE 0)
               + (IF txPresent  THEN 1 ELSE 0)
               + (IF gtPresent  THEN 1 ELSE 0)
    IN count <= 1

InvNoOrphanedArrivalDuplicate ==
    ~(inArrivals /\ skyPresent /\ gtPresent)

InvNoHoldingOrbitTeleport ==
    ~skyTeleported

=============================================================================
