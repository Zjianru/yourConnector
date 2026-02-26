# yourConnector 工具详情与数据采集：opencode.v1 详情模型

## 1. 数据来源

`opencode.v1` 由 OpenCode 适配器生成，主文件：

1. `services/sidecar/src/tooling/adapters/opencode.rs`

采集来源包含两类：

1. 进程与会话状态：工作目录、会话 ID、模型、token 用量。
2. `opencode debug` 输出：skills 与 MCP 配置快照。

## 2. Schema 顶层字段

`opencode.v1` 数据体包含：

1. `workspaceDir`
2. `sessionId`
3. `sessionTitle`
4. `sessionUpdatedAt`
5. `agentMode`
6. `providerId`
7. `modelId`
8. `model`
9. `latestTokens`
10. `modelUsage`
11. `skills`
12. `mcp`
13. `collectedAt`
14. `expiresAt`

## 3. 发现与实例策略

1. 优先识别 wrapper 进程（`opencode`）并绑定 runtime 子进程。
2. 若 wrapper 已退出但 runtime 仍存活，补齐 standalone runtime 工具。
3. `toolId` 由工作目录 + 进程实例构成，避免跨实例冲突。

对应代码：`services/sidecar/src/tooling/adapters/opencode.rs`。

## 4. 移动端渲染口径

1. `schema === opencode.v1` 走默认详情卡片渲染分支。
2. 重点展示：会话信息、模型信息、token 用量、工作目录。
3. `stale=true` 时显示“数据过期（展示最近成功值）”，但保留上次可读数据。

对应代码：`app/mobile/ui/js/modals/tool-detail.js`。

## 5. 进程失效提示

1. 若检测到同族新 PID 出现在候选列表，旧卡片会被标记 `invalidPidChanged`。
2. 被标记的 OpenCode 卡片状态强制为 `INVALID`，提示“删除卡片后重新接入新进程”。

对应代码：

1. `app/mobile/ui/js/state/runtime.js` `syncOpencodeInvalidState()`
2. `app/mobile/ui/js/views/tools.js`
