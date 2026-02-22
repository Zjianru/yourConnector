//! 鉴权 HTTP 路由处理函数。

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};

use crate::{
    api::{
        response::{ApiEnvelope, ok_response},
        types::{
            AuthDevicesData, AuthDevicesQuery, AuthRefreshData, AuthRefreshRequest,
            AuthRevokeDeviceData, AuthRevokeDeviceRequest,
        },
    },
    state::AppState,
};

/// 刷新接口：轮换 refresh 并颁发新 access。
pub(crate) async fn auth_refresh_handler(
    State(state): State<AppState>,
    Json(req): Json<AuthRefreshRequest>,
) -> (StatusCode, Json<ApiEnvelope<AuthRefreshData>>) {
    match state.refresh_device_credential(&req).await {
        Ok(data) => ok_response(
            StatusCode::OK,
            "凭证刷新成功",
            "继续使用当前会话",
            Some(data),
        ),
        Err(err) => {
            let (status, body) = err.into_response();
            (
                status,
                Json(ApiEnvelope {
                    ok: body.0.ok,
                    code: body.0.code,
                    message: body.0.message,
                    suggestion: body.0.suggestion,
                    data: None,
                }),
            )
        }
    }
}

/// 吊销设备接口。
pub(crate) async fn auth_revoke_device_handler(
    State(state): State<AppState>,
    Json(req): Json<AuthRevokeDeviceRequest>,
) -> (StatusCode, Json<ApiEnvelope<AuthRevokeDeviceData>>) {
    match state.revoke_device(&req).await {
        Ok(data) => ok_response(
            StatusCode::OK,
            "设备已吊销",
            "被吊销设备需重新配对",
            Some(data),
        ),
        Err(err) => {
            let (status, body) = err.into_response();
            (
                status,
                Json(ApiEnvelope {
                    ok: body.0.ok,
                    code: body.0.code,
                    message: body.0.message,
                    suggestion: body.0.suggestion,
                    data: None,
                }),
            )
        }
    }
}

/// 设备列表接口。
pub(crate) async fn auth_devices_handler(
    State(state): State<AppState>,
    Query(query): Query<AuthDevicesQuery>,
) -> (StatusCode, Json<ApiEnvelope<AuthDevicesData>>) {
    match state.list_devices(&query).await {
        Ok(data) => ok_response(
            StatusCode::OK,
            "设备列表获取成功",
            "可以对设备进行管理",
            Some(data),
        ),
        Err(err) => {
            let (status, body) = err.into_response();
            (
                status,
                Json(ApiEnvelope {
                    ok: body.0.ok,
                    code: body.0.code,
                    message: body.0.message,
                    suggestion: body.0.suggestion,
                    data: None,
                }),
            )
        }
    }
}
