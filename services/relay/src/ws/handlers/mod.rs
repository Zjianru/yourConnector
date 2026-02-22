//! WebSocket 握手与会话处理模块。

mod auth;
mod http;

pub(crate) use http::ws_handler;
