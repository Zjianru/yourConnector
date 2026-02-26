# yourConnector CLI 与环境变量

## 1. 文档目标

1. 统一说明 `yc-relay`、`yc-sidecar` 与分发脚本的命令口径。
2. 统一说明运行时环境变量，避免部署参数漂移。

## 2. 二进制 CLI

### 2.1 `yc-relay`

1. `yc-relay run`
2. `yc-relay status`
3. `yc-relay doctor [--format text|json]`
4. `yc-relay service <start|stop|restart|status>`
5. `yc-relay version`

### 2.2 `yc-sidecar`

1. `yc-sidecar run`
2. `yc-sidecar status`
3. `yc-sidecar doctor [--format text|json]`
4. `yc-sidecar service <start|stop|restart|status>`
5. `yc-sidecar version`
6. `yc-sidecar relay [set|-change|test|reset]`
7. `yc-sidecar pairing show [--format text|json|link|qr] [--relay <wss-url>] [--allow-insecure-ws]`

## 3. 分发脚本 CLI

### 3.1 `scripts/dist/yc-relay.sh`

1. 命令：`install|uninstall|status|doctor|start|stop|restart`。
2. 关键参数：`--acme-email`、`--public-ip`、`--asset-base`、`--acme-staging`、`--keep-data`、`--yes`、`--dry-run`、`--format`。

### 3.2 `scripts/dist/yc-sidecar.sh`

1. 命令：`install|uninstall|status|doctor|start|stop|restart`。
2. 关键参数：`--relay-ip`、`--relay`、`--allow-insecure-ws`、`--asset-base`、`--keep-data`、`--yes`、`--dry-run`、`--format`。

## 4. Makefile 常用入口

1. 服务：`make run-relay`、`make run-sidecar`。
2. iOS：`make run-mobile-tauri-ios`、`make run-mobile-tauri-ios-dev`。
3. Android：`make init-mobile-tauri-android`、`make run-mobile-tauri-android-dev`。
4. 配对：`make show-pairing`、`make show-pairing-link`、`make simulate-ios-scan`、`make simulate-android-scan`。
5. 质量：`make check-governance`、`make check-all`。

## 5. Relay 环境变量

1. `RELAY_ADDR`：监听地址，默认 `0.0.0.0:18080`。
2. `RELAY_PUBLIC_WS_URL`：对外公开的 Relay WS 地址（配对签发使用）。
3. `RUST_LOG`：stdout 日志过滤。
4. `YC_FILE_LOG_LEVEL`：文件日志级别，默认 `debug`。
5. `YC_LOG_DIR`：日志根目录，默认 `logs`。
6. `YC_LOG_ARCHIVE_INTERVAL_SEC`：归档周期秒数，默认 `3600`。

说明：脚本部署（Linux/macOS）会把 `RELAY_ADDR` 设为 `127.0.0.1:18080`，由 nginx 对外暴露 `443`。

## 6. Sidecar 环境变量

### 6.1 连接与身份

1. `RELAY_WS_URL`：Relay WS 地址，默认 `ws://127.0.0.1:18080/v1/ws`。
2. `SYSTEM_ID`、`PAIR_TOKEN`、`DEVICE_ID`、`HOST_NAME`。
3. `YC_ALLOW_INSECURE_WS`：允许非回环 `ws://`（仅 debug/research 构建）。
4. `YC_BUILD_CHANNEL`：构建渠道标记（`research` 时可配合放开不安全 ws）。

### 6.2 控制与授权

1. `CONTROLLER_DEVICE_IDS`：预授权控制端设备列表（CSV）。
2. `ALLOW_FIRST_CONTROLLER_BIND`：是否允许首个 App 自动绑定控制端。

### 6.3 周期与详情采集

1. `SIDECAR_ADDR`：健康检查监听地址，默认 `0.0.0.0:18081`。
2. `HEARTBEAT_INTERVAL_SEC`：心跳周期，默认 `5`。
3. `METRICS_INTERVAL_SEC`：快照周期，默认 `10`。
4. `PAIRING_BANNER_REFRESH_SEC`：配对 Banner 刷新，默认 `120`。
5. `DETAILS_INTERVAL_SEC`：详情周期，默认 `45`。
6. `DETAILS_REFRESH_DEBOUNCE_SEC`：详情去抖，默认 `3`。
7. `DETAILS_COMMAND_TIMEOUT_MS`：详情命令超时，默认 `8000`。
8. `DETAILS_MAX_PARALLEL`：详情并发上限，默认 `2`。
9. `FALLBACK_TOOL_ENABLED`：是否启用 fallback 工具占位。

### 6.4 日志

1. `YC_DEBUG_RAW_PAYLOAD`：是否打印原始协议 payload。
2. `RUST_LOG`、`YC_FILE_LOG_LEVEL`、`YC_LOG_DIR`、`YC_LOG_ARCHIVE_INTERVAL_SEC`。

## 7. 参考代码

1. Relay CLI：`services/relay/src/cli/mod.rs`
2. Sidecar CLI：`services/sidecar/src/cli/mod.rs`
3. Sidecar 配置：`services/sidecar/src/config.rs`
4. 分发脚本：`scripts/dist/yc-relay.sh`、`scripts/dist/yc-sidecar.sh`
5. 本地命令入口：`Makefile`
