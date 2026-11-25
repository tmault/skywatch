mod adapters;
mod platform;
mod seed;
mod time;
mod theme;
mod usage;
mod world;
mod ws;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, Router};
use tokio::sync::{broadcast, mpsc, Mutex};
use tower::ServiceExt;
use tower_http::services::ServeDir;

use crate::world::World;
use crate::ws::{HubState, SharedWorld};

const TICK_INTERVAL: Duration = Duration::from_millis(50);
const BROADCAST_CAPACITY: usize = 64;
const ADAPTER_CHANNEL_BUFFER: usize = 64;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let boot_config = theme::BootConfig::parse(std::env::var("SKYWATCH_THEME").ok().as_deref())
        .unwrap_or_else(|error| panic!("{error}"));

    let world: SharedWorld = Arc::new(Mutex::new(World::new()));
    let (deltas, _delta_rx) = broadcast::channel::<String>(BROADCAST_CAPACITY);
    let (adapter_tx, mut adapter_rx) = mpsc::channel(ADAPTER_CHANNEL_BUFFER);

    // Demo mode: seed a fixed cast and run the scripted state machine.
    // SKYWATCH_DEMO=0 disables; unset or any other value enables it for now.
    let demo_mode = match std::env::var("SKYWATCH_DEMO").as_deref() {
        Ok("0") | Ok("false") => false,
        _ => true,
    };

    // T3 adapter: mirror recent T3 threads as aircraft. SKYWATCH_T3=0
    // disables; when enabled it takes the adapter channel and the demo
    // seed is skipped so the world shows real traffic only.
    let driver = adapters::config::select(
        std::env::var("SKYWATCH_DRIVER").ok().as_deref(),
        std::env::var("SKYWATCH_T3").ok().as_deref(),
    )
    .expect("invalid driver configuration");
    let live_mode = driver != adapters::config::Driver::Disabled;
    if driver == adapters::config::Driver::OpenCode {
        let config =
            adapters::opencode::Config::from_env().unwrap_or_else(|error| panic!("{error}"));
        tokio::spawn(adapters::opencode::run(
            Arc::clone(&world),
            adapter_rx,
            config,
            deltas.clone(),
        ));
        tracing::info!("OpenCode adapter enabled");
    } else if driver == adapters::config::Driver::T3 {
        tokio::spawn(adapters::t3::run(Arc::clone(&world), adapter_rx));
        tracing::info!("t3 adapter enabled");
    } else if demo_mode {
        {
            let mut w = world.lock().await;
            seed::seed(&mut w);
            tracing::info!(count = w.iter().count(), "demo world seeded");
        }
        {
            let world = Arc::clone(&world);
            tokio::spawn(seed::run_script(world));
        }
        // Drain client frames so the channel keeps working; the ws layer has
        // already applied world effects (e.g. approve releases the aircraft).
        tokio::spawn(async move {
            while let Some(frame) = adapter_rx.recv().await {
                tracing::debug!(?frame, "demo adapter: frame dropped");
            }
        });
    } else {
        // Neither adapter nor demo: drain the channel.
        tokio::spawn(async move { while adapter_rx.recv().await.is_some() {} });
    }

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
                    world.tick(TICK_INTERVAL.as_secs_f32());
                    world.snapshot()
                };
                let frame = ws::delta_json(&snapshot);
                // Fine if nobody is listening yet.
                let _ = deltas.send(frame);
            }
        });
    }

    // T3 adapter relays messages via `t3 thread send` when enabled.

    // Serve the browser client; unknown paths fall back to index.html with 200.
    let web_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("web");
    let serve_dir = ServeDir::new(&web_dir);
    let index_bytes = std::fs::read(web_dir.join("index.html")).expect("web/index.html");

    let app = Router::new()
        .route("/config.json", get(move || { let config = boot_config.clone(); async move { axum::Json(config) } }))
        .route("/health", get(|| async { "ok" }))
        .route("/ws", get(ws::ws_handler))
        .with_state(HubState {
            world,
            demo: !live_mode && demo_mode,
            deltas,
            adapter_tx,
        })
        .fallback(get(move |req: Request| {
            let serve_dir = serve_dir.clone();
            let index_bytes = index_bytes.clone();
            async move {
                let mut res = serve_dir.oneshot(req).await.unwrap();
                if res.status() == StatusCode::NOT_FOUND {
                    Response::builder()
                        .status(StatusCode::OK)
                        .header("content-type", "text/html")
                        .header("cache-control", "no-cache")
                        .body(Body::from(index_bytes))
                        .unwrap()
                } else {
                    res.headers_mut().insert(
                        axum::http::header::CACHE_CONTROL,
                        axum::http::HeaderValue::from_static("no-cache"),
                    );
                    res.map(Body::new)
                }
            }
        }));

    let port = std::env::var("SKYWATCH_PORT").unwrap_or_else(|_| "8422".to_string());
    let host = std::env::var("SKYWATCH_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let addr = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("listening on {addr}");
    axum::serve(listener, app).await.unwrap();
}
