//! 工具实例标识生成。

use super::cli_parse::normalize_path;

/// 依据“工作区 + 实例”生成 opencode 工具 ID，支持同工作区多进程并存。
pub(crate) fn build_opencode_tool_id(workspace: &str, fallback_pid: i32) -> String {
    let instance = normalize_tool_instance_suffix(fallback_pid);
    let normalized = normalize_path(workspace);
    if normalized.trim().is_empty() {
        return format!("opencode_{instance}");
    }
    let hex = format!("{:016x}", fnv1a64(normalized.as_bytes()));
    format!("opencode_{}_{instance}", &hex[..12])
}

/// 依据“工作区/命令 + 实例”生成 openclaw 工具 ID，支持同工作区多进程并存。
pub(crate) fn build_openclaw_tool_id(workspace: &str, cmd: &str, fallback_pid: i32) -> String {
    let instance = normalize_tool_instance_suffix(fallback_pid);
    let normalized_workspace = normalize_path(workspace);
    if !normalized_workspace.trim().is_empty() {
        let hex = format!("{:016x}", fnv1a64(normalized_workspace.as_bytes()));
        return format!("openclaw_{}_{instance}", &hex[..12]);
    }

    let normalized_cmd = cmd.trim().to_ascii_lowercase();
    if !normalized_cmd.is_empty() {
        let hex = format!("{:016x}", fnv1a64(normalized_cmd.as_bytes()));
        return format!("openclaw_{}_{instance}", &hex[..12]);
    }

    format!("openclaw_{instance}")
}

/// FNV-1a 64 位哈希，用于稳定生成 toolId。
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;

    let mut hash = OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// 将进程实例转换为 toolId 可读后缀；未知 pid 时退化为稳定哈希。
fn normalize_tool_instance_suffix(pid: i32) -> String {
    if pid > 0 {
        return format!("p{pid}");
    }
    let raw = pid.to_string();
    let hex = format!("{:016x}", fnv1a64(raw.as_bytes()));
    format!("x{}", &hex[..8])
}
