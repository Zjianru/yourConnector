//! 鉴权 HTTP 接口处理模块。

mod devices;
mod http;
mod refresh;
mod revoke;
mod verify;

pub(crate) use http::{auth_devices_handler, auth_refresh_handler, auth_revoke_device_handler};
