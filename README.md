# yourConnector

## 项目结构

1. `/Users/codez/develop/yourConnector/app/mobile`：Tauri Mobile App（当前 iOS）
2. `/Users/codez/develop/yourConnector/services/relay`：Relay 服务（Rust）
3. `/Users/codez/develop/yourConnector/services/sidecar`：Sidecar 服务（Rust）
4. `/Users/codez/develop/yourConnector/protocol/rust`：共享协议类型（Rust）
5. `/Users/codez/develop/yourConnector/docs`：设计、治理、验收文档

## 核心命令

```bash
cd /Users/codez/develop/yourConnector

# 服务启动
make run-relay
make run-sidecar

# iOS 启动
make run-mobile-tauri-ios IOS_SIM="iPhone 17 Pro"

# 配对辅助
make show-pairing
make show-pairing-link
make simulate-ios-scan
```

## Linux 分发脚本（v3.3）

```bash
# 网关+执行机（一体节点）
sudo bash /path/to/relay-sidecar.sh install \
  --version vX.Y.Z \
  --acme-email you@example.com

# 执行机（连接远端 Relay）
sudo bash /path/to/sidecar.sh install \
  --version vX.Y.Z \
  --relay wss://<公网IPv4>/v1/ws
```

说明：

1. 公网默认仅支持 `wss://.../v1/ws`，`ws://` 仅用于开发调试开关。
2. 一体节点脚本强制使用 Let’s Encrypt shortlived IP 证书（HTTP-01 + webroot）。
3. 详细参数、卸载与 `doctor/status` 见 `/Users/codez/develop/yourConnector/docs/分发安装与卸载-v1.md`。
4. GitHub Actions 已支持“打 tag 自动构建发布资产（amd64）”，工作流见 `/Users/codez/develop/yourConnector/.github/workflows/release-linux.yml`。

## 质量门禁

```bash
cd /Users/codez/develop/yourConnector

# 治理门禁（注释/行长/文档一致性）
make check-governance

# 全量门禁（编译、格式、静态检查、测试、JS 语法、治理）
make check-all
```

## 系统日志

1. Relay 与 Sidecar 启动后会同时写入 stdout 与文件日志。
2. 默认目录：
   - 原始日志：`/Users/codez/develop/yourConnector/logs/raw`
   - 每日归档：`/Users/codez/develop/yourConnector/logs/archive`
3. 归档规则：
   - 文件按天命名：`<service>.log.YYYY-MM-DD`
   - 已完成日期会自动打包为 `YYYY-MM-DD.7z`
4. 常用环境变量：
   - `YC_LOG_DIR`：日志根目录（默认 `logs`）
   - `YC_LOG_ARCHIVE_INTERVAL_SEC`：归档轮询周期（默认 `3600` 秒）
   - `YC_FILE_LOG_LEVEL`：文件日志级别（默认 `debug`）
   - `RUST_LOG`：仅影响 stdout 级别，不影响文件日志
5. 配对信息（配对码、配对链接、模拟扫码命令）会以高亮 banner 直接输出到终端，便于现场配对。
6. 详细说明见：`/Users/codez/develop/yourConnector/docs/系统日志与归档-v1.md`

## 文档入口

1. `/Users/codez/develop/yourConnector/docs/文档导航-v2.md`
2. `/Users/codez/develop/yourConnector/docs/代码治理与注释规范-v1.md`
3. `/Users/codez/develop/yourConnector/docs/质量门禁与检查规范-v1.md`
4. `/Users/codez/develop/yourConnector/docs/分发安装与卸载-v1.md`
5. `/Users/codez/develop/yourConnector/docs/系统日志与归档-v1.md`
6. `/Users/codez/develop/yourConnector/docs/已完成功能验收-v1.md`
7. `/Users/codez/develop/yourConnector/docs/工具接入核心组件-v1.md`
