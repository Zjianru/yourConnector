//! 工具适配器注册模块职责：
//! 1. 汇总 OpenCode/OpenClaw 适配器并对外暴露统一入口。
//! 2. 定义工具详情 schema 常量，确保跨端字段约定稳定。

pub(crate) mod claude_code;
pub(crate) mod codex;
pub(crate) mod openclaw;
pub(crate) mod opencode;

/// OpenClaw 详情结构版本标识。
pub(crate) const OPENCLAW_SCHEMA_V1: &str = "openclaw.v1";
/// OpenCode 详情结构版本标识。
pub(crate) const OPENCODE_SCHEMA_V1: &str = "opencode.v1";
/// Codex 详情结构版本标识。
pub(crate) const CODEX_SCHEMA_V1: &str = "codex.v1";
/// Claude Code 详情结构版本标识。
pub(crate) const CLAUDE_CODE_SCHEMA_V1: &str = "claude-code.v1";
