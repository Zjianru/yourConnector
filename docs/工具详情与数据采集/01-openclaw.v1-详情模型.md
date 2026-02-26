# yourConnector 工具详情与数据采集：openclaw.v1 详情模型

## 1. 数据来源

`openclaw.v1` 由 Sidecar OpenClaw 适配器采集，主文件：

1. `services/sidecar/src/tooling/adapters/openclaw.rs`

核心命令来源：

1. `openclaw status --json --usage`（失败回退 `status --json`）
2. `openclaw agents list --json --bindings`
3. `openclaw channels status --json`
4. `openclaw models status --json`
5. `openclaw sessions --json`（兜底）
6. `openclaw health --json`
7. `openclaw gateway status --json`
8. `openclaw memory status --json`（深采）
9. `openclaw security audit --json`（深采）

说明：`memory/security` 只在“指定工具且 force 刷新”时采集。

## 2. Schema 顶层字段

当前 `openclaw.v1` 数据体包含：

1. `overview`
2. `agents`
3. `sessions`
4. `usage`
5. `systemService`
6. `statusDots`
7. `workspaceDir`
8. 向后兼容字段：`channelOverview` `healthSummary`

## 3. 移动端渲染结构

工具详情弹窗将 `openclaw.v1` 渲染为五屏：

1. 概览
2. Agents
3. Sessions
4. Usage
5. 系统与服务

实现：`app/mobile/ui/js/modals/tool-detail.js`。

### 3.1 Sessions 分段

1. `diagnostics`
2. `timeline`
3. `ledger`

### 3.2 Usage 分段

1. 支持切换 `1h/24h/7d/all` 标签。
2. 当前版本真实采集窗口只有 `1h`，其余窗口展示提示文本。

## 4. 状态点口径

1. `statusDots.gateway`：网关在线态（`online/offline/unknown`）。
2. `statusDots.data`：数据新鲜度（`fresh/stale/collecting/unknown`）。
3. 若详情条目标记 `stale=true`，UI 强制按 `data=stale` 展示。

## 5. 采集失败与降级

1. 单次采集失败不会清空上次成功数据。
2. 缓存条目会被标记为 `stale=true` 并注入 `collectError`。
3. 前端显示“数据过期（展示最近成功值）”。

对应缓存逻辑：`services/sidecar/src/tooling/core/cache.rs`。
