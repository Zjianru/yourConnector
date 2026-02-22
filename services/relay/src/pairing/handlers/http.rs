//! 配对 HTTP 路由处理函数。

use axum::{Json, extract::State, http::StatusCode};

use crate::{
    api::{
        response::{ApiEnvelope, ok_response},
        types::{
            PairBootstrapData, PairBootstrapRequest, PairExchangeData, PairExchangeRequest,
            PairPreflightData, PairPreflightRequest,
        },
    },
    state::AppState,
};

/// 配对预检接口：用于移动端精确映射失败弹窗。
pub(crate) async fn pair_preflight_handler(
    State(state): State<AppState>,
    Json(req): Json<PairPreflightRequest>,
) -> (StatusCode, Json<ApiEnvelope<PairPreflightData>>) {
    match state.preflight_pair_credentials(&req).await {
        Ok(mode) => ok_response(
            StatusCode::OK,
            "配对信息可用",
            "可以继续执行配对并连接",
            Some(PairPreflightData { auth_mode: mode }),
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

/// 配对换发接口：绑定设备公钥并签发 access/refresh。
pub(crate) async fn pair_exchange_handler(
    State(state): State<AppState>,
    Json(req): Json<PairExchangeRequest>,
) -> (StatusCode, Json<ApiEnvelope<PairExchangeData>>) {
    let result = state.exchange_device_credential(&req).await;
    match result {
        Ok(data) => ok_response(
            StatusCode::OK,
            "设备凭证换发成功",
            "后续连接将使用设备凭证",
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

/// 配对签发接口：统一 sidecar 与脚本的配对链接来源。
pub(crate) async fn pair_bootstrap_handler(
    State(state): State<AppState>,
    Json(req): Json<PairBootstrapRequest>,
) -> (StatusCode, Json<ApiEnvelope<PairBootstrapData>>) {
    match state.issue_pair_bootstrap(&req).await {
        Ok(data) => ok_response(
            StatusCode::OK,
            "配对信息已签发",
            "请使用配对链接或二维码完成接入",
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
