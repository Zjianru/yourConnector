# Mobile App（Tauri iOS）

## 启动与调试

```bash
cd /Users/codez/develop/yourConnector

# 先启动 relay 与 sidecar
make run-relay
make run-sidecar

# 启动 iOS App
make run-mobile-tauri-ios IOS_SIM="iPhone 17 Pro"

# 开发模式（可选）
make run-mobile-tauri-ios-dev IOS_SIM="iPhone 17 Pro"
make run-mobile-tauri-ios-dev-clean IOS_SIM="iPhone 17 Pro"
make repair-ios-sim IOS_SIM="iPhone 17 Pro"
```

## 配对调试辅助

```bash
cd /Users/codez/develop/yourConnector
make show-pairing
make show-pairing-link
make simulate-ios-scan
```

## 前端模块结构

1. `/Users/codez/develop/yourConnector/app/mobile/ui/index.html`：页面骨架
2. `/Users/codez/develop/yourConnector/app/mobile/ui/styles/*`：样式层
3. `/Users/codez/develop/yourConnector/app/mobile/ui/js/main.js`：装配入口
4. `/Users/codez/develop/yourConnector/app/mobile/ui/js/state/*`：状态层
5. `/Users/codez/develop/yourConnector/app/mobile/ui/js/services/*`：服务层
6. `/Users/codez/develop/yourConnector/app/mobile/ui/js/flows/*`：流程层
7. `/Users/codez/develop/yourConnector/app/mobile/ui/js/views/*`：渲染层
8. `/Users/codez/develop/yourConnector/app/mobile/ui/js/modals/*`：弹窗层

## 本地检查

```bash
cd /Users/codez/develop/yourConnector
find /Users/codez/develop/yourConnector/app/mobile/ui/js -name '*.js' -print0 | xargs -0 -I{} sh -c 'node --check "$$1" && node --check --input-type=module < "$$1"' _ "{}"
```
