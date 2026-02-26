# yourConnector API 与事件协议

## 1. 协议总览

1. HTTP：用于配对签发、凭证换发、凭证刷新、设备吊销与设备列表。
2. WebSocket：用于 App 与 Sidecar 的实时事件通信。
3. 协议 envelope 定义位于 `protocol/rust/src/lib.rs`。

## 2. Relay HTTP API

### 2.1 路由清单

1. `GET /healthz`：健康检查。
2. `GET /v1/debug/systems`：调试接口，返回每个 `systemId` 在线连接数。
3. `POST /v1/pair/bootstrap`：签发 `yc://pair` 链接与 `pairTicket`。
4. `POST /v1/pair/preflight`：配对预检（不消费票据）。
5. `POST /v1/pair/exchange`：配对换发（消费票据）。
6. `POST /v1/auth/refresh`：刷新设备凭证（轮换 refresh）。
7. `POST /v1/auth/revoke-device`：吊销设备。
8. `GET /v1/auth/devices`：查询设备列表。
9. `GET /v1/ws`：WebSocket 握手入口。

### 2.2 关键请求/响应字段

1. `/v1/pair/bootstrap` 请求：`systemId`、`pairToken`、`hostName?`、`relayWsUrl?`、`includeCode?`、`ttlSec?`。
2. `/v1/pair/bootstrap` 响应：`pairLink`、`pairTicket`、`relayWsUrl`、`systemId`、`hostName`、`pairCode?`、`simctlCommand`。
3. `/v1/pair/preflight` 请求：`systemId`、`deviceId`、`pairTicket`。
4. `/v1/pair/exchange` 请求：`systemId`、`deviceId`、`deviceName`、`pairTicket`、`keyId`、`devicePubKey`、`proof`。
5. `/v1/pair/exchange` 响应：`accessToken`、`refreshToken`、`keyId`、`credentialId`、`accessExpiresInSec`、`refreshExpiresInSec`。

## 3. 鉴权约束

### 3.1 App 链路

1. 仅支持 `accessToken + PoP` 连接 `/v1/ws`。
2. 握手签名 payload：`ws\n{systemId}\n{deviceId}\n{keyId}\n{ts}\n{nonce}`。
3. `pairToken` 或 `pairTicket` 直连 WS 会被拒绝（`PAIR_TOKEN_NOT_SUPPORTED`）。

### 3.2 Sidecar 链路

1. Sidecar 使用 `pairToken` 连接 WS。
2. `systemId` 首次上线可初始化 room，后续按 `pairToken` 校验或轮换策略处理。

### 3.3 时效默认值

1. `pairTicket` 默认 TTL：`300s`（范围 `30-3600`）。
2. `accessToken` TTL：`600s`。
3. `refreshToken` TTL：`30天`。
4. PoP 时间窗：`120s`。

## 4. WebSocket Envelope

事件公共字段：

1. `v`：协议版本。
2. `eventId`：事件唯一标识。
3. `traceId`：链路追踪标识。
4. `type`：事件类型。
5. `systemId`：宿主机标识。
6. `seq`：序号（可选）。
7. `ts`：事件时间。
8. `payload`：事件载荷。

## 5. 事件矩阵

### 5.1 Sidecar -> App

1. `heartbeat`
2. `tools_snapshot`
3. `tools_candidates`
4. `metrics_snapshot`
5. `tool_details_snapshot`
6. `tool_whitelist_updated`
7. `tool_process_control_updated`
8. `controller_bind_updated`
9. `tool_chat_started`
10. `tool_chat_chunk`
11. `tool_chat_finished`
12. `tool_report_fetch_started`
13. `tool_report_fetch_chunk`
14. `tool_report_fetch_finished`

### 5.2 App -> Sidecar

1. `tools_refresh_request`
2. `tool_connect_request`
3. `tool_disconnect_request`
4. `tool_whitelist_reset_request`
5. `tool_details_refresh_request`
6. `tool_process_control_request`
7. `controller_rebind_request`
8. `tool_chat_request`
9. `tool_chat_cancel_request`
10. `tool_report_fetch_request`

## 6. 常见错误码

1. `PAIR_TOKEN_NOT_SUPPORTED`
2. `PAIR_TICKET_INVALID`
3. `PAIR_TICKET_EXPIRED`
4. `PAIR_TICKET_REPLAYED`
5. `SYSTEM_NOT_REGISTERED`
6. `ACCESS_TOKEN_INVALID`
7. `ACCESS_TOKEN_EXPIRED`
8. `ACCESS_SIGNATURE_EXPIRED`
9. `ACCESS_SIGNATURE_REPLAYED`
10. `DEVICE_REVOKED`

## 7. 参考代码

1. Relay 路由：`services/relay/src/app.rs`
2. 配对与票据：`services/relay/src/pairing/*`
3. 鉴权：`services/relay/src/auth/*`、`services/relay/src/ws/handlers/auth.rs`
4. Sidecar 控制事件常量：`services/sidecar/src/control.rs`
5. Sidecar 会话事件发送：`services/sidecar/src/session/snapshots.rs`、`services/sidecar/src/session/loop/*`
