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

## 质量门禁

```bash
cd /Users/codez/develop/yourConnector

# 治理门禁（注释/行长/文档一致性）
make check-governance

# 全量门禁（编译、格式、静态检查、测试、JS 语法、治理）
make check-all
```

## 文档入口

1. `/Users/codez/develop/yourConnector/docs/文档导航-v2.md`
2. `/Users/codez/develop/yourConnector/docs/代码治理与注释规范-v1.md`
3. `/Users/codez/develop/yourConnector/docs/质量门禁与检查规范-v1.md`
4. `/Users/codez/develop/yourConnector/docs/已完成功能验收-v1.md`
5. `/Users/codez/develop/yourConnector/docs/工具接入核心组件-v1.md`
