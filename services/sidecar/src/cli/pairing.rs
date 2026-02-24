//! pairing 子命令：输出配对链接、JSON、二维码等信息。

use std::process::Command;

use anyhow::{Context, anyhow};

use crate::{
    config::{Config, validate_user_relay_ws_url},
    pairing::{banner::print_pairing_banner, bootstrap_client::fetch_pair_bootstrap},
};

/// 配对输出格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PairingOutputFormat {
    /// 高亮文本。
    Text,
    /// JSON。
    Json,
    /// 仅链接。
    Link,
    /// 终端二维码。
    Qr,
}

impl PairingOutputFormat {
    /// 从字符串解析输出格式。
    pub(crate) fn parse(raw: &str) -> anyhow::Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "link" => Ok(Self::Link),
            "qr" => Ok(Self::Qr),
            other => Err(anyhow!(
                "unsupported pairing format: {other}, expected text|json|link|qr"
            )),
        }
    }
}

/// `pairing show` 的参数。
#[derive(Debug, Clone)]
pub(crate) struct PairingShowCommand {
    /// 输出格式。
    pub(crate) format: PairingOutputFormat,
    /// 可选 relay 覆盖地址。
    pub(crate) relay_override: Option<String>,
    /// 是否允许 `ws://`（仅调试态）。
    pub(crate) allow_insecure_ws: bool,
}

/// 执行 `pairing show`。
pub(crate) async fn execute_show(command: PairingShowCommand) -> anyhow::Result<()> {
    let cfg = Config::from_env()?;
    let relay_ws_url = if let Some(raw) = command.relay_override.as_ref() {
        validate_user_relay_ws_url(raw, command.allow_insecure_ws)
            .with_context(|| format!("invalid relay override: {raw}"))?
    } else {
        cfg.relay_ws_url.clone()
    };
    let relay_ws_url_for_link = command
        .relay_override
        .as_ref()
        .map(|_| relay_ws_url.as_str());

    let data = fetch_pair_bootstrap(
        &relay_ws_url,
        relay_ws_url_for_link,
        &cfg.system_id,
        &cfg.pair_token,
        &cfg.host_name,
    )
    .await
    .context("fetch pairing bootstrap failed")?;

    match command.format {
        PairingOutputFormat::Text => {
            print_pairing_banner(&data);
        }
        PairingOutputFormat::Json => {
            let payload = serde_json::json!({
                "relayWsUrl": data.relay_ws_url,
                "systemId": data.system_id,
                "hostName": data.host_name,
                "pairTicket": data.pair_ticket,
                "pairCode": data.pair_code,
                "pairLink": data.pair_link,
                "simctlCommand": data.simctl_command,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&payload).context("encode pairing json failed")?
            );
        }
        PairingOutputFormat::Link => println!("{}", data.pair_link),
        PairingOutputFormat::Qr => print_pairing_qr(&data.pair_link)?,
    }
    Ok(())
}

/// 打印终端二维码，依赖本机安装 `qrencode`。
fn print_pairing_qr(pair_link: &str) -> anyhow::Result<()> {
    let status = Command::new("qrencode")
        .args(["-t", "ANSIUTF8", pair_link])
        .status()
        .context("run qrencode failed; please install qrencode")?;

    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("qrencode exited with non-zero status"))
    }
}
