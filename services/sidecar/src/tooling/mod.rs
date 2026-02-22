//! 工具识别与 OpenCode 会话解析聚合模块。

pub(crate) mod cli_parse;
pub(crate) mod num;
pub(crate) mod opencode_session;
pub(crate) mod tool_id;

pub(crate) use cli_parse::{
    detect_openclaw_mode, detect_opencode_mode, evaluate_opencode_connection, first_non_empty,
    is_openclaw_candidate_command, is_opencode_candidate_command, is_opencode_wrapper_command,
    normalize_path, normalize_probe_host, option_non_empty, parse_cli_flag_value,
    parse_serve_address, pick_runtime_pid,
};
pub(crate) use num::{bytes_to_gb, bytes_to_mb, round2};
pub(crate) use opencode_session::collect_opencode_session_state;
pub(crate) use tool_id::{build_openclaw_tool_id, build_opencode_tool_id};
