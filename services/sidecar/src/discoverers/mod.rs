//! 工具发现器编排模块职责：
//! 1. 统一管理各工具发现器（OpenCode/OpenClaw）的调用顺序。
//! 2. 聚合发现结果并保持稳定顺序，必要时注入 fallback 工具。
//! 3. 对上提供单一 `discover_tools` 入口。

use std::collections::HashMap;

use yc_shared_protocol::ToolRuntimePayload;

use crate::{ProcInfo, fallback_tools_or_empty};

mod openclaw;
mod opencode;

/// 单个发现器函数签名。
type DiscoverFn = fn(&ToolDiscoveryContext<'_>) -> Vec<ToolRuntimePayload>;

/// 发现器运行上下文，避免每个发现器重复构建索引。
pub(crate) struct ToolDiscoveryContext<'a> {
    /// 全量进程映射（pid -> 进程信息）。
    pub(crate) all: &'a HashMap<i32, ProcInfo>,
    /// 父子进程索引（ppid -> children pid 列表）。
    pub(crate) children_by_ppid: &'a HashMap<i32, Vec<i32>>,
}

/// 发现器组件：负责调度多个工具发现器并合并结果。
pub(crate) struct ToolDiscoveryComponent {
    /// 已注册的发现器。
    discoverers: Vec<DiscoverFn>,
    /// 无工具时是否返回 fallback 占位。
    fallback_tool: bool,
}

impl ToolDiscoveryComponent {
    /// 构建发现器组件并注册默认发现器列表。
    pub(crate) fn new(fallback_tool: bool) -> Self {
        Self {
            discoverers: vec![
                opencode::discover_opencode_tools,
                openclaw::discover_openclaw_tools,
            ],
            fallback_tool,
        }
    }

    /// 执行所有发现器并汇总结果。
    pub(crate) fn discover(&self, context: &ToolDiscoveryContext<'_>) -> Vec<ToolRuntimePayload> {
        let mut tools = Vec::new();
        for discoverer in &self.discoverers {
            tools.extend(discoverer(context));
        }

        // 未发现真实工具时按配置返回 fallback。
        if tools.is_empty() {
            return fallback_tools_or_empty(self.fallback_tool);
        }

        // 统一排序，避免上层渲染抖动；实例级 toolId 不做去重。
        tools.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.workspace_dir.cmp(&b.workspace_dir))
                .then_with(|| a.pid.unwrap_or_default().cmp(&b.pid.unwrap_or_default()))
                .then_with(|| a.tool_id.cmp(&b.tool_id))
        });
        tools
    }
}

/// 对外发现入口：构造上下文后执行标准发现流程。
pub(crate) fn discover_tools(
    all: &HashMap<i32, ProcInfo>,
    children_by_ppid: &HashMap<i32, Vec<i32>>,
    fallback_tool: bool,
) -> Vec<ToolRuntimePayload> {
    let context = ToolDiscoveryContext {
        all,
        children_by_ppid,
    };
    ToolDiscoveryComponent::new(fallback_tool).discover(&context)
}
