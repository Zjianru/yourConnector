# 贡献指南（Contributing）

感谢你参与 `yourConnector`。

## 1. 适用范围

本指南适用于以下目录的代码与文档变更：

1. `app/mobile`
2. `services/relay`
3. `services/sidecar`
4. `protocol/rust`
5. `scripts`
6. `docs`

## 2. 开发前准备

1. 安装 Rust / Cargo。
2. 安装 Node.js（用于前端语法检查）。
3. 如需移动端调试：安装 Xcode（iOS）或 Android SDK（Android）。

## 3. 本地启动（最小闭环）

```bash
make run-relay
make run-sidecar
make run-mobile-tauri-ios IOS_SIM="iPhone 17 Pro"
```

## 4. 提交前必须执行

```bash
make check-governance
make check-all
```

说明：

1. `check-governance` 会检查注释规范、行长、文档一致性。
2. `check-all` 会执行编译、格式、静态检查、测试、前端语法检查与治理门禁。

## 5. 变更要求

1. 任何协议或事件变更，必须同步更新文档：
   - `docs/API与事件协议.md`
   - `docs/配对与宿主机接入/02-协议与安全方案.md`
2. 任何 CLI 或脚本参数变更，必须同步更新：
   - `docs/CLI与环境变量.md`
   - `docs/分发安装与卸载.md`
3. 文档命名采用单版本，不新增 `v1/v2` 文件。
4. 所有文档正文使用中文（特殊名词保留英文）。
5. 新增或重命名关键源码文件时，必须同步更新 `docs/代码事实总索引.md`。

## 6. Pull Request 建议结构

1. 变更目的与范围。
2. 关键实现点（按模块列出）。
3. 回归与验证命令结果。
4. 文档更新清单。
5. 风险与回滚方案（如有）。

## 7. 常见拒绝原因

1. 未通过 `make check-all`。
2. 协议变更未更新文档。
3. 引入新文档但未纳入 `docs/文档导航.md`。
4. 功能描述与代码事实不一致。
