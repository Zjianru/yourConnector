//! Relay 主程序职责：
//! 1. 接收 app/sidecar 的 WebSocket 连接并按 systemId 进行路由。
//! 2. 提供配对预检、设备凭证换发、刷新、吊销与设备列表接口。
//! 3. 仅持久化认证元数据（设备绑定/会话/吊销），不落业务消息数据。

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    extract::{
        Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{
        Method, StatusCode,
        header::{AUTHORIZATION, CONTENT_TYPE},
    },
    response::IntoResponse,
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{RwLock, mpsc};
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, warn};
use url::Url;
use uuid::Uuid;
use yc_shared_protocol::{EventEnvelope, normalize_client_type, now_rfc3339_nanos};

/// 预检与换发接口默认使用的 access token TTL（秒）。
const ACCESS_TOKEN_TTL_SEC: u64 = 600;
/// refresh token 有效期（秒）。
const REFRESH_TOKEN_TTL_SEC: u64 = 30 * 24 * 3600;
/// PoP 签名请求时间窗（秒）。
const POP_MAX_SKEW_SEC: u64 = 120;
/// 配对票据默认有效期（秒）。
const DEFAULT_PAIR_TICKET_TTL_SEC: u64 = 300;

/// 终端高亮样式：重置。
const ANSI_RESET: &str = "\x1b[0m";
/// 终端高亮样式：粗体。
const ANSI_BOLD: &str = "\x1b[1m";
/// 终端高亮样式：青色。
const ANSI_CYAN: &str = "\x1b[36m";
/// 终端高亮样式：亮白。
const ANSI_WHITE: &str = "\x1b[97m";

/// Relay 共享状态。
#[derive(Clone)]
struct AppState {
    /// 在线 system 房间（内存）。
    systems: Arc<RwLock<HashMap<String, SystemRoom>>>,
    /// 认证元数据（持久化）。
    auth_store: Arc<RwLock<AuthStore>>,
    /// 认证元数据文件路径。
    auth_store_path: Arc<PathBuf>,
    /// HTTP 鉴权接口 nonce（内存防重放）。
    auth_nonces: Arc<RwLock<HashMap<String, u64>>>,
}

impl Default for AppState {
    fn default() -> Self {
        let path = auth_store_path();
        let store = load_auth_store(&path).unwrap_or_else(|err| {
            warn!("load auth store failed: {err}");
            AuthStore::new()
        });
        Self {
            systems: Arc::new(RwLock::new(HashMap::new())),
            auth_store: Arc::new(RwLock::new(store)),
            auth_store_path: Arc::new(path),
            auth_nonces: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

/// 单个 system 房间状态。
struct SystemRoom {
    /// 当前 system 配对令牌（sidecar 注册）。
    pair_token: String,
    /// 已使用短时票据 nonce。
    ticket_nonces: HashMap<String, u64>,
    /// WS PoP nonce（app）防重放。
    app_nonces: HashMap<String, u64>,
    /// 当前连接客户端集合。
    clients: HashMap<Uuid, ClientHandle>,
}

/// 单个连接发送句柄。
#[derive(Clone)]
struct ClientHandle {
    sender: mpsc::UnboundedSender<Message>,
}

/// WS 握手 query 参数。
#[derive(Debug, Deserialize)]
struct WsQuery {
    #[serde(rename = "systemId")]
    system_id: String,
    #[serde(rename = "clientType")]
    client_type: String,
    #[serde(rename = "deviceId")]
    device_id: String,
    #[serde(rename = "pairToken", default)]
    pair_token: String,
    #[serde(rename = "pairTicket", default)]
    pair_ticket: Option<String>,
    #[serde(rename = "hostName", default)]
    host_name: Option<String>,
    /// 设备 access token（生产链路）。
    #[serde(rename = "accessToken", default)]
    access_token: Option<String>,
    /// 设备 key id。
    #[serde(rename = "keyId", default)]
    key_id: Option<String>,
    /// 签名时间戳（秒）。
    #[serde(rename = "ts", default)]
    ts: Option<String>,
    /// 请求 nonce。
    #[serde(default)]
    nonce: Option<String>,
    /// PoP 签名。
    #[serde(rename = "sig", default)]
    sig: Option<String>,
}

/// 通用 API 成功/失败包裹结构。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiEnvelope<T>
where
    T: Serialize,
{
    ok: bool,
    code: String,
    message: String,
    suggestion: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<T>,
}

/// 认证错误。
#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    suggestion: &'static str,
}

impl ApiError {
    fn new(
        status: StatusCode,
        code: &'static str,
        message: impl Into<String>,
        suggestion: &'static str,
    ) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            suggestion,
        }
    }

    fn into_response(self) -> (StatusCode, Json<ApiEnvelope<Value>>) {
        (
            self.status,
            Json(ApiEnvelope {
                ok: false,
                code: self.code.to_string(),
                message: self.message,
                suggestion: self.suggestion.to_string(),
                data: None,
            }),
        )
    }
}

/// 配对鉴权方式。
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
enum PairAuthMode {
    PairTicket,
    PairToken,
}

/// 配对预检请求。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairPreflightRequest {
    system_id: String,
    device_id: String,
    #[serde(default)]
    pair_token: Option<String>,
    #[serde(default)]
    pair_ticket: Option<String>,
}

/// 配对预检返回数据。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PairPreflightData {
    auth_mode: PairAuthMode,
}

/// 配对换发请求。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairExchangeRequest {
    system_id: String,
    device_id: String,
    #[serde(default)]
    device_name: String,
    #[serde(default)]
    pair_token: Option<String>,
    #[serde(default)]
    pair_ticket: Option<String>,
    device_pub_key: String,
    key_id: String,
    proof: String,
}

/// 配对换发数据。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PairExchangeData {
    auth_mode: PairAuthMode,
    access_token: String,
    refresh_token: String,
    key_id: String,
    credential_id: String,
    access_expires_in_sec: u64,
    refresh_expires_in_sec: u64,
}

/// 刷新请求。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthRefreshRequest {
    system_id: String,
    device_id: String,
    refresh_token: String,
    key_id: String,
    ts: String,
    nonce: String,
    sig: String,
}

/// 刷新返回。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthRefreshData {
    access_token: String,
    refresh_token: String,
    key_id: String,
    credential_id: String,
    access_expires_in_sec: u64,
    refresh_expires_in_sec: u64,
}

/// 吊销请求。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthRevokeDeviceRequest {
    system_id: String,
    device_id: String,
    target_device_id: String,
    access_token: String,
    key_id: String,
    ts: String,
    nonce: String,
    sig: String,
}

/// 吊销结果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthRevokeDeviceData {
    target_device_id: String,
}

/// 设备列表查询参数。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthDevicesQuery {
    system_id: String,
    device_id: String,
    access_token: String,
    key_id: String,
    ts: String,
    nonce: String,
    sig: String,
}

/// 设备列表项。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceEntry {
    device_id: String,
    device_name: String,
    key_id: String,
    status: String,
    created_at: String,
    last_seen_at: String,
    revoked_at: Option<String>,
}

/// 设备列表返回。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthDevicesData {
    devices: Vec<DeviceEntry>,
}

/// 持久化认证元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthStore {
    version: u32,
    signing_key: String,
    systems: HashMap<String, SystemAuthState>,
}

impl AuthStore {
    fn new() -> Self {
        Self {
            version: 1,
            signing_key: generate_signing_key_seed(),
            systems: HashMap::new(),
        }
    }

    fn system_mut(&mut self, system_id: &str) -> &mut SystemAuthState {
        self.systems
            .entry(system_id.to_string())
            .or_insert_with(SystemAuthState::default)
    }

    fn system_ref(&self, system_id: &str) -> Option<&SystemAuthState> {
        self.systems.get(system_id)
    }
}

/// 单 system 认证元数据。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SystemAuthState {
    pair_token_hash: Option<String>,
    pair_token_updated_at: Option<String>,
    devices: HashMap<String, DeviceCredential>,
    refresh_sessions: HashMap<String, RefreshSession>,
}

/// 设备凭证记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceCredential {
    device_id: String,
    device_name: String,
    key_id: String,
    public_key: String,
    status: String,
    created_at: String,
    last_seen_at: String,
    revoked_at: Option<String>,
}

/// refresh 会话记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RefreshSession {
    session_id: String,
    system_id: String,
    device_id: String,
    key_id: String,
    credential_id: String,
    refresh_secret_hash: String,
    expires_at: u64,
    created_at: String,
    revoked_at: Option<String>,
    rotated_from: Option<String>,
}

/// access token claims。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccessTokenClaims {
    sid: String,
    did: String,
    kid: String,
    iat: u64,
    exp: u64,
    jti: String,
}

/// 短时票据 claims。
#[derive(Debug, Deserialize)]
struct PairTicketClaims {
    sid: String,
    iat: u64,
    exp: u64,
    nonce: String,
}

/// 配对票据校验错误。
#[derive(Debug)]
enum PairTicketError {
    Empty,
    Format,
    SignatureFormat,
    SignatureVerify,
    Payload,
    Claims,
    SystemMismatch,
    EmptyNonce,
    Expired,
    IatInvalid,
    Replay,
}

/// pairToken 鉴权决策。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum PairTokenAuthDecision {
    Allow,
    Rotate,
    Initialize,
}

/// Relay 入口：启动 HTTP/WS 路由。
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

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

/// 配对预检接口：用于移动端精确映射失败弹窗。
async fn pair_preflight_handler(
    State(state): State<AppState>,
    Json(req): Json<PairPreflightRequest>,
) -> (StatusCode, Json<ApiEnvelope<PairPreflightData>>) {
    match state.preflight_pair_credentials(&req).await {
        Ok(mode) => (
            StatusCode::OK,
            Json(ApiEnvelope {
                ok: true,
                code: "OK".to_string(),
                message: "配对信息可用".to_string(),
                suggestion: "可以继续执行配对并连接".to_string(),
                data: Some(PairPreflightData { auth_mode: mode }),
            }),
        ),
        Err(err) => {
            let (status, body) = err.into_response();
            (
                status,
                Json(ApiEnvelope {
                    ok: body.0.ok,
                    code: body.0.code,
                    message: body.0.message,
                    suggestion: body.0.suggestion,
                    data: None,
                }),
            )
        }
    }
}

/// 配对换发接口：绑定设备公钥并签发 access/refresh。
async fn pair_exchange_handler(
    State(state): State<AppState>,
    Json(req): Json<PairExchangeRequest>,
) -> (StatusCode, Json<ApiEnvelope<PairExchangeData>>) {
    let result = state.exchange_device_credential(&req).await;
    match result {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiEnvelope {
                ok: true,
                code: "OK".to_string(),
                message: "设备凭证换发成功".to_string(),
                suggestion: "后续连接将使用设备凭证".to_string(),
                data: Some(data),
            }),
        ),
        Err(err) => {
            let (status, body) = err.into_response();
            (
                status,
                Json(ApiEnvelope {
                    ok: body.0.ok,
                    code: body.0.code,
                    message: body.0.message,
                    suggestion: body.0.suggestion,
                    data: None,
                }),
            )
        }
    }
}

/// 刷新接口：轮换 refresh 并颁发新 access。
async fn auth_refresh_handler(
    State(state): State<AppState>,
    Json(req): Json<AuthRefreshRequest>,
) -> (StatusCode, Json<ApiEnvelope<AuthRefreshData>>) {
    match state.refresh_device_credential(&req).await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiEnvelope {
                ok: true,
                code: "OK".to_string(),
                message: "凭证刷新成功".to_string(),
                suggestion: "继续使用当前会话".to_string(),
                data: Some(data),
            }),
        ),
        Err(err) => {
            let (status, body) = err.into_response();
            (
                status,
                Json(ApiEnvelope {
                    ok: body.0.ok,
                    code: body.0.code,
                    message: body.0.message,
                    suggestion: body.0.suggestion,
                    data: None,
                }),
            )
        }
    }
}

/// 吊销设备接口。
async fn auth_revoke_device_handler(
    State(state): State<AppState>,
    Json(req): Json<AuthRevokeDeviceRequest>,
) -> (StatusCode, Json<ApiEnvelope<AuthRevokeDeviceData>>) {
    match state.revoke_device(&req).await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiEnvelope {
                ok: true,
                code: "OK".to_string(),
                message: "设备已吊销".to_string(),
                suggestion: "被吊销设备需重新配对".to_string(),
                data: Some(data),
            }),
        ),
        Err(err) => {
            let (status, body) = err.into_response();
            (
                status,
                Json(ApiEnvelope {
                    ok: body.0.ok,
                    code: body.0.code,
                    message: body.0.message,
                    suggestion: body.0.suggestion,
                    data: None,
                }),
            )
        }
    }
}

/// 设备列表接口。
async fn auth_devices_handler(
    State(state): State<AppState>,
    Query(query): Query<AuthDevicesQuery>,
) -> (StatusCode, Json<ApiEnvelope<AuthDevicesData>>) {
    match state.list_devices(&query).await {
        Ok(data) => (
            StatusCode::OK,
            Json(ApiEnvelope {
                ok: true,
                code: "OK".to_string(),
                message: "设备列表获取成功".to_string(),
                suggestion: "可以对设备进行管理".to_string(),
                data: Some(data),
            }),
        ),
        Err(err) => {
            let (status, body) = err.into_response();
            (
                status,
                Json(ApiEnvelope {
                    ok: body.0.ok,
                    code: body.0.code,
                    message: body.0.message,
                    suggestion: body.0.suggestion,
                    data: None,
                }),
            )
        }
    }
}

/// WS 握手入口：校验 query 并升级连接。
async fn ws_handler(
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

    q.client_type = normalize_client_type(&q.client_type);
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
            ClientHandle { sender: tx.clone() },
        )
        .await;

    if q.client_type == "sidecar" {
        print_pairing_banner_from_relay(&q.system_id, &q.pair_token, q.host_name.as_deref());
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

impl AppState {
    /// 连接鉴权入口：app 优先走 access token，兼容 pairToken/pairTicket；sidecar 只走 pairToken。
    async fn authorize_connection(&self, q: &WsQuery) -> Result<(), ApiError> {
        if q.client_type == "sidecar" {
            if q.pair_token.trim().is_empty() {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "MISSING_CREDENTIALS",
                    "sidecar 缺少 pairToken",
                    "请重启 sidecar 并检查配置",
                ));
            }
            return self.authorize_sidecar(q).await;
        }

        if let Some(access_token) = q.access_token.as_deref().map(str::trim)
            && !access_token.is_empty()
        {
            return self.authorize_app_with_access(q).await;
        }

        self.authorize_app_with_pair(q).await
    }

    /// sidecar 鉴权并建房。
    async fn authorize_sidecar(&self, q: &WsQuery) -> Result<(), ApiError> {
        let incoming_pair_token = q.pair_token.trim();
        let mut guard = self.systems.write().await;
        let Some(room) = guard.get_mut(&q.system_id) else {
            guard.insert(
                q.system_id.clone(),
                SystemRoom {
                    pair_token: incoming_pair_token.to_string(),
                    ticket_nonces: HashMap::new(),
                    app_nonces: HashMap::new(),
                    clients: HashMap::new(),
                },
            );
            self.persist_pair_token_meta(&q.system_id, incoming_pair_token)
                .await;
            return Ok(());
        };

        match authorize_pair_token(
            Some(room.pair_token.as_str()),
            room.clients.len(),
            &q.client_type,
            incoming_pair_token,
        )
        .map_err(|_| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "PAIR_TOKEN_MISMATCH",
                "pairToken 不匹配",
                "请重新生成配对信息后再试",
            )
        })? {
            PairTokenAuthDecision::Allow => Ok(()),
            PairTokenAuthDecision::Rotate => {
                room.pair_token = incoming_pair_token.to_string();
                drop(guard);
                self.persist_pair_token_meta(&q.system_id, incoming_pair_token)
                    .await;
                Ok(())
            }
            PairTokenAuthDecision::Initialize => Ok(()),
        }
    }

    /// app 使用 access token + PoP 的生产鉴权。
    async fn authorize_app_with_access(&self, q: &WsQuery) -> Result<(), ApiError> {
        let access_token = q
            .access_token
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "MISSING_CREDENTIALS",
                    "缺少 accessToken",
                    "请重新配对并连接",
                )
            })?;

        let key_id = q
            .key_id
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "MISSING_CREDENTIALS",
                    "缺少 keyId",
                    "请重新配对并连接",
                )
            })?;

        let ts = parse_ts(
            q.ts.as_deref().unwrap_or_default(),
            "ACCESS_SIGNATURE_EXPIRED",
            "签名时间戳无效",
        )?;
        verify_ts_window(ts, "ACCESS_SIGNATURE_EXPIRED", "签名时间窗已过期")?;

        let nonce = q
            .nonce
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "MISSING_CREDENTIALS",
                    "缺少 nonce",
                    "请重新连接",
                )
            })?;
        let sig = q
            .sig
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "MISSING_CREDENTIALS",
                    "缺少签名",
                    "请重新连接",
                )
            })?;

        let payload = ws_pop_payload(&q.system_id, &q.device_id, key_id, ts, nonce);
        let device = {
            let guard = self.auth_store.read().await;
            verify_access_token(
                access_token,
                &guard.signing_key,
                &q.system_id,
                &q.device_id,
                key_id,
            )?;

            let Some(system) = guard.system_ref(&q.system_id) else {
                return Err(ApiError::new(
                    StatusCode::UNAUTHORIZED,
                    "SYSTEM_NOT_REGISTERED",
                    "system 未注册，请先启动 sidecar",
                    "先启动宿主机 sidecar 再配对",
                ));
            };
            let Some(device) = system.devices.get(&q.device_id) else {
                return Err(ApiError::new(
                    StatusCode::UNAUTHORIZED,
                    "DEVICE_REVOKED",
                    "设备未绑定或已被移除",
                    "请重新扫码配对",
                ));
            };
            if device.status != "ACTIVE" {
                return Err(ApiError::new(
                    StatusCode::UNAUTHORIZED,
                    "DEVICE_REVOKED",
                    "设备已被吊销",
                    "请重新扫码配对",
                ));
            }
            verify_pop_signature(&device.public_key, &payload, sig)?;
            device.clone()
        };

        let mut guard = self.systems.write().await;
        let Some(room) = guard.get_mut(&q.system_id) else {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "SYSTEM_NOT_REGISTERED",
                "宿主机未在线",
                "请先启动 sidecar",
            ));
        };

        let now = unix_now();
        room.app_nonces.retain(|_, exp| exp.saturating_add(5) > now);
        if let Some(exp) = room.app_nonces.get(nonce)
            && *exp > now
        {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "ACCESS_SIGNATURE_REPLAYED",
                "签名请求已使用，请重试",
                "请重新发起连接",
            ));
        }
        room.app_nonces
            .insert(nonce.to_string(), now.saturating_add(POP_MAX_SKEW_SEC));

        drop(guard);
        self.touch_device_last_seen(&q.system_id, &device.device_id)
            .await;
        Ok(())
    }

    /// app 的兼容链路：pairTicket / pairToken。
    async fn authorize_app_with_pair(&self, q: &WsQuery) -> Result<(), ApiError> {
        let pair_token = q.pair_token.trim();
        let pair_ticket = q.pair_ticket.as_deref().map(str::trim).unwrap_or("");

        let mut guard = self.systems.write().await;
        let Some(room) = guard.get_mut(&q.system_id) else {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "SYSTEM_NOT_REGISTERED",
                "system 未注册，请先启动 sidecar",
                "请先启动宿主机 sidecar",
            ));
        };

        if !pair_ticket.is_empty() {
            match verify_pairing_ticket(
                pair_ticket,
                &q.system_id,
                &room.pair_token,
                &mut room.ticket_nonces,
                true,
            ) {
                Ok(_) => return Ok(()),
                Err(ticket_err) => return Err(pair_ticket_error_to_api(ticket_err)),
            }
        }

        if pair_token.is_empty() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "MISSING_CREDENTIALS",
                "缺少 pairToken/pairTicket",
                "请重新扫码或手动输入配对码",
            ));
        }

        authorize_pair_token(
            Some(room.pair_token.as_str()),
            room.clients.len(),
            &q.client_type,
            pair_token,
        )
        .map_err(|_| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "PAIR_TOKEN_MISMATCH",
                "配对信息无效",
                "请重新生成配对信息后再试",
            )
        })?;
        Ok(())
    }

    /// 配对预检（不消费票据）。
    async fn preflight_pair_credentials(
        &self,
        req: &PairPreflightRequest,
    ) -> Result<PairAuthMode, ApiError> {
        let system_id = req.system_id.trim();
        let device_id = req.device_id.trim();
        if system_id.is_empty() || device_id.is_empty() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "MISSING_CREDENTIALS",
                "systemId/deviceId 不能为空",
                "请检查配对信息",
            ));
        }

        let pair_token = req.pair_token.as_deref().unwrap_or_default().trim();
        let pair_ticket = req.pair_ticket.as_deref().unwrap_or_default().trim();
        self.verify_pair_credentials(system_id, pair_token, pair_ticket, false)
            .await
    }

    /// 配对换发设备凭证（消费票据）。
    async fn exchange_device_credential(
        &self,
        req: &PairExchangeRequest,
    ) -> Result<PairExchangeData, ApiError> {
        let system_id = req.system_id.trim();
        let device_id = req.device_id.trim();
        let key_id = req.key_id.trim();
        let pubkey = req.device_pub_key.trim();
        let proof = req.proof.trim();

        if system_id.is_empty()
            || device_id.is_empty()
            || key_id.is_empty()
            || pubkey.is_empty()
            || proof.is_empty()
        {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "MISSING_CREDENTIALS",
                "换发参数不完整",
                "请重新扫码或重新粘贴配对链接",
            ));
        }

        let pair_token = req.pair_token.as_deref().unwrap_or_default().trim();
        let pair_ticket = req.pair_ticket.as_deref().unwrap_or_default().trim();
        let auth_mode = self
            .verify_pair_credentials(system_id, pair_token, pair_ticket, true)
            .await?;

        let expected_payload = pair_exchange_payload(system_id, device_id, key_id);
        verify_pop_signature(pubkey, &expected_payload, proof)?;

        let expected_key_id = key_id_for_public_key(pubkey)?;
        if expected_key_id != key_id {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "PAIR_PROOF_INVALID",
                "keyId 与设备公钥不匹配",
                "请重新生成设备绑定信息后重试",
            ));
        }

        let mut store = self.auth_store.write().await;
        let signing_key = store.signing_key.clone();
        let system = store.system_mut(system_id);

        let now_text = now_rfc3339_nanos();
        let device_name = normalize_device_name(&req.device_name, device_id);
        let credential_id = format!("crd_{}", Uuid::new_v4().simple());

        system.devices.insert(
            device_id.to_string(),
            DeviceCredential {
                device_id: device_id.to_string(),
                device_name,
                key_id: key_id.to_string(),
                public_key: pubkey.to_string(),
                status: "ACTIVE".to_string(),
                created_at: now_text.clone(),
                last_seen_at: now_text,
                revoked_at: None,
            },
        );

        let access_token = issue_access_token(
            &signing_key,
            system_id,
            device_id,
            key_id,
            ACCESS_TOKEN_TTL_SEC,
        )?;
        let (refresh_token, refresh_session) =
            issue_refresh_session(system_id, device_id, key_id, &credential_id);
        system
            .refresh_sessions
            .insert(refresh_session.session_id.clone(), refresh_session);

        persist_auth_store(&self.auth_store_path, &store).map_err(|err| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                err,
                "请稍后重试",
            )
        })?;

        Ok(PairExchangeData {
            auth_mode,
            access_token,
            refresh_token,
            key_id: key_id.to_string(),
            credential_id,
            access_expires_in_sec: ACCESS_TOKEN_TTL_SEC,
            refresh_expires_in_sec: REFRESH_TOKEN_TTL_SEC,
        })
    }

    /// 刷新设备凭证（轮换 refresh）。
    async fn refresh_device_credential(
        &self,
        req: &AuthRefreshRequest,
    ) -> Result<AuthRefreshData, ApiError> {
        let system_id = req.system_id.trim();
        let device_id = req.device_id.trim();
        let key_id = req.key_id.trim();
        if system_id.is_empty() || device_id.is_empty() || key_id.is_empty() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "MISSING_CREDENTIALS",
                "刷新参数不完整",
                "请重新登录设备",
            ));
        }

        let ts = parse_ts(&req.ts, "ACCESS_SIGNATURE_EXPIRED", "签名时间戳无效")?;
        verify_ts_window(ts, "ACCESS_SIGNATURE_EXPIRED", "签名时间窗已过期")?;
        self.consume_auth_nonce("refresh", &req.nonce, ts).await?;

        let payload = auth_refresh_payload(system_id, device_id, key_id, ts, &req.nonce);
        let (session_id, refresh_secret) = parse_refresh_token(&req.refresh_token)?;
        let mut store = self.auth_store.write().await;
        let signing_key = store.signing_key.clone();
        let Some(system) = store.systems.get_mut(system_id) else {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "SYSTEM_NOT_REGISTERED",
                "system 未注册",
                "请重新配对",
            ));
        };

        let Some(old_session) = system.refresh_sessions.get_mut(&session_id) else {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "REFRESH_TOKEN_INVALID",
                "refreshToken 无效",
                "请重新配对",
            ));
        };

        if old_session.revoked_at.is_some() {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "REFRESH_TOKEN_INVALID",
                "refreshToken 已失效",
                "请重新配对",
            ));
        }
        if old_session.expires_at <= unix_now() {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "REFRESH_TOKEN_EXPIRED",
                "refreshToken 已过期",
                "请重新配对",
            ));
        }

        let hash = sha256_hex(&refresh_secret);
        if hash != old_session.refresh_secret_hash {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "REFRESH_TOKEN_INVALID",
                "refreshToken 校验失败",
                "请重新配对",
            ));
        }

        if old_session.device_id != device_id || old_session.key_id != key_id {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "REFRESH_TOKEN_INVALID",
                "refreshToken 与设备不匹配",
                "请重新配对",
            ));
        }
        let Some(device) = system.devices.get(device_id) else {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "DEVICE_REVOKED",
                "设备未绑定",
                "请重新配对",
            ));
        };
        if device.status != "ACTIVE" {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "DEVICE_REVOKED",
                "设备已吊销",
                "请重新配对",
            ));
        }
        verify_pop_signature(&device.public_key, &payload, &req.sig)?;

        old_session.revoked_at = Some(now_rfc3339_nanos());
        let credential_id = old_session.credential_id.clone();
        let rotated_from = Some(old_session.session_id.clone());

        let access_token = issue_access_token(
            &signing_key,
            system_id,
            device_id,
            key_id,
            ACCESS_TOKEN_TTL_SEC,
        )?;
        let (refresh_token, mut new_session) =
            issue_refresh_session(system_id, device_id, key_id, &credential_id);
        new_session.rotated_from = rotated_from;
        system
            .refresh_sessions
            .insert(new_session.session_id.clone(), new_session);

        persist_auth_store(&self.auth_store_path, &store).map_err(|err| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                err,
                "请稍后重试",
            )
        })?;

        Ok(AuthRefreshData {
            access_token,
            refresh_token,
            key_id: key_id.to_string(),
            credential_id,
            access_expires_in_sec: ACCESS_TOKEN_TTL_SEC,
            refresh_expires_in_sec: REFRESH_TOKEN_TTL_SEC,
        })
    }

    /// 吊销指定设备。
    async fn revoke_device(
        &self,
        req: &AuthRevokeDeviceRequest,
    ) -> Result<AuthRevokeDeviceData, ApiError> {
        let system_id = req.system_id.trim();
        let device_id = req.device_id.trim();
        let key_id = req.key_id.trim();
        let target_device_id = req.target_device_id.trim();
        if system_id.is_empty()
            || device_id.is_empty()
            || key_id.is_empty()
            || target_device_id.is_empty()
        {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "MISSING_CREDENTIALS",
                "吊销参数不完整",
                "请检查输入后重试",
            ));
        }

        let ts = parse_ts(&req.ts, "ACCESS_SIGNATURE_EXPIRED", "签名时间戳无效")?;
        verify_ts_window(ts, "ACCESS_SIGNATURE_EXPIRED", "签名时间窗已过期")?;
        self.consume_auth_nonce("revoke", &req.nonce, ts).await?;

        let payload = auth_revoke_payload(
            system_id,
            device_id,
            target_device_id,
            key_id,
            ts,
            &req.nonce,
        );
        self.verify_access_http(
            system_id,
            device_id,
            key_id,
            &req.access_token,
            &payload,
            &req.sig,
        )
        .await?;

        let mut store = self.auth_store.write().await;
        let Some(system) = store.systems.get_mut(system_id) else {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "SYSTEM_NOT_REGISTERED",
                "system 不存在",
                "请先完成配对",
            ));
        };

        let Some(target) = system.devices.get_mut(target_device_id) else {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "DEVICE_NOT_FOUND",
                "目标设备不存在",
                "请刷新后重试",
            ));
        };

        target.status = "REVOKED".to_string();
        target.revoked_at = Some(now_rfc3339_nanos());
        for session in system.refresh_sessions.values_mut() {
            if session.device_id == target_device_id {
                session.revoked_at = Some(now_rfc3339_nanos());
            }
        }

        persist_auth_store(&self.auth_store_path, &store).map_err(|err| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                err,
                "请稍后重试",
            )
        })?;

        Ok(AuthRevokeDeviceData {
            target_device_id: target_device_id.to_string(),
        })
    }

    /// 查询设备列表。
    async fn list_devices(&self, req: &AuthDevicesQuery) -> Result<AuthDevicesData, ApiError> {
        let system_id = req.system_id.trim();
        let device_id = req.device_id.trim();
        let key_id = req.key_id.trim();
        if system_id.is_empty() || device_id.is_empty() || key_id.is_empty() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "MISSING_CREDENTIALS",
                "设备列表参数不完整",
                "请检查后重试",
            ));
        }

        let ts = parse_ts(&req.ts, "ACCESS_SIGNATURE_EXPIRED", "签名时间戳无效")?;
        verify_ts_window(ts, "ACCESS_SIGNATURE_EXPIRED", "签名时间窗已过期")?;
        self.consume_auth_nonce("devices", &req.nonce, ts).await?;

        let payload = auth_list_payload(system_id, device_id, key_id, ts, &req.nonce);
        self.verify_access_http(
            system_id,
            device_id,
            key_id,
            &req.access_token,
            &payload,
            &req.sig,
        )
        .await?;

        let store = self.auth_store.read().await;
        let Some(system) = store.system_ref(system_id) else {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "SYSTEM_NOT_REGISTERED",
                "system 不存在",
                "请重新配对",
            ));
        };

        let mut devices = system
            .devices
            .values()
            .cloned()
            .map(|item| DeviceEntry {
                device_id: item.device_id,
                device_name: item.device_name,
                key_id: item.key_id,
                status: item.status,
                created_at: item.created_at,
                last_seen_at: item.last_seen_at,
                revoked_at: item.revoked_at,
            })
            .collect::<Vec<_>>();
        devices.sort_by(|a, b| a.device_id.cmp(&b.device_id));
        Ok(AuthDevicesData { devices })
    }

    /// HTTP 鉴权：access token + PoP。
    async fn verify_access_http(
        &self,
        system_id: &str,
        device_id: &str,
        key_id: &str,
        access_token: &str,
        payload: &str,
        sig: &str,
    ) -> Result<(), ApiError> {
        let store = self.auth_store.read().await;
        verify_access_token(
            access_token,
            &store.signing_key,
            system_id,
            device_id,
            key_id,
        )?;

        let Some(system) = store.system_ref(system_id) else {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "SYSTEM_NOT_REGISTERED",
                "system 未注册",
                "请先启动 sidecar",
            ));
        };
        let Some(device) = system.devices.get(device_id) else {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "DEVICE_REVOKED",
                "设备未绑定",
                "请重新配对",
            ));
        };
        if device.status != "ACTIVE" {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "DEVICE_REVOKED",
                "设备已吊销",
                "请重新配对",
            ));
        }
        verify_pop_signature(&device.public_key, payload, sig)
    }

    /// pair 凭证校验。
    async fn verify_pair_credentials(
        &self,
        system_id: &str,
        pair_token: &str,
        pair_ticket: &str,
        consume_ticket: bool,
    ) -> Result<PairAuthMode, ApiError> {
        let mut guard = self.systems.write().await;
        let Some(room) = guard.get_mut(system_id) else {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "SYSTEM_NOT_REGISTERED",
                "宿主机未在线",
                "请先启动 sidecar",
            ));
        };

        if !pair_ticket.is_empty() {
            match verify_pairing_ticket(
                pair_ticket,
                system_id,
                &room.pair_token,
                &mut room.ticket_nonces,
                consume_ticket,
            ) {
                Ok(_) => return Ok(PairAuthMode::PairTicket),
                Err(err) => return Err(pair_ticket_error_to_api(err)),
            }
        }

        if pair_token.is_empty() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "MISSING_CREDENTIALS",
                "缺少 pairToken/pairTicket",
                "请重新扫码或手动输入配对码",
            ));
        }

        authorize_pair_token(
            Some(room.pair_token.as_str()),
            room.clients.len(),
            "app",
            pair_token,
        )
        .map_err(|_| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "PAIR_TOKEN_MISMATCH",
                "配对信息无效",
                "请重新生成配对信息后再试",
            )
        })?;
        Ok(PairAuthMode::PairToken)
    }

    /// 注册 system 房间连接。
    async fn insert(
        &self,
        system_id: String,
        pair_token: String,
        client_id: Uuid,
        handle: ClientHandle,
    ) {
        let mut guard = self.systems.write().await;
        let room = guard.entry(system_id).or_insert_with(|| SystemRoom {
            pair_token,
            ticket_nonces: HashMap::new(),
            app_nonces: HashMap::new(),
            clients: HashMap::new(),
        });
        room.clients.insert(client_id, handle);
    }

    /// 移除 system 房间连接。
    async fn remove(&self, system_id: &str, client_id: Uuid) {
        let mut guard = self.systems.write().await;
        if let Some(room) = guard.get_mut(system_id) {
            room.clients.remove(&client_id);
        }
    }

    /// 广播到同 system 其他连接。
    async fn broadcast(&self, system_id: &str, origin_id: Uuid, msg: String) {
        let mut stale = Vec::new();

        {
            let guard = self.systems.read().await;
            if let Some(room) = guard.get(system_id) {
                for (client_id, handle) in &room.clients {
                    if *client_id == origin_id {
                        continue;
                    }
                    if handle
                        .sender
                        .send(Message::Text(msg.clone().into()))
                        .is_err()
                    {
                        stale.push(*client_id);
                    }
                }
            }
        }

        if stale.is_empty() {
            return;
        }

        let mut guard = self.systems.write().await;
        if let Some(room) = guard.get_mut(system_id) {
            for client_id in stale {
                room.clients.remove(&client_id);
            }
        }
    }

    /// system 连接数快照。
    async fn snapshot(&self) -> HashMap<String, usize> {
        let guard = self.systems.read().await;
        guard
            .iter()
            .map(|(system_id, room)| (system_id.clone(), room.clients.len()))
            .collect()
    }

    /// 记录 pair token 元数据（仅 hash，不存明文）。
    async fn persist_pair_token_meta(&self, system_id: &str, pair_token: &str) {
        let mut store = self.auth_store.write().await;
        let system = store.system_mut(system_id);
        system.pair_token_hash = Some(sha256_hex(pair_token));
        system.pair_token_updated_at = Some(now_rfc3339_nanos());
        if let Err(err) = persist_auth_store(&self.auth_store_path, &store) {
            warn!("persist pair token meta failed: {err}");
        }
    }

    /// 更新设备最后活跃时间。
    async fn touch_device_last_seen(&self, system_id: &str, device_id: &str) {
        let mut store = self.auth_store.write().await;
        let Some(system) = store.systems.get_mut(system_id) else {
            return;
        };
        let Some(device) = system.devices.get_mut(device_id) else {
            return;
        };
        device.last_seen_at = now_rfc3339_nanos();
        if let Err(err) = persist_auth_store(&self.auth_store_path, &store) {
            warn!("persist device last_seen failed: {err}");
        }
    }

    /// 消费 HTTP nonce（防重放）。
    async fn consume_auth_nonce(&self, scope: &str, nonce: &str, ts: u64) -> Result<(), ApiError> {
        let normalized = nonce.trim();
        if normalized.is_empty() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "MISSING_CREDENTIALS",
                "缺少 nonce",
                "请重试",
            ));
        }
        let now = unix_now();
        if ts > now.saturating_add(POP_MAX_SKEW_SEC) {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "ACCESS_SIGNATURE_EXPIRED",
                "签名时间无效",
                "请重新发起请求",
            ));
        }

        let key = format!("{scope}:{normalized}");
        let mut guard = self.auth_nonces.write().await;
        guard.retain(|_, exp| exp.saturating_add(5) > now);
        if let Some(exp) = guard.get(&key)
            && *exp > now
        {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "ACCESS_SIGNATURE_REPLAYED",
                "签名请求重复",
                "请重新发起请求",
            ));
        }
        guard.insert(key, now.saturating_add(POP_MAX_SKEW_SEC));
        Ok(())
    }
}

/// Relay 用于展示配对链接的公开 WS 地址。
fn relay_public_ws_url() -> String {
    let from_env = std::env::var("RELAY_PUBLIC_WS_URL")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    from_env.unwrap_or_else(|| "ws://127.0.0.1:18080/v1/ws".to_string())
}

/// 读取配对票据有效期（秒）。
fn pairing_ticket_ttl_sec() -> u64 {
    std::env::var("PAIRING_TICKET_TTL_SEC")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v >= 30 && *v <= 3600)
        .unwrap_or(DEFAULT_PAIR_TICKET_TTL_SEC)
}

/// 生成短时配对票据（`pct_v1.<payload_b64url>.<sig_b64url>`）。
fn generate_pairing_ticket(system_id: &str, pair_token: &str, ttl_sec: u64) -> String {
    let now = unix_now();
    let exp = now.saturating_add(ttl_sec);
    let nonce = Uuid::new_v4().simple().to_string();
    let payload = json!({
        "sid": system_id,
        "iat": now,
        "exp": exp,
        "nonce": nonce
    });
    let payload_raw =
        serde_json::to_string(&payload).expect("pair ticket payload must be serializable");
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload_raw.as_bytes());

    type HmacSha256 = Hmac<Sha256>;
    let mut mac =
        HmacSha256::new_from_slice(pair_token.as_bytes()).expect("hmac key should be valid");
    mac.update(payload_b64.as_bytes());
    let sig = mac.finalize().into_bytes();
    let sig_b64 = URL_SAFE_NO_PAD.encode(sig);
    format!("pct_v1.{payload_b64}.{sig_b64}")
}

/// 生成可扫码导入的统一配对链接。
fn build_pairing_link(
    relay_ws_url: &str,
    system_id: &str,
    pair_token: &str,
    host_name: Option<&str>,
) -> String {
    let pair_ticket = generate_pairing_ticket(system_id, pair_token, pairing_ticket_ttl_sec());
    let mut link = Url::parse("yc://pair").expect("pairing link base must be valid");
    {
        let mut pairs = link.query_pairs_mut();
        pairs.append_pair("relay", relay_ws_url);
        pairs.append_pair("sid", system_id);
        pairs.append_pair("ticket", &pair_ticket);
        if let Some(name) = host_name {
            let normalized = name.trim();
            if !normalized.is_empty() {
                pairs.append_pair("name", normalized);
            }
        }
    }
    link.to_string()
}

/// sidecar 接入 relay 后，高亮打印配对信息。
fn print_pairing_banner_from_relay(system_id: &str, pair_token: &str, host_name: Option<&str>) {
    let relay_ws_url = relay_public_ws_url();
    let pairing_code = format!("{system_id}.{pair_token}");
    let pairing_link = build_pairing_link(&relay_ws_url, system_id, pair_token, host_name);
    let simctl_cmd = format!("xcrun simctl openurl booted \"{pairing_link}\"");

    println!(
        "{cyan}{bold}\n╔══════════════════════════════════════════════════════════════╗\n\
         ║                 首次配对（Relay 视角）                  ║\n\
         ╚══════════════════════════════════════════════════════════════╝{reset}",
        cyan = ANSI_CYAN,
        bold = ANSI_BOLD,
        reset = ANSI_RESET
    );
    println!(
        "{white}{bold}Relay WS:{reset} {ws}",
        white = ANSI_WHITE,
        bold = ANSI_BOLD,
        reset = ANSI_RESET,
        ws = relay_ws_url
    );
    println!(
        "{white}{bold}配对码:{reset} {white}{code}{reset}",
        white = ANSI_WHITE,
        bold = ANSI_BOLD,
        reset = ANSI_RESET,
        code = pairing_code
    );
    if let Some(name) = host_name {
        let normalized = name.trim();
        if !normalized.is_empty() {
            println!(
                "{white}{bold}宿主机名:{reset} {name}",
                white = ANSI_WHITE,
                bold = ANSI_BOLD,
                reset = ANSI_RESET,
                name = normalized
            );
        }
    }
    println!(
        "{white}{bold}配对链接:{reset} {link}",
        white = ANSI_WHITE,
        bold = ANSI_BOLD,
        reset = ANSI_RESET,
        link = pairing_link
    );
    println!(
        "{white}{bold}模拟扫码(iOS):{reset} {cmd}\n",
        white = ANSI_WHITE,
        bold = ANSI_BOLD,
        reset = ANSI_RESET,
        cmd = simctl_cmd
    );
}

/// 校验并修正上行 envelope。
fn sanitize_envelope(
    raw: &str,
    system_id: &str,
    source_client_type: &str,
    source_device_id: &str,
) -> Result<String, String> {
    let mut env: Value = serde_json::from_str(raw).map_err(|err| err.to_string())?;
    let obj = env
        .as_object_mut()
        .ok_or_else(|| "envelope must be an object".to_string())?;

    if !obj.contains_key("v") {
        obj.insert("v".to_string(), json!(1));
    }

    let event_type = obj
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if event_type.is_empty() {
        return Err("missing type".to_string());
    }

    if let Some(sid) = obj.get("systemId").and_then(Value::as_str)
        && sid != system_id
    {
        return Err("systemId mismatch".to_string());
    }

    obj.insert("systemId".to_string(), Value::String(system_id.to_string()));
    obj.insert(
        "sourceClientType".to_string(),
        Value::String(source_client_type.to_string()),
    );
    obj.insert(
        "sourceDeviceId".to_string(),
        Value::String(source_device_id.to_string()),
    );
    obj.insert(
        "peerId".to_string(),
        Value::String(source_device_id.to_string()),
    );

    let ts_empty = obj
        .get("ts")
        .and_then(Value::as_str)
        .map(str::trim)
        .map(str::is_empty)
        .unwrap_or(true);
    if ts_empty {
        obj.insert("ts".to_string(), Value::String(now_rfc3339_nanos()));
    }

    if !matches!(obj.get("payload"), Some(v) if v.is_object()) {
        obj.insert("payload".to_string(), json!({}));
    }

    serde_json::to_string(&env).map_err(|err| err.to_string())
}

/// 连接成功后回推 server_presence。
fn send_server_presence(
    tx: &mpsc::UnboundedSender<Message>,
    system_id: &str,
    client_type: &str,
    device_id: &str,
) {
    let env = EventEnvelope::new(
        "server_presence",
        system_id,
        json!({
            "status": "connected",
            "clientType": client_type,
            "deviceId": device_id,
        }),
    );

    if let Ok(raw) = serde_json::to_string(&env) {
        let _ = tx.send(Message::Text(raw.into()));
    }
}

/// 校验短时配对票据。
fn verify_pairing_ticket(
    ticket: &str,
    expected_system_id: &str,
    pair_token: &str,
    used_nonces: &mut HashMap<String, u64>,
    consume: bool,
) -> Result<(), PairTicketError> {
    if ticket.is_empty() {
        return Err(PairTicketError::Empty);
    }

    let mut parts = ticket.split('.');
    let version = parts.next().unwrap_or_default();
    let payload_b64 = parts.next().unwrap_or_default();
    let sig_b64 = parts.next().unwrap_or_default();
    if version != "pct_v1" || payload_b64.is_empty() || sig_b64.is_empty() || parts.next().is_some()
    {
        return Err(PairTicketError::Format);
    }

    let sig = URL_SAFE_NO_PAD
        .decode(sig_b64.as_bytes())
        .map_err(|_| PairTicketError::SignatureFormat)?;

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(pair_token.as_bytes())
        .map_err(|_| PairTicketError::SignatureVerify)?;
    mac.update(payload_b64.as_bytes());
    mac.verify_slice(&sig)
        .map_err(|_| PairTicketError::SignatureVerify)?;

    let payload_raw = URL_SAFE_NO_PAD
        .decode(payload_b64.as_bytes())
        .map_err(|_| PairTicketError::Payload)?;
    let claims: PairTicketClaims =
        serde_json::from_slice(&payload_raw).map_err(|_| PairTicketError::Claims)?;

    if claims.sid != expected_system_id {
        return Err(PairTicketError::SystemMismatch);
    }
    if claims.nonce.trim().is_empty() {
        return Err(PairTicketError::EmptyNonce);
    }

    let now = unix_now();
    if claims.exp <= now {
        return Err(PairTicketError::Expired);
    }
    if claims.iat > now.saturating_add(30) {
        return Err(PairTicketError::IatInvalid);
    }

    used_nonces.retain(|_, exp| exp.saturating_add(30) > now);
    if let Some(exp) = used_nonces.get(&claims.nonce)
        && *exp > now
    {
        return Err(PairTicketError::Replay);
    }

    if consume {
        used_nonces.insert(claims.nonce, claims.exp);
    }

    Ok(())
}

/// pairToken 鉴权决策。
fn authorize_pair_token(
    existing_pair_token: Option<&str>,
    active_client_count: usize,
    client_type: &str,
    incoming_pair_token: &str,
) -> Result<PairTokenAuthDecision, String> {
    if incoming_pair_token.trim().is_empty() {
        return Err("pairToken 不能为空".to_string());
    }

    let Some(existing) = existing_pair_token else {
        if client_type == "sidecar" {
            return Ok(PairTokenAuthDecision::Initialize);
        }
        return Err("system 未注册，请先启动 sidecar 完成配对".to_string());
    };

    if existing == incoming_pair_token {
        return Ok(PairTokenAuthDecision::Allow);
    }

    if client_type == "sidecar" && active_client_count == 0 {
        return Ok(PairTokenAuthDecision::Rotate);
    }

    Err("pairToken 不匹配".to_string())
}

/// 生成 access token。
fn issue_access_token(
    signing_key: &str,
    system_id: &str,
    device_id: &str,
    key_id: &str,
    ttl_sec: u64,
) -> Result<String, ApiError> {
    let now = unix_now();
    let claims = AccessTokenClaims {
        sid: system_id.to_string(),
        did: device_id.to_string(),
        kid: key_id.to_string(),
        iat: now,
        exp: now.saturating_add(ttl_sec),
        jti: Uuid::new_v4().simple().to_string(),
    };
    let payload = serde_json::to_string(&claims).map_err(|err| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            format!("encode access token claims failed: {err}"),
            "请稍后重试",
        )
    })?;
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload.as_bytes());
    let sig_b64 = hmac_b64url(signing_key, payload_b64.as_bytes())?;
    Ok(format!("yat_v1.{payload_b64}.{sig_b64}"))
}

/// 校验 access token。
fn verify_access_token(
    token: &str,
    signing_key: &str,
    expected_system: &str,
    expected_device: &str,
    expected_key_id: &str,
) -> Result<AccessTokenClaims, ApiError> {
    let mut parts = token.split('.');
    let version = parts.next().unwrap_or_default();
    let payload_b64 = parts.next().unwrap_or_default();
    let sig_b64 = parts.next().unwrap_or_default();
    if version != "yat_v1" || payload_b64.is_empty() || sig_b64.is_empty() || parts.next().is_some()
    {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "ACCESS_TOKEN_INVALID",
            "accessToken 格式无效",
            "请重新配对",
        ));
    }

    let sig = URL_SAFE_NO_PAD.decode(sig_b64.as_bytes()).map_err(|_| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "ACCESS_TOKEN_INVALID",
            "accessToken 签名格式无效",
            "请重新配对",
        )
    })?;

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(signing_key.as_bytes()).map_err(|_| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "ACCESS_TOKEN_INVALID",
            "accessToken 签名器无效",
            "请重新配对",
        )
    })?;
    mac.update(payload_b64.as_bytes());
    mac.verify_slice(&sig).map_err(|_| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "ACCESS_TOKEN_INVALID",
            "accessToken 签名校验失败",
            "请重新配对",
        )
    })?;

    let payload_raw = URL_SAFE_NO_PAD
        .decode(payload_b64.as_bytes())
        .map_err(|_| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "ACCESS_TOKEN_INVALID",
                "accessToken payload 无效",
                "请重新配对",
            )
        })?;
    let claims: AccessTokenClaims = serde_json::from_slice(&payload_raw).map_err(|_| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "ACCESS_TOKEN_INVALID",
            "accessToken claims 无效",
            "请重新配对",
        )
    })?;

    if claims.sid != expected_system
        || claims.did != expected_device
        || claims.kid != expected_key_id
    {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "ACCESS_TOKEN_MISMATCH",
            "accessToken 与当前连接信息不匹配",
            "请重新配对",
        ));
    }

    let now = unix_now();
    if claims.exp <= now {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "ACCESS_TOKEN_EXPIRED",
            "accessToken 已过期",
            "请刷新凭证或重新配对",
        ));
    }

    Ok(claims)
}

/// 生成 refresh 会话。
fn issue_refresh_session(
    system_id: &str,
    device_id: &str,
    key_id: &str,
    credential_id: &str,
) -> (String, RefreshSession) {
    let session_id = format!("rs_{}", Uuid::new_v4().simple());
    let secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let token = format!("yrt_v1.{session_id}.{secret}");
    let now = now_rfc3339_nanos();
    (
        token,
        RefreshSession {
            session_id: session_id.clone(),
            system_id: system_id.to_string(),
            device_id: device_id.to_string(),
            key_id: key_id.to_string(),
            credential_id: credential_id.to_string(),
            refresh_secret_hash: sha256_hex(&secret),
            expires_at: unix_now().saturating_add(REFRESH_TOKEN_TTL_SEC),
            created_at: now,
            revoked_at: None,
            rotated_from: None,
        },
    )
}

/// 解析 refresh token（`yrt_v1.<session>.<secret>`）。
fn parse_refresh_token(token: &str) -> Result<(String, String), ApiError> {
    let mut parts = token.split('.');
    let version = parts.next().unwrap_or_default();
    let session = parts.next().unwrap_or_default();
    let secret = parts.next().unwrap_or_default();
    if version != "yrt_v1" || session.is_empty() || secret.is_empty() || parts.next().is_some() {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "REFRESH_TOKEN_INVALID",
            "refreshToken 格式无效",
            "请重新配对",
        ));
    }
    Ok((session.to_string(), secret.to_string()))
}

/// 校验 Ed25519 PoP 签名。
fn verify_pop_signature(
    public_key_b64: &str,
    payload: &str,
    signature_b64: &str,
) -> Result<(), ApiError> {
    let pk_raw = URL_SAFE_NO_PAD
        .decode(public_key_b64.as_bytes())
        .map_err(|_| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "PAIR_PROOF_INVALID",
                "设备公钥格式无效",
                "请重新生成设备绑定信息",
            )
        })?;
    let pk_bytes: [u8; 32] = pk_raw.try_into().map_err(|_| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "PAIR_PROOF_INVALID",
            "设备公钥长度无效",
            "请重新生成设备绑定信息",
        )
    })?;

    let sig_raw = URL_SAFE_NO_PAD
        .decode(signature_b64.as_bytes())
        .map_err(|_| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "PAIR_PROOF_INVALID",
                "签名格式无效",
                "请重试",
            )
        })?;
    let sig_bytes: [u8; 64] = sig_raw.try_into().map_err(|_| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "PAIR_PROOF_INVALID",
            "签名长度无效",
            "请重试",
        )
    })?;

    let verifying_key = VerifyingKey::from_bytes(&pk_bytes).map_err(|_| {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "PAIR_PROOF_INVALID",
            "设备公钥无法解析",
            "请重新生成设备绑定信息",
        )
    })?;

    let signature = Signature::from_bytes(&sig_bytes);
    verifying_key
        .verify(payload.as_bytes(), &signature)
        .map_err(|_| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "PAIR_PROOF_INVALID",
                "签名校验失败",
                "请重试或重新配对",
            )
        })
}

/// 计算 keyId。
fn key_id_for_public_key(public_key_b64: &str) -> Result<String, ApiError> {
    let pk_raw = URL_SAFE_NO_PAD
        .decode(public_key_b64.as_bytes())
        .map_err(|_| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "PAIR_PROOF_INVALID",
                "设备公钥格式无效",
                "请重新生成设备绑定信息",
            )
        })?;
    let digest = Sha256::digest(pk_raw);
    Ok(format!("kid_{}", URL_SAFE_NO_PAD.encode(&digest[..10])))
}

/// HMAC-SHA256 并输出 base64url。
fn hmac_b64url(secret: &str, payload: &[u8]) -> Result<String, ApiError> {
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            "签名密钥无效",
            "请稍后重试",
        )
    })?;
    mac.update(payload);
    Ok(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

/// pairTicket 错误映射到 API 错误。
fn pair_ticket_error_to_api(err: PairTicketError) -> ApiError {
    match err {
        PairTicketError::Empty => ApiError::new(
            StatusCode::BAD_REQUEST,
            "MISSING_CREDENTIALS",
            "缺少配对票据",
            "请重新扫码",
        ),
        PairTicketError::Format
        | PairTicketError::SignatureFormat
        | PairTicketError::SignatureVerify
        | PairTicketError::Payload
        | PairTicketError::Claims
        | PairTicketError::SystemMismatch
        | PairTicketError::EmptyNonce
        | PairTicketError::IatInvalid => ApiError::new(
            StatusCode::UNAUTHORIZED,
            "PAIR_TICKET_INVALID",
            "配对票据无效",
            "请重新扫码获取最新配对信息",
        ),
        PairTicketError::Expired => ApiError::new(
            StatusCode::UNAUTHORIZED,
            "PAIR_TICKET_EXPIRED",
            "配对票据已过期",
            "请重新扫码获取最新二维码",
        ),
        PairTicketError::Replay => ApiError::new(
            StatusCode::UNAUTHORIZED,
            "PAIR_TICKET_REPLAYED",
            "配对票据已使用",
            "请重新扫码获取最新二维码",
        ),
    }
}

/// 解析秒级时间戳。
fn parse_ts(raw: &str, code: &'static str, message: &'static str) -> Result<u64, ApiError> {
    raw.trim()
        .parse::<u64>()
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, code, message, "请刷新后重试"))
}

/// 校验时间窗。
fn verify_ts_window(ts: u64, code: &'static str, message: &'static str) -> Result<(), ApiError> {
    let now = unix_now();
    if ts.saturating_add(POP_MAX_SKEW_SEC) < now || ts > now.saturating_add(POP_MAX_SKEW_SEC) {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            code,
            message,
            "请重新发起请求",
        ));
    }
    Ok(())
}

/// 组装 WS PoP 签名 payload。
fn ws_pop_payload(system_id: &str, device_id: &str, key_id: &str, ts: u64, nonce: &str) -> String {
    format!("ws\n{system_id}\n{device_id}\n{key_id}\n{ts}\n{nonce}")
}

/// 组装 exchange proof payload。
fn pair_exchange_payload(system_id: &str, device_id: &str, key_id: &str) -> String {
    format!("pair-exchange\n{system_id}\n{device_id}\n{key_id}")
}

/// 组装 refresh 签名 payload。
fn auth_refresh_payload(
    system_id: &str,
    device_id: &str,
    key_id: &str,
    ts: u64,
    nonce: &str,
) -> String {
    format!("auth-refresh\n{system_id}\n{device_id}\n{key_id}\n{ts}\n{nonce}")
}

/// 组装 revoke 签名 payload。
fn auth_revoke_payload(
    system_id: &str,
    device_id: &str,
    target_device_id: &str,
    key_id: &str,
    ts: u64,
    nonce: &str,
) -> String {
    format!(
        "auth-revoke\n{system_id}\n{device_id}\n{target_device_id}\n{key_id}\n{ts}\n{nonce}"
    )
}

/// 组装 list-devices 签名 payload。
fn auth_list_payload(
    system_id: &str,
    device_id: &str,
    key_id: &str,
    ts: u64,
    nonce: &str,
) -> String {
    format!("auth-list-devices\n{system_id}\n{device_id}\n{key_id}\n{ts}\n{nonce}")
}

/// 归一化设备名称。
fn normalize_device_name(raw: &str, fallback: &str) -> String {
    let normalized = raw.trim();
    if normalized.is_empty() {
        return fallback.to_string();
    }
    normalized.chars().take(64).collect()
}

/// sha256 hex。
fn sha256_hex(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// 当前 unix 秒。
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 认证存储路径。
fn auth_store_path() -> PathBuf {
    if let Ok(path) = std::env::var("RELAY_AUTH_STORE_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("yourconnector")
        .join("relay")
        .join("auth-store.json")
}

/// 加载认证元数据。
fn load_auth_store(path: &Path) -> Result<AuthStore, String> {
    if !path.exists() {
        return Ok(AuthStore::new());
    }
    let raw = fs::read(path).map_err(|err| format!("read auth store failed: {err}"))?;
    let mut parsed: AuthStore =
        serde_json::from_slice(&raw).map_err(|err| format!("decode auth store failed: {err}"))?;
    if parsed.signing_key.trim().is_empty() {
        parsed.signing_key = generate_signing_key_seed();
    }
    Ok(parsed)
}

/// 持久化认证元数据。
fn persist_auth_store(path: &Path, store: &AuthStore) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create auth store dir failed: {err}"))?;
    }
    let encoded = serde_json::to_vec_pretty(store)
        .map_err(|err| format!("encode auth store failed: {err}"))?;
    fs::write(path, encoded).map_err(|err| format!("write auth store failed: {err}"))
}

/// 生成 relay 自身 token 签名种子。
fn generate_signing_key_seed() -> String {
    format!(
        "relay_sk_{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    )
}

#[cfg(test)]
mod tests {
    use super::{
        PairTokenAuthDecision, auth_list_payload, auth_refresh_payload, auth_revoke_payload,
        authorize_pair_token, generate_pairing_ticket, pair_exchange_payload, sanitize_envelope,
        verify_pairing_ticket, ws_pop_payload,
    };
    use std::collections::HashMap;

    #[test]
    fn sanitize_envelope_injects_trusted_source_fields() {
        let raw = r#"{"type":"tool_connect_request","payload":{"toolId":"opencode_x"}}"#;
        let sanitized = sanitize_envelope(raw, "sys_test", "app", "ios_device_1")
            .expect("sanitize should work");
        let value: serde_json::Value =
            serde_json::from_str(&sanitized).expect("sanitized payload should be valid json");

        assert_eq!(
            value.get("systemId").and_then(|v| v.as_str()),
            Some("sys_test")
        );
        assert_eq!(
            value.get("sourceClientType").and_then(|v| v.as_str()),
            Some("app")
        );
        assert_eq!(
            value.get("sourceDeviceId").and_then(|v| v.as_str()),
            Some("ios_device_1")
        );
        assert_eq!(
            value.get("peerId").and_then(|v| v.as_str()),
            Some("ios_device_1")
        );
        assert!(value.get("payload").and_then(|v| v.as_object()).is_some());
    }

    #[test]
    fn sanitize_envelope_rejects_mismatched_system_id() {
        let raw = r#"{"type":"tool_connect_request","systemId":"sys_wrong","payload":{"toolId":"opencode_x"}}"#;
        let result = sanitize_envelope(raw, "sys_test", "app", "ios_device_1");
        assert!(result.is_err());
    }

    #[test]
    fn authorize_pair_token_allows_matching_token() {
        let decision = authorize_pair_token(Some("ptk_a"), 2, "app", "ptk_a")
            .expect("matching token should pass");
        assert_eq!(decision, PairTokenAuthDecision::Allow);
    }

    #[test]
    fn authorize_pair_token_rejects_mismatch_for_app() {
        let result = authorize_pair_token(Some("ptk_a"), 1, "app", "ptk_b");
        assert!(result.is_err());
    }

    #[test]
    fn authorize_pair_token_allows_sidecar_rotate_when_room_idle() {
        let decision = authorize_pair_token(Some("ptk_old"), 0, "sidecar", "ptk_new")
            .expect("sidecar should rotate token when room is idle");
        assert_eq!(decision, PairTokenAuthDecision::Rotate);
    }

    #[test]
    fn verify_pairing_ticket_accepts_valid_ticket() {
        let ticket = generate_pairing_ticket("sys_t", "ptk_t", 300);
        let mut used = HashMap::new();
        verify_pairing_ticket(&ticket, "sys_t", "ptk_t", &mut used, true)
            .expect("valid ticket should pass");
    }

    #[test]
    fn verify_pairing_ticket_rejects_replay() {
        let ticket = generate_pairing_ticket("sys_t", "ptk_t", 300);
        let mut used = HashMap::new();
        verify_pairing_ticket(&ticket, "sys_t", "ptk_t", &mut used, true)
            .expect("first consume should pass");
        let result = verify_pairing_ticket(&ticket, "sys_t", "ptk_t", &mut used, true);
        assert!(result.is_err());
    }

    #[test]
    fn verify_pairing_ticket_rejects_wrong_system() {
        let ticket = generate_pairing_ticket("sys_a", "ptk_t", 300);
        let mut used = HashMap::new();
        let result = verify_pairing_ticket(&ticket, "sys_b", "ptk_t", &mut used, true);
        assert!(result.is_err());
    }

    #[test]
    fn pop_payloads_use_real_newline_separator() {
        let ws = ws_pop_payload("sys_x", "ios_y", "kid_z", 123, "n1");
        assert_eq!(ws, "ws\nsys_x\nios_y\nkid_z\n123\nn1");

        let exchange = pair_exchange_payload("sys_x", "ios_y", "kid_z");
        assert_eq!(exchange, "pair-exchange\nsys_x\nios_y\nkid_z");

        let refresh = auth_refresh_payload("sys_x", "ios_y", "kid_z", 123, "n1");
        assert_eq!(refresh, "auth-refresh\nsys_x\nios_y\nkid_z\n123\nn1");

        let revoke = auth_revoke_payload("sys_x", "ios_y", "ios_t", "kid_z", 123, "n1");
        assert_eq!(revoke, "auth-revoke\nsys_x\nios_y\nios_t\nkid_z\n123\nn1");

        let list = auth_list_payload("sys_x", "ios_y", "kid_z", 123, "n1");
        assert_eq!(list, "auth-list-devices\nsys_x\nios_y\nkid_z\n123\nn1");
    }
}
