//! WebSocket 握手入口与消息转发循环。

use axum::{
    extract::{
        Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    api::types::{PairBootstrapRequest, WsQuery},
    pairing::bootstrap::print_pairing_banner_from_relay,
    state::{AppState, ClientHandle},
    ws::envelope::{sanitize_envelope, send_server_presence},
};

/// WS 握手入口：校验 query 并升级连接。
pub(crate) async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(mut q): Query<WsQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if q.system_id.trim().is_empty()
        || q.client_type.trim().is_empty()
        || q.device_id.trim().is_empty()
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "missing systemId/clientType/deviceId".to_string(),
        ));
    }

    q.client_type = yc_shared_protocol::normalize_client_type(&q.client_type);
    if q.client_type != "app" && q.client_type != "sidecar" {
        return Err((StatusCode::BAD_REQUEST, "invalid clientType".to_string()));
    }

    let auth_result = state.authorize_connection(&q).await;
    if let Err(err) = auth_result {
        return Err((err.status, format!("{}: {}", err.code, err.message)));
    }

    Ok(ws.on_upgrade(move |socket| handle_socket(state, socket, q)))
}

/// 单连接处理：注册连接、转发消息、连接断开清理。
async fn handle_socket(state: AppState, socket: WebSocket, q: WsQuery) {
    let client_id = Uuid::new_v4();
    let (mut ws_sender, mut ws_reader) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    state
        .insert(
            q.system_id.clone(),
            q.pair_token.clone(),
            client_id,
            ClientHandle {
                client_type: q.client_type.clone(),
                sender: tx.clone(),
            },
        )
        .await;

    if q.client_type == "sidecar" {
        match state
            .issue_pair_bootstrap(&PairBootstrapRequest {
                system_id: q.system_id.clone(),
                pair_token: q.pair_token.clone(),
                host_name: q.host_name.clone(),
                relay_ws_url: None,
                include_code: Some(true),
                ttl_sec: None,
            })
            .await
        {
            Ok(data) => print_pairing_banner_from_relay(&data),
            Err(err) => warn!("bootstrap banner failed: {} {}", err.code, err.message),
        }
    }

    info!(
        "ws connected system={} type={} device={}",
        q.system_id, q.client_type, q.device_id
    );
    send_server_presence(&tx, &q.system_id, &q.client_type, &q.device_id);

    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    while let Some(next) = ws_reader.next().await {
        let msg = match next {
            Ok(m) => m,
            Err(err) => {
                warn!(
                    "ws read error system={} device={}: {err}",
                    q.system_id, q.device_id
                );
                break;
            }
        };

        let Message::Text(text) = msg else {
            continue;
        };

        let sanitized = match sanitize_envelope(&text, &q.system_id, &q.client_type, &q.device_id) {
            Ok(v) => v,
            Err(err) => {
                warn!(
                    "drop invalid payload system={} device={}: {}",
                    q.system_id, q.device_id, err
                );
                continue;
            }
        };

        state.broadcast(&q.system_id, client_id, sanitized).await;
    }

    state.remove(&q.system_id, client_id).await;
    writer.abort();
    info!(
        "ws disconnected system={} type={} device={}",
        q.system_id, q.client_type, q.device_id
    );
}
