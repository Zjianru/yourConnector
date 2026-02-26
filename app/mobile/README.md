# Mobile App（Tauri iOS + Android）

## 1. 角色与边界

移动端负责三类能力：

1. 多宿主接入与运维管理（配对、连接、工具接入、详情查看）。
2. 聊天会话与报告查看（按宿主机 + 工具分会话）。
3. 调试与链路排障（结构化日志、事件收发观察）。

## 2. 启动与调试

```bash
cd yourConnector

# 先启动 Relay / Sidecar
make run-relay
make run-sidecar

# iOS 启动（推荐）
make run-mobile-tauri-ios IOS_SIM="iPhone 17 Pro"

# iOS 开发模式
make run-mobile-tauri-ios-dev IOS_SIM="iPhone 17 Pro"
make run-mobile-tauri-ios-dev-clean IOS_SIM="iPhone 17 Pro"
make repair-ios-sim IOS_SIM="iPhone 17 Pro"

# Android
make init-mobile-tauri-android
make run-mobile-tauri-android-dev ANDROID_DEVICE="<device>"
```

## 3. 配对调试辅助

```bash
cd yourConnector

make show-pairing
make show-pairing-link
make simulate-ios-scan
make simulate-android-scan ANDROID_DEVICE="emulator-5554"
```

## 4. 前端模块结构

1. `app/mobile/ui/index.html`：页面骨架。
2. `app/mobile/ui/styles/*`：样式层。
3. `app/mobile/ui/js/main.js`：装配入口。
4. `app/mobile/ui/js/state/*`：状态层。
5. `app/mobile/ui/js/services/*`：平台与网络服务。
6. `app/mobile/ui/js/flows/*`：业务流程编排。
7. `app/mobile/ui/js/views/*`：视图渲染。
8. `app/mobile/ui/js/modals/*`：弹窗与详情面板。

## 5. 关键链路文件

### 5.1 工具详情

1. `app/mobile/ui/js/modals/tool-detail.js`：按 `schema` 渲染 `openclaw.v1` / `opencode.v1`。
2. `app/mobile/ui/js/flows/connection-events.js`：消费 `tool_details_snapshot`。
3. `app/mobile/ui/js/flows/connection-send.js`：发送 `tool_details_refresh_request`。

### 5.2 聊天

1. `app/mobile/ui/js/flows/chat.js`：会话、消息队列、取消、持久化联动。
2. `app/mobile/ui/js/views/chat.js`：聊天列表/详情/完整消息视图。
3. `app/mobile/ui/js/state/chat.js`：聊天状态模型与会话键。

### 5.3 报告查看

1. `app/mobile/ui/js/modals/report-viewer.js`：报告弹窗、进度与 Markdown 渲染。
2. `app/mobile/ui/js/utils/markdown.js`：报告路径识别与 Markdown 工具。

## 6. Tauri 原生命令

原生命令定义在 `app/mobile/src-tauri/src/lib.rs`，分两组：

1. 凭证命令：`auth_get_device_binding`、`auth_sign_payload`、`auth_store_session`、`auth_load_session`、`auth_clear_session`。
2. 聊天存储命令：`chat_store_bootstrap`、`chat_store_append_events`、`chat_store_load_conversation`、`chat_store_upsert_index`、`chat_store_delete_conversation`。

安全存储策略：

1. iOS/macOS：Keychain。
2. Android：`SecureStoreBridge`。
3. 其他平台：仅开发态内存兜底。

## 7. 本地检查

```bash
cd yourConnector
find app/mobile/ui/js -name '*.js' -print0 | xargs -0 -I{} sh -c 'node --check "$$1" && node --check --input-type=module < "$$1"' _ "{}"
```

## 8. 调试日志

1. 页面保留文本日志（`Logs`）与结构化日志（`operationLogs`）双轨。
2. 结构化日志包含 `traceId/eventId/eventType/hostId/toolId` 等字段。
3. 调试页支持“复制结构化日志”导出 JSON 以便回归与排障。

## 9. 运行约束（代码事实）

1. 自动重连策略：固定 `2s` 间隔，最多 `5` 次，超限后进入手动重连。
2. 聊天队列上限：每会话 `20` 条（含运行中请求）。
3. App 配对不支持 `pairToken` 直连，必须使用 `pairTicket` 换发设备凭证。
4. 报告查看只处理 Sidecar 回传的工作区内绝对路径 `.md` 文件流。
