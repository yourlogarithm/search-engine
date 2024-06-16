#[macro_use]
extern crate lazy_static;

mod core;
mod database;
mod robots;
mod routes;
mod state;

use axum::{http::StatusCode, routing::{get, post}, Router};
use state::AppState;

use std::net::SocketAddr;
use tracing::{info, warn};

#[axum::debug_handler]
async fn root() -> StatusCode {
    StatusCode::OK
}

async fn fallback_route() -> (StatusCode, &'static str) {
    warn!("Invalid route accessed");
    (
        StatusCode::NOT_FOUND,
        "The requested resource was not found",
    )
}

#[tokio::main]
async fn main() {
    let state = AppState::new().await;
    let app = Router::new()
        .route("/", get(root))
        .nest(
            "/v1",
            Router::new()
                .route("/crawl/:url", post(routes::crawl))
                .with_state(state),
        )
        .fallback(fallback_route);
    let addr = SocketAddr::from(([0, 0, 0, 0], 8000));
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    info!("Listening on {}", addr);
    axum::serve(listener, app).await.unwrap();
    info!("Server stopped.");
}
