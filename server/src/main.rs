mod world;

use axum::{routing::get, Router};

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .fallback_service(get(|| async { "skywatch" }));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8422").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
