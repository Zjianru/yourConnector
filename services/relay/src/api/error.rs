//! API 错误定义与响应转换。

use axum::{Json, http::StatusCode};
use serde_json::Value;

use super::response::ApiEnvelope;

/// 认证与接口错误。
#[derive(Debug)]
pub(crate) struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) suggestion: &'static str,
}

impl ApiError {
    /// 构造统一 API 错误。
    pub(crate) fn new(
        status: StatusCode,
        code: &'static str,
        message: impl Into<String>,
        suggestion: &'static str,
    ) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            suggestion,
        }
    }

    /// 转换为统一响应体。
    pub(crate) fn into_response(self) -> (StatusCode, Json<ApiEnvelope<Value>>) {
        (
            self.status,
            Json(ApiEnvelope {
                ok: false,
                code: self.code.to_string(),
                message: self.message,
                suggestion: self.suggestion.to_string(),
                data: None,
            }),
        )
    }
}
