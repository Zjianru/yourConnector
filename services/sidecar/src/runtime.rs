//! 运行时探测模块职责：
//! 1. 从系统进程列表提取统一的进程快照结构。
//! 2. 组织父子进程关系并交给 discoverers 做工具识别。
//! 3. 在未发现工具时按配置返回 fallback 占位工具。

use std::collections::HashMap;

use sysinfo::{ProcessesToUpdate, System};
use yc_shared_protocol::{LatestTokensPayload, ToolRuntimePayload, now_rfc3339_nanos};

use crate::{bytes_to_mb, discoverers};

/// 进程摘要信息，作为工具发现器的统一输入。
#[derive(Debug, Clone)]
pub(crate) struct ProcInfo {
    /// 进程 PID。
    pub(crate) pid: i32,
    /// 完整命令行字符串。
    pub(crate) cmd: String,
    /// 当前工作目录。
    pub(crate) cwd: String,
    /// CPU 使用率（百分比）。
    pub(crate) cpu_percent: f64,
    /// 内存占用（MB）。
    pub(crate) memory_mb: f64,
}

/// 扫描系统进程并识别 OpenCode/OpenClaw 工具实例。
pub(crate) fn discover_tools(sys: &mut System, fallback_tool: bool) -> Vec<ToolRuntimePayload> {
    sys.refresh_processes(ProcessesToUpdate::All, true);

    // all：pid -> 进程信息；children_by_ppid：父进程 -> 子进程 pid 列表。
    let mut all: HashMap<i32, ProcInfo> = HashMap::new();
    let mut children_by_ppid: HashMap<i32, Vec<i32>> = HashMap::new();

    for process in sys.processes().values() {
        let pid = process.pid().as_u32() as i32;
        let ppid = process
            .parent()
            .map(|parent| parent.as_u32() as i32)
            .unwrap_or(0);
        let cmd = process
            .cmd()
            .iter()
            .map(|item| item.to_string_lossy().to_string())
            .collect::<Vec<String>>()
            .join(" ");
        if cmd.is_empty() {
            continue;
        }
        let cwd = process
            .cwd()
            .map(|dir| dir.display().to_string())
            .unwrap_or_default();

        all.insert(
            pid,
            ProcInfo {
                pid,
                cmd,
                cwd,
                cpu_percent: process.cpu_usage() as f64,
                memory_mb: bytes_to_mb(process.memory()),
            },
        );
        children_by_ppid.entry(ppid).or_default().push(pid);
    }

    discoverers::discover_tools(&all, &children_by_ppid, fallback_tool)
}

/// 当开关开启且未发现真实工具时，返回单条 fallback 占位工具。
pub(crate) fn fallback_tools_or_empty(fallback_tool: bool) -> Vec<ToolRuntimePayload> {
    if !fallback_tool {
        return Vec::new();
    }
    vec![ToolRuntimePayload {
        tool_id: "tool_local".to_string(),
        name: "Local Tool".to_string(),
        category: "DEV_WORKER".to_string(),
        vendor: "yourconnector".to_string(),
        mode: "TUI".to_string(),
        status: "IDLE".to_string(),
        connected: false,
        endpoint: String::new(),
        pid: None,
        reason: Some("未发现 OpenCode/OpenClaw 进程，展示 fallback 工具".to_string()),
        cpu_percent: Some(0.0),
        memory_mb: Some(0.0),
        source: Some("fallback".to_string()),
        workspace_dir: None,
        session_id: None,
        session_title: None,
        session_updated_at: None,
        agent_mode: None,
        provider_id: None,
        model_id: None,
        model: None,
        latest_tokens: Some(LatestTokensPayload::default()),
        model_usage: Vec::new(),
        collected_at: Some(now_rfc3339_nanos()),
    }]
}
