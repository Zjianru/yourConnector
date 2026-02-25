# yourConnector

## 项目结构

1. `app/mobile`：Tauri Mobile App（iOS + Android）
2. `services/relay`：Relay 服务（Rust）
3. `services/sidecar`：Sidecar 服务（Rust）
4. `protocol/rust`：共享协议类型（Rust）
5. `docs`：设计、治理、验收文档

## 核心命令

```bash
cd yourConnector

# 服务启动
make run-relay
make run-sidecar

# iOS 启动
make run-mobile-tauri-ios IOS_SIM="iPhone 17 Pro"

# Android 初始化与打包
make init-mobile-tauri-android
make build-mobile-tauri-android-apk-test \
  ANDROID_KEYSTORE_PATH="/abs/path/release.jks" \
  ANDROID_KEY_ALIAS="your_alias" \
  ANDROID_KEYSTORE_PASSWORD="***" \
  ANDROID_KEY_PASSWORD="***"
make build-mobile-tauri-android-aab ANDROID_TARGETS="aarch64"

# 配对辅助
make show-pairing
make show-pairing-link
make simulate-ios-scan
```

## 安装流程（A/B 宿主机）

```bash
# A机：先安装 relay
sudo bash /path/to/yc-relay.sh install \
  --acme-email you@example.com \
  --public-ip <公网IPv4>

# A机：再安装 sidecar（接入本机 relay）
sudo bash /path/to/yc-sidecar.sh install \
  --relay-ip <公网IPv4>

# B机：只安装 sidecar（接入 A 机 relay）
sudo bash /path/to/yc-sidecar.sh install \
  --relay-ip <公网IPv4>
```

说明：

1. 公网默认仅支持 `wss://.../v1/ws`，`ws://` 仅用于开发调试开关。
2. 安装脚本面向用户输入统一为公网 IPv4；脚本内部自动拼接标准 Relay 地址。
3. `yc-relay.sh` 在 Linux 上使用 Let’s Encrypt shortlived IP 证书（HTTP-01 + webroot）。
4. 详细参数、卸载与 `doctor/status` 见 `docs/分发安装与卸载-v1.md`。
5. GitHub Actions 已支持“打 tag 自动构建服务端发布资产”，工作流见 `.github/workflows/release-linux.yml`。
6. GitHub Actions 已支持“打 tag 自动构建并签名 Android APK，再上传到对应 GitHub Release”，工作流见 `.github/workflows/release-android.yml`。
7. GitHub Actions 已支持“按 tag 同步服务端发布资产到阿里云 OSS（国内下载）”，工作流见 `.github/workflows/sync-release-to-oss.yml`。
8. 国内下载地址模板：`https://<ALIYUN_OSS_DOMAIN>/<ALIYUN_OSS_PREFIX>/<tag>/<file>`

## 质量门禁

```bash
cd yourConnector

# 治理门禁（注释/行长/文档一致性）
make check-governance

# 全量门禁（编译、格式、静态检查、测试、JS 语法、治理）
make check-all
```

## 系统日志

1. Relay 与 Sidecar 启动后会同时写入 stdout 与文件日志。
2. 默认目录：
   - 原始日志：`logs/raw`
   - 每日归档：`logs/archive`
3. 归档规则：
   - 文件按天命名：`<service>.log.YYYY-MM-DD`
   - 已完成日期会自动打包为 `YYYY-MM-DD.7z`
4. 常用环境变量：
   - `YC_LOG_DIR`：日志根目录（默认 `logs`）
   - `YC_LOG_ARCHIVE_INTERVAL_SEC`：归档轮询周期（默认 `3600` 秒）
   - `YC_FILE_LOG_LEVEL`：文件日志级别（默认 `debug`）
   - `RUST_LOG`：仅影响 stdout 级别，不影响文件日志
5. 配对信息（配对码、配对链接、模拟扫码命令）会以高亮 banner 直接输出到终端，便于现场配对。
6. 详细说明见：`docs/系统日志与归档-v1.md`

## Android 签名

1. 本地测试包与 release 包统一使用 `scripts/mobile/sign-android-apk.sh` 签名。
2. `make build-mobile-tauri-android-apk-test` 与 `make build-mobile-tauri-android-apk-signed` 都会执行签名。
3. 本地签名需要的环境变量/参数：
   - `ANDROID_KEYSTORE_PATH`
   - `ANDROID_KEY_ALIAS`
   - `ANDROID_KEYSTORE_PASSWORD`
   - `ANDROID_KEY_PASSWORD`（可选，默认等于 `ANDROID_KEYSTORE_PASSWORD`）
4. GitHub `release-android` 工作流需要配置仓库 Secrets：
   - `ANDROID_KEYSTORE_BASE64`（`base64` 后的 keystore 内容）
   - `ANDROID_KEY_ALIAS`
   - `ANDROID_KEYSTORE_PASSWORD`
   - `ANDROID_KEY_PASSWORD`（可选）

## 文档入口

1. `docs/文档导航-v2.md`
2. `docs/代码治理与注释规范-v1.md`
3. `docs/质量门禁与检查规范-v1.md`
4. `docs/分发安装与卸载-v1.md`
5. `docs/系统日志与归档-v1.md`
6. `docs/已完成功能验收-v1.md`
7. `docs/工具接入核心组件-v1.md`
8. `docs/跨宿主联调测试-v1.md`
