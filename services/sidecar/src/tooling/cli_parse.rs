//! 命令行与路径解析工具。

use std::{collections::HashMap, path::PathBuf};

use crate::ProcInfo;

/// 当工具会话目录缺失时，用进程 cwd 兜底。
pub(crate) fn first_non_empty(primary: &str, fallback: &str) -> String {
    if !primary.trim().is_empty() {
        return primary.to_string();
    }
    fallback.to_string()
}

/// 将非空字符串包装为 Option，减少构造 payload 时的样板代码。
pub(crate) fn option_non_empty(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

/// 路径归一化：移除 ./ 与 ../ 影响，提升 ID 与匹配稳定性。
pub(crate) fn normalize_path(path: &str) -> String {
    if path.trim().is_empty() {
        return String::new();
    }
    let mut normalized = PathBuf::new();
    for component in std::path::Path::new(path).components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                let _ = normalized.pop();
            }
            _ => normalized.push(component),
        }
    }
    normalized.to_string_lossy().to_string()
}

/// 解析 serve 命令的 host/port 参数，兼容 --key value 和 --key=value 两种写法。
pub(crate) fn parse_serve_address(cmd: &str) -> (String, i32) {
    let mut host = "127.0.0.1".to_string();
    let mut port = 0_i32;
    let tokens = cmd.split_whitespace().collect::<Vec<&str>>();
    let mut idx = 0usize;
    while idx < tokens.len() {
        let token = tokens[idx];
        if token == "--hostname" && idx + 1 < tokens.len() {
            host = tokens[idx + 1].trim().to_string();
            idx += 2;
            continue;
        }
        if let Some(value) = token.strip_prefix("--hostname=") {
            host = value.trim().to_string();
            idx += 1;
            continue;
        }
        if token == "--port" && idx + 1 < tokens.len() {
            if let Ok(value) = tokens[idx + 1].trim().parse::<i32>() {
                port = value;
            }
            idx += 2;
            continue;
        }
        if let Some(value) = token.strip_prefix("--port=")
            && let Ok(parsed) = value.trim().parse::<i32>()
        {
            port = parsed;
        }
        idx += 1;
    }
    (host, port)
}

/// 通用 CLI 参数读取函数。
pub(crate) fn parse_cli_flag_value(cmd: &str, flag: &str) -> Option<String> {
    let tokens = cmd.split_whitespace().collect::<Vec<&str>>();
    let mut idx = 0usize;
    while idx < tokens.len() {
        let token = tokens[idx];
        if token == flag {
            if idx + 1 < tokens.len() {
                let value = tokens[idx + 1].trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
            idx += 2;
            continue;
        }
        let prefix = format!("{flag}=");
        if let Some(value) = token.strip_prefix(&prefix) {
            let cleaned = value.trim();
            if !cleaned.is_empty() {
                return Some(cleaned.to_string());
            }
        }
        idx += 1;
    }
    None
}

/// 判断命令行中是否包含独立命令词，避免子串误判。
fn contains_command_word(cmd_lower: &str, word: &str) -> bool {
    cmd_lower == word
        || cmd_lower.starts_with(&format!("{word} "))
        || cmd_lower.contains(&format!(" {word} "))
        || cmd_lower.ends_with(&format!("/{word}"))
        || cmd_lower.contains(&format!("/{word} "))
}

/// 判断 token 是否是目标命令本体（支持 /path/codex 形态）。
fn token_matches_command(token: &str, word: &str) -> bool {
    let trimmed = token.trim_matches(|ch| ch == '"' || ch == '\'');
    if trimmed == word {
        return true;
    }
    let unix_name = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let windows_name = unix_name.rsplit('\\').next().unwrap_or(unix_name);
    windows_name == word
}

/// 判断是否是可接入的 opencode 运行命令。
pub(crate) fn is_opencode_candidate_command(cmd_lower: &str) -> bool {
    if !contains_command_word(cmd_lower, "opencode") {
        return false;
    }
    if cmd_lower.contains("--help")
        || cmd_lower.contains("--version")
        || cmd_lower.contains(" opencode debug")
        || cmd_lower.contains(" opencode completion")
    {
        return false;
    }
    true
}

/// 判断是否是 opencode 的 wrapper 进程命令。
pub(crate) fn is_opencode_wrapper_command(cmd_lower: &str) -> bool {
    is_opencode_candidate_command(cmd_lower)
        && !cmd_lower.contains("opencode-darwin-arm64/bin/opencode")
}

/// 判断是否是可接入的 openclaw 命令。
pub(crate) fn is_openclaw_candidate_command(cmd_lower: &str) -> bool {
    let is_openclaw_cli = contains_command_word(cmd_lower, "openclaw");
    let is_openclaw_gateway = contains_command_word(cmd_lower, "openclaw-gateway");
    if !is_openclaw_cli && !is_openclaw_gateway {
        return false;
    }
    if cmd_lower.contains("--help")
        || cmd_lower.contains("--version")
        || cmd_lower.contains(" openclaw debug")
        || cmd_lower.contains(" openclaw completion")
    {
        return false;
    }
    true
}

/// 判断是否是可接入的 codex 命令。
pub(crate) fn is_codex_candidate_command(cmd_lower: &str) -> bool {
    if !contains_command_word(cmd_lower, "codex") {
        return false;
    }
    // 排除 Codex 桌面端 / VSCode 插件内嵌进程：
    // 这些通常以 `codex app-server` 运行，并非用户可接管的 CLI 会话。
    let tokens = cmd_lower.split_whitespace().collect::<Vec<&str>>();
    for (idx, token) in tokens.iter().enumerate() {
        if !token_matches_command(token, "codex") {
            continue;
        }
        let next = tokens
            .get(idx + 1)
            .map(|value| value.trim_matches(|ch| ch == '"' || ch == '\''))
            .unwrap_or_default();
        if next == "app-server" {
            return false;
        }
    }
    if cmd_lower.contains("/applications/codex.app/")
        || cmd_lower.contains("/.vscode/extensions/openai.chatgpt-")
    {
        return false;
    }
    if cmd_lower.contains("--help")
        || cmd_lower.contains("--version")
        || cmd_lower.contains(" codex completion")
        || cmd_lower.contains(" codex doctor")
    {
        return false;
    }
    true
}

/// 判断是否是可接入的 claude code 命令（claude）。
pub(crate) fn is_claude_code_candidate_command(cmd_lower: &str) -> bool {
    if !contains_command_word(cmd_lower, "claude") {
        return false;
    }
    // 排除桌面端内嵌/应用进程，只接入 Claude Code CLI。
    let tokens = cmd_lower.split_whitespace().collect::<Vec<&str>>();
    for (idx, token) in tokens.iter().enumerate() {
        if !token_matches_command(token, "claude") {
            continue;
        }
        let next = tokens
            .get(idx + 1)
            .map(|value| value.trim_matches(|ch| ch == '"' || ch == '\''))
            .unwrap_or_default();
        if next == "app-server" {
            return false;
        }
    }
    if cmd_lower.contains("/applications/claude.app/") || cmd_lower.contains("claude helper") {
        return false;
    }
    if cmd_lower.contains("--help")
        || cmd_lower.contains("--version")
        || cmd_lower.contains(" claude completion")
    {
        return false;
    }
    true
}

/// 统一探测主机地址：0.0.0.0/:: 对外展示为本机可访问地址。
pub(crate) fn normalize_probe_host(host: &str) -> String {
    match host.trim() {
        "" | "0.0.0.0" | "::" => "127.0.0.1".to_string(),
        raw => raw.to_string(),
    }
}

/// 在 wrapper + children 中优先挑选真实 runtime 进程 pid。
pub(crate) fn pick_runtime_pid(
    wrapper_pid: i32,
    candidate_pids: &[i32],
    all: &HashMap<i32, ProcInfo>,
) -> i32 {
    for pid in candidate_pids {
        let Some(info) = all.get(pid) else {
            continue;
        };
        if info
            .cmd
            .to_lowercase()
            .contains("opencode-darwin-arm64/bin/opencode")
        {
            return *pid;
        }
    }
    wrapper_pid
}

/// 基于运行模式和会话状态判断连接状态与提示。
pub(crate) fn evaluate_opencode_connection(
    mode: &str,
    state: &super::opencode_session::OpenCodeSessionState,
) -> (bool, &'static str, String) {
    if mode == "SERVE" {
        return (
            false,
            "UNSUPPORTED_MODE",
            "当前策略只支持通过 opencode 命令运行的会话，不支持 opencode serve。".to_string(),
        );
    }
    if state.session_id.is_empty() {
        return (
            true,
            "RUNNING",
            "已接入 opencode 进程，等待会话消息后补充模式和模型信息。".to_string(),
        );
    }
    (true, "RUNNING", String::new())
}

/// 基于运行模式和模型参数判断 OpenClaw 可接入状态与提示。
pub(crate) fn evaluate_openclaw_connection(
    mode: &str,
    model: &str,
) -> (bool, &'static str, String) {
    if mode == "SERVE" {
        return (
            false,
            "UNSUPPORTED_MODE",
            "当前策略只支持通过 openclaw 命令运行的会话，不支持 openclaw serve。".to_string(),
        );
    }
    if model.trim().is_empty() {
        return (
            true,
            "RUNNING",
            "已发现 openclaw 进程，等待模型参数同步。".to_string(),
        );
    }
    (
        true,
        "RUNNING",
        format!("已发现 openclaw 进程，模型：{model}"),
    )
}

/// 根据命令行特征判断 OpenCode 当前模式。
pub(crate) fn detect_opencode_mode(cmd: &str) -> &'static str {
    if cmd.contains("opencode serve") || cmd.contains("opencode web") {
        return "SERVE";
    }
    "TUI"
}

/// 根据命令行特征判断 OpenClaw 当前模式。
pub(crate) fn detect_openclaw_mode(cmd: &str) -> &'static str {
    if cmd.contains("openclaw serve") || cmd.contains("openclaw web") {
        return "SERVE";
    }
    "CLI"
}

#[cfg(test)]
mod tests {
    use super::{
        evaluate_openclaw_connection, is_claude_code_candidate_command, is_codex_candidate_command,
        is_openclaw_candidate_command,
    };

    #[test]
    fn evaluate_openclaw_connection_rejects_serve_mode() {
        let (connected, status, reason) = evaluate_openclaw_connection("SERVE", "gpt-5");
        assert!(!connected);
        assert_eq!(status, "UNSUPPORTED_MODE");
        assert!(reason.contains("openclaw serve"));
    }

    #[test]
    fn evaluate_openclaw_connection_accepts_cli_mode() {
        let (connected, status, reason) = evaluate_openclaw_connection("CLI", "gpt-5");
        assert!(connected);
        assert_eq!(status, "RUNNING");
        assert!(reason.contains("模型"));
    }

    #[test]
    fn openclaw_candidate_accepts_gateway_process() {
        assert!(is_openclaw_candidate_command("openclaw-gateway"));
        assert!(is_openclaw_candidate_command(
            "/usr/local/bin/openclaw-gateway --port 18000"
        ));
    }

    #[test]
    fn openclaw_candidate_rejects_help_command() {
        assert!(!is_openclaw_candidate_command("openclaw --help"));
    }

    #[test]
    fn codex_candidate_accepts_runtime_command() {
        assert!(is_codex_candidate_command("codex run \"hello\""));
    }

    #[test]
    fn codex_candidate_rejects_app_server_subcommand() {
        assert!(!is_codex_candidate_command("codex app-server --analytics-default-enabled"));
        assert!(!is_codex_candidate_command(
            "/applications/codex.app/contents/resources/codex app-server --analytics-default-enabled"
        ));
    }

    #[test]
    fn codex_candidate_rejects_vscode_embedded_codex() {
        assert!(!is_codex_candidate_command(
            "/users/codez/.vscode/extensions/openai.chatgpt-0.4.78-darwin-arm64/bin/macos-aarch64/codex app-server"
        ));
    }

    #[test]
    fn claude_candidate_accepts_runtime_command() {
        assert!(is_claude_code_candidate_command("claude -p \"hello\""));
    }

    #[test]
    fn claude_candidate_rejects_app_server_or_desktop_process() {
        assert!(!is_claude_code_candidate_command("claude app-server"));
        assert!(!is_claude_code_candidate_command(
            "/applications/claude.app/contents/macos/claude"
        ));
    }
}
