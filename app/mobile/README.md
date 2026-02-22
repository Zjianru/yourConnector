# Mobile App（Tauri iOS）

## 启动步骤

```bash
# 1) 启动 relay + sidecar
cd /Users/codez/develop/yourConnector
make run-relay
make run-sidecar

# 2) 读取配对信息（relay 统一签发）
make show-pairing
# 或直接模拟扫码投递
make simulate-ios-scan

# 3) 启动 iOS App（推荐：打包安装模式）
make run-mobile-tauri-ios IOS_SIM="iPhone 17 Pro"
```

## dev 模式（可选）

```bash
cd /Users/codez/develop/yourConnector
make run-mobile-tauri-ios-dev IOS_SIM="iPhone 17 Pro"
make run-mobile-tauri-ios-dev-clean IOS_SIM="iPhone 17 Pro"
make repair-ios-sim IOS_SIM="iPhone 17 Pro"
```

`dev` 模式出现本地网络权限提示时，需要在 iOS `设置 > 隐私与安全性 > 本地网络` 允许 `yourConnector Mobile`，然后重启 App。

## 配对输入

手动配对页输入：

1. `Relay WS URL`
2. `System ID（sid）`
3. `配对票据（ticket）`
4. `宿主机名称`（可选修改）

## UI 文件结构（当前）

- `/Users/codez/develop/yourConnector/app/mobile/ui/index.html`：页面骨架
- `/Users/codez/develop/yourConnector/app/mobile/ui/styles/base.css`：当前完整样式
- `/Users/codez/develop/yourConnector/app/mobile/ui/styles/layout.css`
- `/Users/codez/develop/yourConnector/app/mobile/ui/styles/components.css`
- `/Users/codez/develop/yourConnector/app/mobile/ui/styles/modals.css`
- `/Users/codez/develop/yourConnector/app/mobile/ui/styles/tools.css`
- `/Users/codez/develop/yourConnector/app/mobile/ui/js/main.js`：主流程脚本
- `/Users/codez/develop/yourConnector/app/mobile/ui/js/state/*`：状态模块
- `/Users/codez/develop/yourConnector/app/mobile/ui/js/services/*`：服务模块
- `/Users/codez/develop/yourConnector/app/mobile/ui/js/utils/*`：工具模块
