use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::{broadcast, mpsc, Mutex};

use crate::world::{State as AircraftState, World, WorldSnapshot};

/// Shared world state behind an async mutex.
pub type SharedWorld = Arc<Mutex<World>>;

/// Channel that carries client frames to the adapters (future task).
pub type AdapterTx = mpsc::Sender<ClientFrame>;

/// A client→server frame, parsed and ready for the adapter layer.
#[derive(Debug, Clone)]
pub enum ClientFrame {
    /// Chat message from a pilot/agent.
    Message { aircraft_id: u64, text: String },
    /// ATC approval to leave holding.
    Approve { aircraft_id: u64 },
}

/// Wire format of client→server frames.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMsg {
    Message {
        aircraft_id: u64,
        #[serde(default)]
        text: String,
    },
    Approve {
        aircraft_id: u64,
    },
}

/// State shared with all WS connections.
#[derive(Clone)]
pub struct HubState {
    pub world: SharedWorld,
    /// Broadcast channel of delta frames; the sim ticker publishes here.
    pub deltas: broadcast::Sender<String>,
    /// mpsc channel consumed by the adapter layer (Task 4).
    pub adapter_tx: AdapterTx,
}

fn snapshot_frame(snapshot: &WorldSnapshot) -> String {
    serde_json::json!({ "type": "snapshot", "snapshot": snapshot }).to_string()
}

/// Public so the sim ticker in main.rs can build delta frames.
pub fn delta_json(snapshot: &WorldSnapshot) -> String {
    serde_json::json!({ "type": "delta", "snapshot": snapshot }).to_string()
}

/// Handle a parsed client frame: log it, apply immediate world effects,
/// forward it to the adapter channel.
async fn handle_frame(state: &HubState, frame: ClientFrame) {
    match &frame {
        ClientFrame::Message { aircraft_id, text } => {
            tracing::info!(aircraft_id, text, "client message");
        }
        ClientFrame::Approve { aircraft_id } => {
            tracing::info!(aircraft_id, "client approve");
            // Interactive demo effect: release a holding aircraft immediately.
            let mut world = state.world.lock().await;
            if let Some(a) = world
                .aircraft
                .iter_mut()
                .find(|a| a.id == *aircraft_id && a.state == AircraftState::Holding)
            {
                tracing::info!(aircraft_id, callsign = %a.callsign, "releasing holding aircraft");
                a.release();
            }
        }
    }
    if state.adapter_tx.send(frame).await.is_err() {
        tracing::warn!("adapter channel closed; frame dropped");
    }
}

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<HubState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: HubState) {
    let (mut tx, mut rx) = socket.split();
    tracing::info!("ws client connected");

    // First frame: full snapshot of the current world.
    let snap = state.world.lock().await.snapshot();
    if tx.send(Message::Text(snapshot_frame(&snap))).await.is_err() {
        return;
    }

    let mut delta_rx = state.deltas.subscribe();

    loop {
        tokio::select! {
            msg = rx.next() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    _ => break,
                };
                let text = match msg {
                    Message::Text(t) => t,
                    Message::Close(_) => break,
                    _ => continue,
                };
                match serde_json::from_str::<ClientMsg>(&text) {
                    Ok(ClientMsg::Message { aircraft_id, text }) => {
                        handle_frame(&state, ClientFrame::Message { aircraft_id, text }).await;
                    }
                    Ok(ClientMsg::Approve { aircraft_id }) => {
                        handle_frame(&state, ClientFrame::Approve { aircraft_id }).await;
                    }
                    Err(e) => {
                        tracing::debug!(%text, error = %e, "ignoring unknown client frame");
                    }
                }
            }
            delta = delta_rx.recv() => {
                match delta {
                    Ok(frame) => {
                        if tx.send(Message::Text(frame)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(lagged = n, "ws client lagging behind deltas");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    tracing::info!("ws client disconnected");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Aircraft, Kind};

    #[tokio::test]
    async fn approve_releases_holding_aircraft_and_forwards_frame() {
        let world: SharedWorld = Arc::new(Mutex::new(World::new()));
        let mut a = Aircraft::new("SKY1", Kind::Agent);
        a.hold("awaiting approval");
        let id = a.id;
        world.lock().await.add(a);

        let (deltas, _dr) = broadcast::channel(64);
        let (adapter_tx, mut adapter_rx) = mpsc::channel(64);
        let state = HubState {
            world: Arc::clone(&world),
            deltas,
            adapter_tx,
        };

        handle_frame(&state, ClientFrame::Approve { aircraft_id: id }).await;

        let world = state.world.lock().await;
        let a = world.aircraft.iter().find(|a| a.id == id).unwrap();
        assert!(matches!(a.state, crate::world::State::Takeoff));
        assert!(a.hold_reason.is_none());
        drop(world);

        match adapter_rx.recv().await.unwrap() {
            ClientFrame::Approve { aircraft_id } => assert_eq!(aircraft_id, id),
            other => panic!("unexpected frame: {other:?}"),
        }
    }

    #[tokio::test]
    async fn approve_non_holding_aircraft_is_noop() {
        let world: SharedWorld = Arc::new(Mutex::new(World::new()));
        let mut a = Aircraft::new("SKY2", Kind::Agent);
        a.state = crate::world::State::Cruising;
        let id = a.id;
        world.lock().await.add(a);

        let (deltas, _dr) = broadcast::channel(64);
        let (adapter_tx, _adapter_rx) = mpsc::channel(64);
        let state = HubState {
            world: Arc::clone(&world),
            deltas,
            adapter_tx,
        };

        handle_frame(&state, ClientFrame::Approve { aircraft_id: id }).await;

        let world = state.world.lock().await;
        let a = world.aircraft.iter().find(|a| a.id == id).unwrap();
        assert!(matches!(a.state, crate::world::State::Cruising));
    }
}
