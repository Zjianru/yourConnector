# Mobile App (Tauri)

```bash
# 1) 启动 relay + sidecar（各开一个终端）
cd /Users/codez/develop/yourConnector
make run-relay
make run-sidecar
make show-pairing-code
make show-pairing-link
make show-pairing

# 可选：开发态模拟扫码（把 yc://pair 链接投递给 iOS 模拟器）
make simulate-ios-scan

# 2) 启动 iOS App（推荐：打包安装模式，不依赖本地 dev server）
cd /Users/codez/develop/yourConnector
make run-mobile-tauri-ios IOS_SIM="iPhone 17 Pro"

# 3) 需要热更新时，使用 dev 模式（依赖本地网络权限）
make run-mobile-tauri-ios-dev IOS_SIM="iPhone 17 Pro"

# dev 模式白屏或卡启动时，先清理再重启
make run-mobile-tauri-ios-dev-clean IOS_SIM="iPhone 17 Pro"

# 如果模拟器服务状态异常（白屏、卡启动），先修复模拟器再启动
make repair-ios-sim IOS_SIM="iPhone 17 Pro"
```

如果 `dev` 模式提示本地网络权限或出现白屏：

- 到 iOS `设置 > 隐私与安全性 > 本地网络`，允许 `yourConnector Mobile`
- 关闭 App 后重新启动

连接前先在仓库根目录执行 `make show-pairing`（或 `make show-pairing-code`）获取配对信息，再在 App 内输入：

- `Relay WS URL`
- `配对码（systemId.pairToken）`
- `宿主机名称`（扫码可自动填充，可手改）

扫码链路建议：

1. 先启动 App 到“配对宿主机”页面
2. 执行 `make simulate-ios-scan`
3. App 自动导入 `yc://pair?...` 并尝试连接

配对命令支持参数化：

```bash
cd /Users/codez/develop/yourConnector
make pairing PAIR_ARGS="--show all --name 我的Mac"
make pairing PAIR_ARGS="--show qr --qr-png /tmp/yc-pair.png"
make pairing PAIR_ARGS="--show link --ttl-sec 180"
make pairing PAIR_ARGS="--show link --no-code"
```

终端二维码输出依赖 `qrencode`（可选：`brew install qrencode`）。

当前配对链接默认包含 `code + sid + ticket`；
如需只保留短时票据，可使用 `--no-code` 生成仅 `sid + ticket` 链接。

如果换机/重装后出现“设备未授权控制”：

- 先连接成功
- 在调试页点击“绑定当前设备为控制端”

UI 文件：`/Users/codez/develop/yourConnector/app/mobile/ui/index.html`
