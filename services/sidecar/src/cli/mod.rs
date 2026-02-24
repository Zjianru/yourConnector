//! sidecar CLI 分发：`run`、`relay`、`pairing show`、`status`、`doctor`、`service`、`version`。

use std::process::Command;

use anyhow::{Context, anyhow, bail};
use serde_json::json;

mod pairing;
mod relay;

use pairing::{PairingOutputFormat, PairingShowCommand};
use relay::RelayCommand;

/// CLI 处理结果。
pub(crate) enum CliDispatch {
    /// 继续进入 sidecar 主循环。
    Run,
    /// 命令已处理完成，主程序应直接退出。
    Exit,
}

/// 解析并执行 sidecar CLI。
pub(crate) async fn dispatch(args: &[String]) -> anyhow::Result<CliDispatch> {
    if args.is_empty() {
        return Ok(CliDispatch::Run);
    }

    let cmd = args[0].trim();
    if cmd.is_empty() || cmd == "run" {
        return Ok(CliDispatch::Run);
    }

    if matches!(cmd, "-h" | "--help" | "help") {
        print_root_help();
        return Ok(CliDispatch::Exit);
    }

    match cmd {
        "relay" => {
            if args
                .get(1)
                .map(|value| matches!(value.as_str(), "-h" | "--help" | "help"))
                .unwrap_or(false)
            {
                print_relay_help();
                return Ok(CliDispatch::Exit);
            }
            let relay_cmd = parse_relay_command(&args[1..])?;
            relay::execute(relay_cmd).await?;
            Ok(CliDispatch::Exit)
        }
        "pairing" => {
            if args[1..]
                .iter()
                .any(|value| matches!(value.as_str(), "-h" | "--help" | "help"))
            {
                print_pairing_help();
                return Ok(CliDispatch::Exit);
            }
            let pairing_cmd = parse_pairing_command(&args[1..])?;
            pairing::execute_show(pairing_cmd).await?;
            Ok(CliDispatch::Exit)
        }
        "status" => {
            let active = service_active();
            println!("yc-sidecar: {}", if active { "active" } else { "inactive" });
            if !active {
                std::process::exit(1);
            }
            Ok(CliDispatch::Exit)
        }
        "doctor" => {
            let format = parse_doctor_format(&args[1..])?;
            run_doctor(format);
            Ok(CliDispatch::Exit)
        }
        "service" => {
            let action = args.get(1).map(String::as_str).unwrap_or("");
            run_service_action(action)?;
            Ok(CliDispatch::Exit)
        }
        "version" => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            Ok(CliDispatch::Exit)
        }
        other => Err(anyhow!(
            "unknown command: {other}; run `yc-sidecar --help` for usage"
        )),
    }
}

/// 解析 `relay` 子命令。
fn parse_relay_command(args: &[String]) -> anyhow::Result<RelayCommand> {
    if args.is_empty() {
        return Ok(RelayCommand::Show);
    }

    if args[0] == "-h" || args[0] == "--help" || args[0] == "help" {
        print_relay_help();
        return Ok(RelayCommand::Show);
    }

    match args[0].as_str() {
        "set" => {
            let (allow_insecure_ws, rest) = strip_allow_insecure_flag(&args[1..]);
            if rest.len() != 1 {
                return Err(anyhow!(
                    "usage: yc-sidecar relay set <wss-url> [--allow-insecure-ws]"
                ));
            }
            Ok(RelayCommand::Set {
                url: rest[0].clone(),
                allow_insecure_ws,
            })
        }
        "-change" => {
            let (allow_insecure_ws, rest) = strip_allow_insecure_flag(&args[1..]);
            if rest.len() != 1 {
                return Err(anyhow!(
                    "usage: yc-sidecar relay -change <wss-url> [--allow-insecure-ws]"
                ));
            }
            Ok(RelayCommand::Set {
                url: rest[0].clone(),
                allow_insecure_ws,
            })
        }
        "test" => {
            let (allow_insecure_ws, rest) = strip_allow_insecure_flag(&args[1..]);
            if rest.len() > 1 {
                return Err(anyhow!(
                    "usage: yc-sidecar relay test [wss-url] [--allow-insecure-ws]"
                ));
            }
            Ok(RelayCommand::Test {
                url: rest.first().cloned(),
                allow_insecure_ws,
            })
        }
        "reset" => {
            if args.len() != 1 {
                return Err(anyhow!("usage: yc-sidecar relay reset"));
            }
            Ok(RelayCommand::Reset)
        }
        other => Err(anyhow!(
            "unsupported relay command: {other}; run `yc-sidecar relay --help`"
        )),
    }
}

/// 解析 `pairing show` 子命令。
fn parse_pairing_command(args: &[String]) -> anyhow::Result<PairingShowCommand> {
    if args.is_empty() || args[0].as_str() != "show" {
        return Err(anyhow!(
            "usage: yc-sidecar pairing show [--format text|json|link|qr] [--relay <wss-url>] [--allow-insecure-ws]"
        ));
    }

    let mut format = PairingOutputFormat::Text;
    let mut relay_override: Option<String> = None;
    let mut allow_insecure_ws = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--format" => {
                let Some(raw) = args.get(i + 1) else {
                    return Err(anyhow!("--format requires value"));
                };
                format = PairingOutputFormat::parse(raw)?;
                i += 2;
            }
            "--relay" => {
                let Some(raw) = args.get(i + 1) else {
                    return Err(anyhow!("--relay requires value"));
                };
                relay_override = Some(raw.clone());
                i += 2;
            }
            "--allow-insecure-ws" => {
                allow_insecure_ws = true;
                i += 1;
            }
            other => {
                return Err(anyhow!(
                    "unsupported pairing option: {other}; run `yc-sidecar pairing show --help`"
                ));
            }
        }
    }

    Ok(PairingShowCommand {
        format,
        relay_override,
        allow_insecure_ws,
    })
}

/// 提取 `--allow-insecure-ws`，返回剩余位置参数。
fn strip_allow_insecure_flag(args: &[String]) -> (bool, Vec<String>) {
    let mut allow_insecure_ws = false;
    let mut rest = Vec::new();
    for arg in args {
        if arg == "--allow-insecure-ws" {
            allow_insecure_ws = true;
            continue;
        }
        rest.push(arg.clone());
    }
    (allow_insecure_ws, rest)
}

/// 打印 root help。
fn print_root_help() {
    println!("yc-sidecar usage:");
    println!("  yc-sidecar run");
    println!("  yc-sidecar relay [set|-change|test|reset]");
    println!("  yc-sidecar pairing show [--format text|json|link|qr]");
    println!("  yc-sidecar status");
    println!("  yc-sidecar doctor [--format text|json]");
    println!("  yc-sidecar service <start|stop|restart|status>");
    println!("  yc-sidecar version");
}

/// 打印 relay help。
fn print_relay_help() {
    println!("yc-sidecar relay usage:");
    println!("  yc-sidecar relay");
    println!("  yc-sidecar relay set <wss-url> [--allow-insecure-ws]");
    println!("  yc-sidecar relay -change <wss-url> [--allow-insecure-ws]");
    println!("  yc-sidecar relay test [wss-url] [--allow-insecure-ws]");
    println!("  yc-sidecar relay reset");
}

/// 打印 pairing help。
fn print_pairing_help() {
    println!(
        "yc-sidecar pairing show usage:\n  yc-sidecar pairing show [--format text|json|link|qr] [--relay <wss-url>] [--allow-insecure-ws]"
    );
}

/// doctor 输出格式。
enum DoctorFormat {
    Text,
    Json,
}

/// 解析 doctor 的 `--format` 参数。
fn parse_doctor_format(args: &[String]) -> anyhow::Result<DoctorFormat> {
    if args.is_empty() {
        return Ok(DoctorFormat::Text);
    }
    if args.len() == 2 && args[0] == "--format" {
        return match args[1].as_str() {
            "text" => Ok(DoctorFormat::Text),
            "json" => Ok(DoctorFormat::Json),
            other => Err(anyhow!("unsupported doctor format: {other}")),
        };
    }
    Err(anyhow!("usage: yc-sidecar doctor [--format text|json]"))
}

/// 输出 sidecar 诊断信息，并按健康度设置退出码。
fn run_doctor(format: DoctorFormat) {
    let manager = service_manager();
    let active = service_active();
    let health_addr = std::env::var("SIDECAR_ADDR").unwrap_or_else(|_| "0.0.0.0:18081".to_string());
    let relay_ws = std::env::var("RELAY_WS_URL").unwrap_or_default();

    match format {
        DoctorFormat::Text => {
            println!("service-manager: {manager}");
            println!("service-active: {}", if active { "yes" } else { "no" });
            println!("sidecar-addr: {health_addr}");
            println!("relay-ws-url: {relay_ws}");
        }
        DoctorFormat::Json => {
            let payload = json!({
                "serviceManager": manager,
                "serviceActive": active,
                "sidecarAddr": health_addr,
                "relayWsUrl": relay_ws,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
            );
        }
    }

    if !active {
        std::process::exit(1);
    }
}

/// 执行 service start|stop|restart|status。
fn run_service_action(action: &str) -> anyhow::Result<()> {
    match action {
        "start" => service_start(),
        "stop" => service_stop(),
        "restart" => service_restart(),
        "status" => {
            let active = service_active();
            println!("yc-sidecar: {}", if active { "active" } else { "inactive" });
            if !active {
                std::process::exit(1);
            }
            Ok(())
        }
        _ => Err(anyhow!(
            "usage: yc-sidecar service <start|stop|restart|status>"
        )),
    }
}

/// 服务管理器标识。
fn service_manager() -> &'static str {
    if cfg!(target_os = "linux") {
        "systemd"
    } else if cfg!(target_os = "macos") {
        "launchd"
    } else {
        "unknown"
    }
}

/// 检查 sidecar 是否由守护进程托管并活跃。
fn service_active() -> bool {
    if cfg!(target_os = "linux") {
        return Command::new("systemctl")
            .args(["is-active", "--quiet", "yc-sidecar.service"])
            .status()
            .map(|st| st.success())
            .unwrap_or(false);
    }

    if cfg!(target_os = "macos") {
        return Command::new("launchctl")
            .args(["print", "system/dev.yourconnector.sidecar"])
            .status()
            .map(|st| st.success())
            .unwrap_or(false);
    }

    false
}

/// 启动 sidecar 服务。
fn service_start() -> anyhow::Result<()> {
    if cfg!(target_os = "linux") {
        run_command("systemctl", &["start", "yc-sidecar.service"])
    } else if cfg!(target_os = "macos") {
        run_command(
            "launchctl",
            &["kickstart", "-k", "system/dev.yourconnector.sidecar"],
        )
    } else {
        bail!("unsupported platform for service start")
    }
}

/// 停止 sidecar 服务。
fn service_stop() -> anyhow::Result<()> {
    if cfg!(target_os = "linux") {
        run_command("systemctl", &["stop", "yc-sidecar.service"])
    } else if cfg!(target_os = "macos") {
        run_command(
            "launchctl",
            &[
                "bootout",
                "system",
                "/Library/LaunchDaemons/dev.yourconnector.sidecar.plist",
            ],
        )
    } else {
        bail!("unsupported platform for service stop")
    }
}

/// 重启 sidecar 服务。
fn service_restart() -> anyhow::Result<()> {
    if cfg!(target_os = "linux") {
        run_command("systemctl", &["restart", "yc-sidecar.service"])
    } else if cfg!(target_os = "macos") {
        service_stop()?;
        service_start()
    } else {
        bail!("unsupported platform for service restart")
    }
}

/// 运行系统命令并将失败转为可读错误。
fn run_command(cmd: &str, args: &[&str]) -> anyhow::Result<()> {
    let status = Command::new(cmd)
        .args(args)
        .status()
        .map_err(|err| anyhow!("run {cmd} failed: {err}"))?;
    if !status.success() {
        bail!("{cmd} exited with non-zero status");
    }
    Ok(())
}

/// 解析字符串参数为 `String`，并附加上下文。
#[allow(dead_code)]
fn parse_arg(args: &[String], index: usize, name: &str) -> anyhow::Result<String> {
    args.get(index)
        .cloned()
        .with_context(|| format!("missing argument: {name}"))
}
