//! relay 子命令：查看/测试/修改 sidecar 的 relay 地址。

use std::time::Duration;

use anyhow::{Context, anyhow};

use crate::config::{
    Config, DEFAULT_RELAY_WS_URL, load_sidecar_persisted_config, relay_health_url,
    save_sidecar_persisted_config, validate_user_relay_ws_url,
};

/// relay 子命令动作。
#[derive(Debug, Clone)]
pub(crate) enum RelayCommand {
    /// 展示当前 relay 配置与连通性。
    Show,
    /// 持久化设置 relay 地址。
    Set {
        /// 用户输入 URL。
        url: String,
        /// 是否允许 ws（仅调试态）。
        allow_insecure_ws: bool,
    },
    /// 测试 relay 连通性，不写入配置。
    Test {
        /// 可选测试 URL；为空时测试当前生效配置。
        url: Option<String>,
        /// 是否允许 ws（仅调试态）。
        allow_insecure_ws: bool,
    },
    /// 重置为默认 relay。
    Reset,
}

/// 执行 relay 子命令。
pub(crate) async fn execute(command: RelayCommand) -> anyhow::Result<()> {
    match command {
        RelayCommand::Show => show_current_relay().await,
        RelayCommand::Set {
            url,
            allow_insecure_ws,
        } => set_relay(url, allow_insecure_ws),
        RelayCommand::Test {
            url,
            allow_insecure_ws,
        } => test_relay(url, allow_insecure_ws).await,
        RelayCommand::Reset => reset_relay(),
    }
}

/// 展示持久化配置、生效配置和连通性测试结果。
async fn show_current_relay() -> anyhow::Result<()> {
    let persisted = load_sidecar_persisted_config().unwrap_or_default();
    let persisted_relay = persisted
        .relay_ws_url
        .clone()
        .unwrap_or_else(|| DEFAULT_RELAY_WS_URL.to_string());

    println!("relay (persisted): {persisted_relay}");

    let effective = Config::from_env();
    match effective {
        Ok(cfg) => {
            println!("relay (effective): {}", cfg.relay_ws_url);
            match relay_health_check(&cfg.relay_ws_url).await {
                Ok(_) => println!("health: ok"),
                Err(err) => println!("health: failed ({err})"),
            }
        }
        Err(err) => {
            println!("relay (effective): invalid ({err})");
        }
    }

    Ok(())
}

/// 写入 relay 持久化配置。
fn set_relay(raw_url: String, allow_insecure_ws: bool) -> anyhow::Result<()> {
    let normalized = validate_user_relay_ws_url(&raw_url, allow_insecure_ws)
        .with_context(|| format!("invalid relay url: {raw_url}"))?;

    let mut persisted = load_sidecar_persisted_config().unwrap_or_default();
    persisted.relay_ws_url = Some(normalized.clone());
    persisted.version = persisted.version.max(1);
    save_sidecar_persisted_config(&persisted)?;

    println!("relay updated: {normalized}");
    println!("next step: restart sidecar service to apply runtime change");
    Ok(())
}

/// 重置 relay 地址为默认值。
fn reset_relay() -> anyhow::Result<()> {
    let mut persisted = load_sidecar_persisted_config().unwrap_or_default();
    persisted.relay_ws_url = None;
    persisted.version = persisted.version.max(1);
    save_sidecar_persisted_config(&persisted)?;

    println!("relay reset to default: {DEFAULT_RELAY_WS_URL}");
    Ok(())
}

/// 测试 relay 地址可达性。
async fn test_relay(url: Option<String>, allow_insecure_ws: bool) -> anyhow::Result<()> {
    let candidate = if let Some(raw) = url {
        validate_user_relay_ws_url(&raw, allow_insecure_ws)
            .with_context(|| format!("invalid relay url: {raw}"))?
    } else {
        let persisted = load_sidecar_persisted_config().unwrap_or_default();
        let raw = persisted
            .relay_ws_url
            .clone()
            .unwrap_or_else(|| DEFAULT_RELAY_WS_URL.to_string());
        validate_user_relay_ws_url(&raw, allow_insecure_ws)
            .with_context(|| format!("invalid relay url: {raw}"))?
    };

    relay_health_check(&candidate).await?;
    println!("relay test success: {candidate}");
    Ok(())
}

/// 通过 relay `/healthz` 检查连通性。
async fn relay_health_check(relay_ws_url: &str) -> anyhow::Result<()> {
    let health = relay_health_url(relay_ws_url)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("build relay health client failed")?;

    let resp = client
        .get(health.clone())
        .send()
        .await
        .with_context(|| format!("request relay health failed: {health}"))?;

    if !resp.status().is_success() {
        return Err(anyhow!("relay health status is {}", resp.status()));
    }

    let body = resp.text().await.unwrap_or_default();
    if body.trim() != "ok" {
        return Err(anyhow!("relay health body is not ok"));
    }
    Ok(())
}
