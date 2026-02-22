//! Relay 应用装配：路由、CORS 与监听。

use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::State,
    http::{
        Method,
        header::{AUTHORIZATION, CONTENT_TYPE},
    },
    routing::{get, post},
};
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

use crate::{
    auth::handlers::{auth_devices_handler, auth_refresh_handler, auth_revoke_device_handler},
    pairing::handlers::{pair_bootstrap_handler, pair_exchange_handler, pair_preflight_handler},
    state::AppState,
    ws::handlers::ws_handler,
};

/// Relay 入口：启动 HTTP/WS 路由。
pub(crate) async fn run() -> anyhow::Result<()> {
    let addr = std::env::var("RELAY_ADDR").unwrap_or_else(|_| "0.0.0.0:18080".to_string());
    let state = AppState::default();
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([CONTENT_TYPE, AUTHORIZATION]);

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/debug/systems", get(debug_systems))
        .route("/v1/pair/preflight", post(pair_preflight_handler))
        .route("/v1/pair/exchange", post(pair_exchange_handler))
        .route("/v1/pair/bootstrap", post(pair_bootstrap_handler))
        .route("/v1/auth/refresh", post(auth_refresh_handler))
        .route("/v1/auth/revoke-device", post(auth_revoke_device_handler))
        .route("/v1/auth/devices", get(auth_devices_handler))
        .route("/v1/ws", get(ws_handler))
        .layer(cors)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("relay-rs listening on {addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// 健康检查接口。
async fn healthz() -> &'static str {
    "ok"
}

/// 调试接口：查看每个 system 当前连接数。
async fn debug_systems(State(state): State<AppState>) -> Json<HashMap<String, usize>> {
    Json(state.snapshot().await)
}
