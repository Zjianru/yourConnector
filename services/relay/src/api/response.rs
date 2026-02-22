//! API 响应包裹。

use axum::{Json, http::StatusCode};
use serde::Serialize;

/// 通用 API 成功/失败包裹结构。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApiEnvelope<T>
where
    T: Serialize,
{
    pub(crate) ok: bool,
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) suggestion: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) data: Option<T>,
}

/// 构造成功响应。
pub(crate) fn ok_response<T: Serialize>(
    status: StatusCode,
    message: impl Into<String>,
    suggestion: impl Into<String>,
    data: Option<T>,
) -> (StatusCode, Json<ApiEnvelope<T>>) {
    (
        status,
        Json(ApiEnvelope {
            ok: true,
            code: "OK".to_string(),
            message: message.into(),
            suggestion: suggestion.into(),
            data,
        }),
    )
}
