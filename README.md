# yourConnector

## 目录结构

- `app/mobile`：唯一 App（Tauri Mobile，当前 iOS）
- `services/relay`：relay 服务（Rust）
- `services/sidecar`：sidecar 服务（Rust）
- `protocol/rust`：协议 Rust 类型定义（唯一代码源）
- `protocol/schema`：协议 JSON Schema（与 Rust 协议对齐）
- `docs`：项目文档

## 常用命令

```bash
cd /Users/codez/develop/yourConnector

# 编译检查（服务与协议）
cargo check --workspace

# 启动 relay / sidecar
make run-relay
make run-sidecar

# 读取配对码（不需要翻日志）
make show-pairing-code

# 读取扫码链接（默认 code+sid+ticket）
make show-pairing-link

# 一次输出配对码+链接+终端二维码（推荐）
make show-pairing

# 多参数配对命令（可覆盖 relay / 名称 / 输出模式 / 票据 TTL）
make pairing PAIR_ARGS="--show all --name 我的Mac --relay ws://127.0.0.1:18080/v1/ws"

# 显式声明附带 code（默认即附带）
make pairing PAIR_ARGS="--show link --include-code"

# 仅输出 sid+ticket（不附带 code）
make pairing PAIR_ARGS="--show link --no-code"

# 开发态模拟扫码（向 iOS 模拟器投递 yc://pair 链接）
make simulate-ios-scan

# 启动 iOS 移动端
make run-mobile-tauri-ios IOS_SIM="iPhone 17 Pro"

# 需要热更新时，使用 dev 模式（依赖本地网络权限）
make run-mobile-tauri-ios-dev IOS_SIM="iPhone 17 Pro"

# 若 dev 模式出现空白页，先清理重装再启动
make run-mobile-tauri-ios-dev-clean IOS_SIM="iPhone 17 Pro"

# 若模拟器服务异常（白屏/黑屏/启动卡住），先修复再重启
make repair-ios-sim IOS_SIM="iPhone 17 Pro"
```

`dev` 模式出现本地网络权限提示时，需要在 iOS `设置 > 隐私与安全性 > 本地网络` 里允许 `yourConnector Mobile`，然后重启 App。

## 配对说明（新）

- 默认看 sidecar/relay 启动日志里的高亮“首次配对”区块（配对码 + 配对链接）
- 配对链接包含短时票据（`sid + ticket`），并保留 `code` 兼容重连
- `make show-pairing` 作为主命令（含二维码）
- `make show-pairing-code` / `make show-pairing-link` 作为兜底
- `make show-pairing-qr` 可随时重新展示终端二维码
- 终端二维码依赖 `qrencode`（可选安装：`brew install qrencode`）
- iOS 开发调试可用 `make simulate-ios-scan` 模拟二维码扫描投递
- iOS App 连接时需要填写：
  - `Relay WS URL`
  - `配对码（systemId.pairToken）`（或扫码/链接自动导入）
- 若手机重装导致控制命令被拒绝，可在 App 调试页点击“绑定当前设备为控制端”进行重绑。
- `systemId` 与 `pairToken` 由 sidecar 本地持久化生成，默认路径：
  - `~/.config/yourconnector/sidecar/system-id.txt`
  - `~/.config/yourconnector/sidecar/pair-token.txt`

## 里程碑与验收

- 里程碑与待办：`/Users/codez/develop/yourConnector/docs/里程碑与待办-v1.md`
- 已完成功能验收：`/Users/codez/develop/yourConnector/docs/已完成功能验收-v1.md`
