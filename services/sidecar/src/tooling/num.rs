//! 数值换算与展示精度。

/// 将字节数换算为 MB。
pub(crate) fn bytes_to_mb(v: u64) -> f64 {
    v as f64 / 1024.0 / 1024.0
}

/// 将字节数换算为 GB。
pub(crate) fn bytes_to_gb(v: u64) -> f64 {
    v as f64 / 1024.0 / 1024.0 / 1024.0
}

/// 四舍五入保留两位小数，用于前端展示。
pub(crate) fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}
