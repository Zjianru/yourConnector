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
    let normalized_workspace = normalize_path(workspace);
    let normalized_cmd = cmd.trim().to_ascii_lowercase();

    // openclaw-gateway 常由 launchd/systemd 托管，重启时 PID 会变化。
    // 这里使用稳定 ID（不带 PID），避免白名单因 PID 漂移导致“卡片消失/无法回接”。
    if normalized_cmd.contains("openclaw-gateway") {
        let stable_source = if !normalized_workspace.trim().is_empty() {
            normalized_workspace.as_str()
        } else if !normalized_cmd.is_empty() {
            normalized_cmd.as_str()
        } else {
            "openclaw-gateway"
        };
        let hex = format!("{:016x}", fnv1a64(stable_source.as_bytes()));
        return format!("openclaw_{}_gw", &hex[..12]);
    }

    let instance = normalize_tool_instance_suffix(fallback_pid);
    if !normalized_workspace.trim().is_empty() {
        let hex = format!("{:016x}", fnv1a64(normalized_workspace.as_bytes()));
        return format!("openclaw_{}_{instance}", &hex[..12]);
    }

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

#[cfg(test)]
mod tests {
    use super::build_openclaw_tool_id;

    #[test]
    fn openclaw_gateway_id_should_be_stable_across_pid_changes() {
        let cmd = "openclaw-gateway --port 18789";
        let a = build_openclaw_tool_id("", cmd, 1001);
        let b = build_openclaw_tool_id("", cmd, 2002);
        assert_eq!(a, b);
        assert!(a.ends_with("_gw"));
    }

    #[test]
    fn openclaw_cli_id_should_still_distinguish_instances() {
        let cmd = "openclaw --model gpt-5";
        let a = build_openclaw_tool_id("/workspace/a", cmd, 1001);
        let b = build_openclaw_tool_id("/workspace/a", cmd, 2002);
        assert_ne!(a, b);
        assert!(a.ends_with("_p1001"));
    }
}
