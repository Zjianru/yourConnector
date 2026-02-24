//! relay CLI 分发：`run`、`status`、`doctor`、`service`、`version`。

use std::process::Command;

use anyhow::{anyhow, bail};
use serde_json::json;

/// CLI 分发结果。
pub(crate) enum CliDispatch {
    /// 继续进入 relay 主循环。
    Run,
    /// 命令已处理完成，主程序应退出。
    Exit,
}

/// 解析并执行 relay CLI。
pub(crate) fn dispatch(args: &[String]) -> anyhow::Result<CliDispatch> {
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
        "status" => {
            let active = service_active();
            println!("yc-relay: {}", if active { "active" } else { "inactive" });
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
            "unknown command: {other}; run `yc-relay --help` for usage"
        )),
    }
}

/// `doctor` 输出格式。
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
    Err(anyhow!("usage: yc-relay doctor [--format text|json]"))
}

/// 打印 doctor 信息并按健康度设置退出码。
fn run_doctor(format: DoctorFormat) {
    let manager = service_manager();
    let active = service_active();
    let relay_addr = std::env::var("RELAY_ADDR").unwrap_or_else(|_| "0.0.0.0:18080".to_string());
    let public_ws = std::env::var("RELAY_PUBLIC_WS_URL").unwrap_or_default();

    match format {
        DoctorFormat::Text => {
            println!("service-manager: {}", manager);
            println!("service-active: {}", if active { "yes" } else { "no" });
            println!("relay-addr: {}", relay_addr);
            println!("relay-public-ws: {}", public_ws);
        }
        DoctorFormat::Json => {
            let payload = json!({
                "serviceManager": manager,
                "serviceActive": active,
                "relayAddr": relay_addr,
                "relayPublicWsUrl": public_ws,
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
            println!("yc-relay: {}", if active { "active" } else { "inactive" });
            if !active {
                std::process::exit(1);
            }
            Ok(())
        }
        _ => Err(anyhow!(
            "usage: yc-relay service <start|stop|restart|status>"
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

/// 检查 relay 是否由系统守护进程托管并活跃。
fn service_active() -> bool {
    if cfg!(target_os = "linux") {
        return Command::new("systemctl")
            .args(["is-active", "--quiet", "yc-relay.service"])
            .status()
            .map(|st| st.success())
            .unwrap_or(false);
    }

    if cfg!(target_os = "macos") {
        return Command::new("launchctl")
            .args(["print", "system/dev.yourconnector.relay"])
            .status()
            .map(|st| st.success())
            .unwrap_or(false);
    }

    false
}

/// 启动 relay 服务。
fn service_start() -> anyhow::Result<()> {
    if cfg!(target_os = "linux") {
        run_command("systemctl", &["start", "yc-relay.service"])
    } else if cfg!(target_os = "macos") {
        run_command(
            "launchctl",
            &["kickstart", "-k", "system/dev.yourconnector.relay"],
        )
    } else {
        bail!("unsupported platform for service start")
    }
}

/// 停止 relay 服务。
fn service_stop() -> anyhow::Result<()> {
    if cfg!(target_os = "linux") {
        run_command("systemctl", &["stop", "yc-relay.service"])
    } else if cfg!(target_os = "macos") {
        run_command(
            "launchctl",
            &[
                "bootout",
                "system",
                "/Library/LaunchDaemons/dev.yourconnector.relay.plist",
            ],
        )
    } else {
        bail!("unsupported platform for service stop")
    }
}

/// 重启 relay 服务。
fn service_restart() -> anyhow::Result<()> {
    if cfg!(target_os = "linux") {
        run_command("systemctl", &["restart", "yc-relay.service"])
    } else if cfg!(target_os = "macos") {
        service_stop()?;
        service_start()
    } else {
        bail!("unsupported platform for service restart")
    }
}

/// 运行系统命令并把失败转成可读错误。
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

/// 打印 root help。
fn print_root_help() {
    println!("yc-relay usage:");
    println!("  yc-relay run");
    println!("  yc-relay status");
    println!("  yc-relay doctor [--format text|json]");
    println!("  yc-relay service <start|stop|restart|status>");
    println!("  yc-relay version");
}
