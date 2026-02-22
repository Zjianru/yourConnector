# yourConnector

## 项目结构

- `/Users/codez/develop/yourConnector/app/mobile`：唯一 App（Tauri Mobile，当前 iOS）
- `/Users/codez/develop/yourConnector/services/relay`：relay 服务（Rust）
- `/Users/codez/develop/yourConnector/services/sidecar`：sidecar 服务（Rust）
- `/Users/codez/develop/yourConnector/protocol/rust`：协议 Rust 类型定义（唯一代码源）
- `/Users/codez/develop/yourConnector/protocol/schema`：协议 JSON Schema（与 Rust 协议对齐）
- `/Users/codez/develop/yourConnector/docs`：项目文档

## 快速开始

```bash
cd /Users/codez/develop/yourConnector
make help
make check
make run-relay
make run-sidecar
make show-pairing
make run-mobile-tauri-ios IOS_SIM="iPhone 17 Pro"
```

## 核心命令

```bash
# 服务
make run-relay
make run-sidecar

# iOS
make run-mobile-tauri-ios IOS_SIM="iPhone 17 Pro"
make run-mobile-tauri-ios-dev IOS_SIM="iPhone 17 Pro"
make run-mobile-tauri-ios-dev-clean IOS_SIM="iPhone 17 Pro"
make repair-ios-sim IOS_SIM="iPhone 17 Pro"

# 配对（relay 统一签发）
make pairing PAIR_ARGS="--show all"
make show-pairing
make show-pairing-link
make show-pairing-code
make simulate-ios-scan
```

## 配对链路说明

1. `pairTicket` 与 `yc://pair` 链接统一由 relay `POST /v1/pair/bootstrap` 签发。
2. sidecar 启动后会请求 relay 签发并在终端高亮展示配对信息。
3. `scripts/pairing.sh` 也调用 relay 签发接口，避免脚本与 sidecar 链接不一致。
4. App 配对与 WS 握手不再接受 `pairToken`；`pairToken` 仅用于 sidecar 与 relay 鉴权。

## 文档入口

- `/Users/codez/develop/yourConnector/docs/文档导航-v2.md`
- `/Users/codez/develop/yourConnector/docs/已完成功能验收-v1.md`
- `/Users/codez/develop/yourConnector/docs/里程碑与待办-v1.md`
