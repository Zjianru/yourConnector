//! Relay 二进制入口：仅负责启动应用。

mod api;
mod app;
mod auth;
mod cli;
mod logging;
mod pairing;
mod state;
mod ws;

#[tokio::main]
/// 启动 Relay 服务。
async fn main() -> anyhow::Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<String>>();
    match cli::dispatch(&args)? {
        cli::CliDispatch::Run => {}
        cli::CliDispatch::Exit => return Ok(()),
    }

    let _log_runtime = logging::init("relay")?;
    app::run().await
}
