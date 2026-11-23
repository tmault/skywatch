mod world;
mod ws;

use std::sync::Arc;
use std::time::Duration;

use axum::routing::{get, Router};
use tokio::sync::{broadcast, mpsc, Mutex};

use crate::world::World;
use crate::ws::{HubState, SharedWorld};

const TICK_INTERVAL: Duration = Duration::from_millis(50);
const BROADCAST_CAPACITY: usize = 64;
const ADAPTER_CHANNEL_BUFFER: usize = 64;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let world: SharedWorld = Arc::new(Mutex::new(World::new()));
    let (deltas, _delta_rx) = broadcast::channel::<String>(BROADCAST_CAPACITY);
    let (adapter_tx, adapter_rx) = mpsc::channel(ADAPTER_CHANNEL_BUFFER);

    // Sim ticker: advance the world at 20 Hz and broadcast full snapshots.
    {
        let world = Arc::clone(&world);
        let deltas = deltas.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(TICK_INTERVAL);
            loop {
                interval.tick().await;
                let snapshot = {
                    let mut world = world.lock().await;
                    world.tick(0.05);
                    world.snapshot()
                };
                let frame = ws::delta_json(&snapshot);
                // Fine if nobody is listening yet.
                let _ = deltas.send(frame);
            }
        });
    }

    // TODO(Task 4): adapters consume `adapter_rx`.

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/ws", get(ws::ws_handler))
        .with_state(HubState {
            world,
            deltas,
            adapter_tx,
        })
        .fallback_service(get(|| async { "skywatch" }));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8422").await.unwrap();
    tracing::info!("listening on 0.0.0.0:8422");
    axum::serve(listener, app).await.unwrap();
}
