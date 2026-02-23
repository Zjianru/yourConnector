//! Tool Adapter Core 类型定义职责：
//! 1. 定义发现阶段与详情采集阶段的上下文结构。
//! 2. 定义跨适配器共享的详情采集结果与运行选项。

use std::{collections::HashMap, time::Duration};

use serde_json::Value;
use yc_shared_protocol::ToolRuntimePayload;

use crate::ProcInfo;

/// 发现阶段上下文：包含进程索引和父子关系索引。
pub(crate) struct ToolDiscoveryContext<'a> {
    /// 全量进程映射（pid -> 进程信息）。
    pub(crate) all: &'a HashMap<i32, ProcInfo>,
    /// 父子进程关系（ppid -> children pid 列表）。
    pub(crate) children_by_ppid: &'a HashMap<i32, Vec<i32>>,
}

/// 单次详情采集的运行选项。
#[derive(Debug, Clone)]
pub(crate) struct ToolDetailCollectOptions {
    /// 详情数据 TTL，用于计算 `expiresAt`。
    pub(crate) detail_ttl: Duration,
    /// 外部命令执行超时。
    pub(crate) command_timeout: Duration,
    /// 详情采集并发度上限。
    pub(crate) max_parallel: usize,
}

/// 适配器返回的单工具详情结果（成功或失败）。
#[derive(Debug, Clone)]
pub(crate) struct ToolDetailCollectResult {
    /// 目标工具 ID。
    pub(crate) tool_id: String,
    /// 详情 schema（openclaw.v1 / opencode.v1）。
    pub(crate) schema: String,
    /// 可选 profile 分组键。
    pub(crate) profile_key: Option<String>,
    /// 采集成功时的详情数据。
    pub(crate) data: Option<Value>,
    /// 采集失败时的人类可读错误。
    pub(crate) error: Option<String>,
}

impl ToolDetailCollectResult {
    /// 构造成功结果。
    pub(crate) fn success(
        tool_id: impl Into<String>,
        schema: impl Into<String>,
        profile_key: Option<String>,
        data: Value,
    ) -> Self {
        Self {
            tool_id: tool_id.into(),
            schema: schema.into(),
            profile_key,
            data: Some(data),
            error: None,
        }
    }

    /// 构造失败结果（用于标记 stale 并保留旧值）。
    pub(crate) fn failed(
        tool_id: impl Into<String>,
        schema: impl Into<String>,
        profile_key: Option<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            tool_id: tool_id.into(),
            schema: schema.into(),
            profile_key,
            data: None,
            error: Some(error.into()),
        }
    }
}

/// 详情采集请求参数。
#[derive(Debug, Clone)]
pub(crate) struct ToolDetailsCollectRequest {
    /// 参与采集的工具集合（通常是已接入工具）。
    pub(crate) tools: Vec<ToolRuntimePayload>,
    /// 指定工具 ID；为空表示批量刷新。
    pub(crate) target_tool_id: Option<String>,
    /// 是否强制刷新（忽略去抖）。
    pub(crate) force: bool,
}
