//! 日志系统模块职责：
//! 1. 初始化 stdout + 文件双通道 tracing 日志。
//! 2. 将运行日志按天落在 `logs/raw` 目录。
//! 3. 将历史日期日志自动归档到 `logs/archive/<YYYY-MM-DD>.7z`。

use std::{
    collections::BTreeMap,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use chrono::{Local, NaiveDate};
use sevenz_rust::compress_to_path;
use tokio::{task::JoinHandle, time::MissedTickBehavior};
use tracing::warn;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    EnvFilter, Layer, filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt,
};

/// 默认日志根目录（相对当前工作目录）。
const DEFAULT_LOG_DIR: &str = "logs";
/// 日志原始文件目录名。
const RAW_DIR_NAME: &str = "raw";
/// 日志归档目录名。
const ARCHIVE_DIR_NAME: &str = "archive";
/// 归档临时目录名。
const ARCHIVE_TMP_DIR_NAME: &str = ".archive-tmp";
/// 归档互斥锁目录名。
const ARCHIVE_LOCK_DIR_NAME: &str = ".archive-lock";
/// 归档任务默认轮询周期（秒）。
const DEFAULT_ARCHIVE_INTERVAL_SEC: u64 = 3600;
/// 文件日志级别环境变量（独立于 `RUST_LOG`）。
const FILE_LOG_LEVEL_ENV: &str = "YC_FILE_LOG_LEVEL";
/// stdout 默认日志过滤（人类可读摘要）。
const DEFAULT_STDOUT_FILTER: &str = "info";

/// 日志运行时守卫，防止 non-blocking writer 提前析构。
pub(crate) struct LogRuntime {
    _stdout_guard: WorkerGuard,
    _file_guard: WorkerGuard,
    _archiver: JoinHandle<()>,
}

/// 初始化 sidecar 日志系统，并启动自动归档任务。
pub(crate) fn init(service_name: &str) -> Result<LogRuntime> {
    let root_dir = resolve_log_root();
    let raw_dir = root_dir.join(RAW_DIR_NAME);
    let archive_dir = root_dir.join(ARCHIVE_DIR_NAME);
    fs::create_dir_all(&raw_dir)
        .with_context(|| format!("create raw log dir: {}", raw_dir.display()))?;
    fs::create_dir_all(&archive_dir)
        .with_context(|| format!("create archive log dir: {}", archive_dir.display()))?;

    archive_completed_days(&root_dir, &raw_dir, &archive_dir)?;

    let file_appender = tracing_appender::rolling::daily(&raw_dir, format!("{service_name}.log"));
    let (file_writer, file_guard) = tracing_appender::non_blocking(file_appender);
    let (stdout_writer, stdout_guard) = tracing_appender::non_blocking(std::io::stdout());
    let stdout_filter = resolve_stdout_env_filter();
    let file_filter = resolve_file_level_filter();

    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_writer(stdout_writer)
        .with_ansi(true)
        .with_target(false)
        .compact()
        .with_filter(stdout_filter);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(file_writer)
        .with_ansi(false)
        .with_target(true)
        .with_filter(file_filter);

    tracing_subscriber::registry()
        .with(stdout_layer)
        .with(file_layer)
        .init();

    let archiver = spawn_archive_task(root_dir);
    Ok(LogRuntime {
        _stdout_guard: stdout_guard,
        _file_guard: file_guard,
        _archiver: archiver,
    })
}

/// 解析 stdout 日志过滤规则：优先 `RUST_LOG`，回退默认摘要级别。
fn resolve_stdout_env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_STDOUT_FILTER))
}

/// 解析文件日志级别；默认保留 `debug` 级别，确保日志文件可完整回放。
fn resolve_file_level_filter() -> LevelFilter {
    std::env::var(FILE_LOG_LEVEL_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::DEBUG)
}

/// 启动后台归档任务，定期将历史日志打包为 `.7z`。
fn spawn_archive_task(root_dir: PathBuf) -> JoinHandle<()> {
    let interval = archive_interval();
    tokio::spawn(async move {
        let raw_dir = root_dir.join(RAW_DIR_NAME);
        let archive_dir = root_dir.join(ARCHIVE_DIR_NAME);
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            if let Err(err) = archive_completed_days(&root_dir, &raw_dir, &archive_dir) {
                warn!("archive logs failed: {err}");
            }
        }
    })
}

/// 将环境变量中的日志路径解析成绝对路径。
fn resolve_log_root() -> PathBuf {
    let raw = std::env::var("YC_LOG_DIR").unwrap_or_else(|_| DEFAULT_LOG_DIR.to_string());
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        return path;
    }
    match std::env::current_dir() {
        Ok(dir) => dir.join(path),
        Err(_) => PathBuf::from(DEFAULT_LOG_DIR),
    }
}

/// 读取归档轮询间隔配置。
fn archive_interval() -> Duration {
    let raw = std::env::var("YC_LOG_ARCHIVE_INTERVAL_SEC").unwrap_or_default();
    let sec = raw
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_ARCHIVE_INTERVAL_SEC);
    Duration::from_secs(sec)
}

/// 按天归档已完成日期的日志文件，并在成功后删除 raw 原文件。
fn archive_completed_days(root_dir: &Path, raw_dir: &Path, archive_dir: &Path) -> Result<()> {
    if !raw_dir.exists() {
        return Ok(());
    }

    let Some(_lock) = acquire_archive_lock(root_dir)? else {
        return Ok(());
    };

    let today = Local::now().date_naive().format("%Y-%m-%d").to_string();
    let mut grouped: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();

    for entry in
        fs::read_dir(raw_dir).with_context(|| format!("read raw logs: {}", raw_dir.display()))?
    {
        let entry = entry.with_context(|| format!("read entry under {}", raw_dir.display()))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let Some(day) = extract_day_from_log_name(file_name) else {
            continue;
        };
        if day >= today {
            continue;
        }
        grouped.entry(day).or_default().push(path);
    }

    for (day, mut files) in grouped {
        files.sort();
        let archive_path = archive_dir.join(format!("{day}.7z"));
        if archive_path.exists() {
            for file in files {
                let _ = fs::remove_file(file);
            }
            continue;
        }

        let stage_dir = root_dir.join(ARCHIVE_TMP_DIR_NAME).join(&day);
        if stage_dir.exists() {
            let _ = fs::remove_dir_all(&stage_dir);
        }
        fs::create_dir_all(&stage_dir)
            .with_context(|| format!("create archive stage dir: {}", stage_dir.display()))?;

        for file in &files {
            let Some(name) = file.file_name() else {
                continue;
            };
            let target = stage_dir.join(name);
            fs::copy(file, &target).with_context(|| {
                format!(
                    "copy log to stage: {} -> {}",
                    file.display(),
                    target.display()
                )
            })?;
        }

        let archive_tmp = archive_dir.join(format!("{day}.7z.tmp"));
        if archive_tmp.exists() {
            let _ = fs::remove_file(&archive_tmp);
        }
        compress_to_path(&stage_dir, &archive_tmp)
            .with_context(|| format!("compress logs to {}", archive_tmp.display()))?;
        fs::rename(&archive_tmp, &archive_path).with_context(|| {
            format!(
                "finalize archive {} -> {}",
                archive_tmp.display(),
                archive_path.display()
            )
        })?;

        for file in files {
            let _ = fs::remove_file(file);
        }
        let _ = fs::remove_dir_all(&stage_dir);
    }

    Ok(())
}

/// 从日志文件名中提取日期（格式：`YYYY-MM-DD`）。
fn extract_day_from_log_name(file_name: &str) -> Option<String> {
    let day = file_name.rsplit('.').next()?;
    if NaiveDate::parse_from_str(day, "%Y-%m-%d").is_err() {
        return None;
    }
    Some(day.to_string())
}

/// 尝试获取归档互斥锁，避免多个进程同时归档。
fn acquire_archive_lock(root_dir: &Path) -> Result<Option<ArchiveLockGuard>> {
    let lock_dir = root_dir.join(ARCHIVE_LOCK_DIR_NAME);
    match fs::create_dir(&lock_dir) {
        Ok(_) => Ok(Some(ArchiveLockGuard { lock_dir })),
        Err(err) if err.kind() == ErrorKind::AlreadyExists => Ok(None),
        Err(err) => {
            Err(err).with_context(|| format!("create archive lock: {}", lock_dir.display()))
        }
    }
}

/// 归档锁守卫，析构时自动释放锁目录。
struct ArchiveLockGuard {
    lock_dir: PathBuf,
}

impl Drop for ArchiveLockGuard {
    /// 释放归档锁目录。
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.lock_dir);
    }
}
