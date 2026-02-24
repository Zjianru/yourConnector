//! Relay 配对签发客户端。

use anyhow::{Context, anyhow};
use reqwest::Url;
use serde::{Deserialize, Serialize};

/// 配对签发请求。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PairBootstrapRequest {
    system_id: String,
    pair_token: String,
    host_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    relay_ws_url: Option<String>,
    include_code: bool,
}

/// API 包裹。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiEnvelope<T> {
    ok: bool,
    code: String,
    message: String,
    suggestion: String,
    data: Option<T>,
}

/// 配对签发响应数据。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PairBootstrapData {
    pub(crate) pair_link: String,
    pub(crate) pair_ticket: String,
    pub(crate) relay_ws_url: String,
    pub(crate) system_id: String,
    pub(crate) host_name: String,
    pub(crate) pair_code: Option<String>,
    pub(crate) simctl_command: String,
}

/// 将 relay WS URL 映射为 HTTP API base（`/v1/`）。
fn relay_api_base(relay_ws_url: &str) -> anyhow::Result<Url> {
    let mut parsed = Url::parse(relay_ws_url)
        .with_context(|| format!("invalid relay ws url: {relay_ws_url}"))?;
    match parsed.scheme() {
        "ws" => {
            let _ = parsed.set_scheme("http");
        }
        "wss" => {
            let _ = parsed.set_scheme("https");
        }
        "http" | "https" => {}
        other => {
            return Err(anyhow!("unsupported relay scheme: {other}"));
        }
    }
    parsed.set_query(None);
    parsed.set_fragment(None);
    // 结尾保留 `/`，确保 `Url::join(\"pair/bootstrap\")` 得到 `/v1/pair/bootstrap`。
    parsed.set_path("/v1/");
    Ok(parsed)
}

/// 请求 relay 签发配对信息。
pub(crate) async fn fetch_pair_bootstrap(
    relay_ws_url: &str,
    relay_ws_url_for_link: Option<&str>,
    system_id: &str,
    pair_token: &str,
    host_name: &str,
) -> anyhow::Result<PairBootstrapData> {
    let base = relay_api_base(relay_ws_url)?;
    let endpoint = base
        .join("pair/bootstrap")
        .context("build bootstrap endpoint failed")?;

    let client = reqwest::Client::new();
    let req = PairBootstrapRequest {
        system_id: system_id.to_string(),
        pair_token: pair_token.to_string(),
        host_name: host_name.to_string(),
        relay_ws_url: relay_ws_url_for_link.map(ToString::to_string),
        include_code: true,
    };

    let resp = client
        .post(endpoint)
        .json(&req)
        .send()
        .await
        .context("request relay pair bootstrap failed")?;

    let status = resp.status();
    let body: ApiEnvelope<PairBootstrapData> = resp
        .json()
        .await
        .context("decode relay pair bootstrap response failed")?;

    if !status.is_success() || !body.ok {
        return Err(anyhow!(
            "pair bootstrap failed: {} {} ({})",
            body.code,
            body.message,
            body.suggestion
        ));
    }

    let Some(data) = body.data else {
        return Err(anyhow!("pair bootstrap response missing data"));
    };
    Ok(data)
}
