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
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::{
    api::types::{PairBootstrapRequest, WsQuery},
    pairing::bootstrap::print_pairing_banner_from_relay,
    state::{AppState, ClientHandle, RelayWriteCommand, WS_WRITE_QUEUE_CAPACITY},
    ws::envelope::{sanitize_envelope, send_server_presence, summarize_envelope},
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
    let (tx, mut rx) = mpsc::channel::<RelayWriteCommand>(WS_WRITE_QUEUE_CAPACITY);
    let drop_count = Arc::new(AtomicU64::new(0));

    state
        .insert(
            q.system_id.clone(),
            q.pair_token.clone(),
            client_id,
            ClientHandle {
                client_type: q.client_type.clone(),
                sender: tx.clone(),
                drop_count: drop_count.clone(),
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
        let mut snapshot_latest: HashMap<String, Message> = HashMap::new();
        while let Some(command) = rx.recv().await {
            match command {
                RelayWriteCommand::Direct(msg) => {
                    if ws_sender.send(msg).await.is_err() {
                        break;
                    }
                }
                RelayWriteCommand::Snapshot { key, msg } => {
                    snapshot_latest.insert(key, msg);
                }
            }

            while !snapshot_latest.is_empty() {
                let Some(next_key) = snapshot_latest.keys().next().cloned() else {
                    break;
                };
                let Some(snapshot_msg) = snapshot_latest.remove(&next_key) else {
                    continue;
                };
                if ws_sender.send(snapshot_msg).await.is_err() {
                    return;
                }
                while let Ok(next_command) = rx.try_recv() {
                    match next_command {
                        RelayWriteCommand::Direct(msg) => {
                            if ws_sender.send(msg).await.is_err() {
                                return;
                            }
                        }
                        RelayWriteCommand::Snapshot { key, msg } => {
                            snapshot_latest.insert(key, msg);
                        }
                    }
                }
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

        let summary = summarize_envelope(&sanitized);
        debug!(
            "ws relay message system={} src_type={} src_device={} type={} event_id={} trace_id={} tool_id={}",
            q.system_id,
            q.client_type,
            q.device_id,
            summary.event_type,
            summary.event_id,
            summary.trace_id,
            summary.tool_id
        );

        state
            .broadcast(&q.system_id, client_id, sanitized, &summary.event_type)
            .await;
    }

    state.remove(&q.system_id, client_id).await;
    writer.abort();
    info!(
        "ws disconnected system={} type={} device={}",
        q.system_id, q.client_type, q.device_id
    );
}
