//! Relay 二进制入口：仅负责启动应用。

mod api;
mod app;
mod auth;
mod pairing;
mod state;
mod ws;

#[tokio::main]
/// 启动 Relay 服务。
async fn main() -> anyhow::Result<()> {
    app::run().await
}
