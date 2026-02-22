//! OpenCode 存储扫描与文件工具函数。

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::tooling::opencode_session::types::{DirSignature, OpenCodeSessionMeta};

/// 计算文件集签名（数量 + 最新 mtime）。
pub(super) fn files_signature(paths: &[PathBuf]) -> DirSignature {
    let mut signature = DirSignature::default();

    for path in paths {
        if !path.is_file() {
            continue;
        }
        signature.file_count += 1;
        signature.latest_mtime_ms = signature.latest_mtime_ms.max(path_mtime_ms(path));
    }

    signature
}

/// 计算 message 目录签名。
pub(super) fn message_dir_signature(root: &Path, session_id: &str) -> DirSignature {
    let message_dir = root.join("message").join(session_id);
    dir_json_signature(&message_dir)
}

/// 收集 session 元数据文件。
pub(super) fn collect_session_meta_files(root: &Path) -> Vec<PathBuf> {
    let session_root = root.join("session");
    let Ok(project_dirs) = fs::read_dir(session_root) else {
        return Vec::new();
    };
    let mut files = Vec::new();

    for project_dir in project_dirs.flatten() {
        let project_path = project_dir.path();
        if !project_path.is_dir() {
            continue;
        }
        let Ok(entries) = fs::read_dir(project_path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || !has_json_ext(&path) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with("ses_") {
                files.push(path);
            }
        }
    }

    files
}

/// 读取 JSON 文件并反序列化。
pub(super) fn read_json_file<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let data = fs::read(path).ok()?;
    serde_json::from_slice::<T>(&data).ok()
}

/// 获取 OpenCode storage 根目录。
pub(super) fn opencode_storage_root() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    if home.trim().is_empty() {
        return None;
    }
    Some(
        Path::new(&home)
            .join(".local")
            .join("share")
            .join("opencode")
            .join("storage"),
    )
}

/// 在候选 session 元数据中按目录优先、更新时间次之，选择目标 session。
pub(super) fn select_session_meta(
    session_files: &[PathBuf],
    normalized_cwd: &str,
) -> Option<OpenCodeSessionMeta> {
    let metas = session_files
        .iter()
        .filter_map(|path| read_json_file::<OpenCodeSessionMeta>(path))
        .filter(|meta| !meta.id.trim().is_empty())
        .collect::<Vec<OpenCodeSessionMeta>>();

    if metas.is_empty() {
        return None;
    }

    if !normalized_cwd.is_empty() {
        let matched = metas
            .iter()
            .filter(|meta| crate::tooling::normalize_path(&meta.directory) == normalized_cwd)
            .max_by_key(|meta| meta.time.updated)
            .cloned();
        if matched.is_some() {
            return matched;
        }
    }

    metas.into_iter().max_by_key(|meta| meta.time.updated)
}

/// 计算目录中 JSON 文件签名。
fn dir_json_signature(path: &Path) -> DirSignature {
    let Ok(entries) = fs::read_dir(path) else {
        return DirSignature::default();
    };

    let mut signature = DirSignature::default();
    for entry in entries.flatten() {
        let file_path = entry.path();
        if !file_path.is_file() || !has_json_ext(&file_path) {
            continue;
        }
        signature.file_count += 1;
        signature.latest_mtime_ms = signature.latest_mtime_ms.max(path_mtime_ms(&file_path));
    }
    signature
}

/// 获取文件修改时间（毫秒）。
fn path_mtime_ms(path: &Path) -> u128 {
    fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|ts| ts.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
        .map(|dur| dur.as_millis())
        .unwrap_or(0)
}

/// 判断文件扩展名是否为 JSON。
fn has_json_ext(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("json"))
        .unwrap_or(false)
}
