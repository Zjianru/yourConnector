//! OpenCode 会话缓存读写。

use std::sync::{Mutex, OnceLock};

use crate::tooling::opencode_session::types::{
    OpenCodeSessionCache, OpenCodeSessionCacheEntry, OpenCodeSessionState, OpenCodeStorageStamp,
};

static OPENCODE_SESSION_CACHE: OnceLock<Mutex<OpenCodeSessionCache>> = OnceLock::new();

/// 生成缓存键。
pub(super) fn opencode_cache_key(normalized_cwd: &str) -> String {
    if normalized_cwd.is_empty() {
        "__global__".to_string()
    } else {
        normalized_cwd.to_string()
    }
}

/// 命中缓存并返回会话状态。
pub(super) fn read_cached_opencode_state(
    cache_key: &str,
    stamp: &OpenCodeStorageStamp,
) -> Option<OpenCodeSessionState> {
    let cache = opencode_session_cache().lock().ok()?;
    let entry = cache.by_cwd.get(cache_key)?;
    if &entry.stamp != stamp {
        return None;
    }
    Some(entry.state.clone())
}

/// 写入缓存。
pub(super) fn write_cached_opencode_state(
    cache_key: String,
    stamp: OpenCodeStorageStamp,
    state: OpenCodeSessionState,
) {
    let Ok(mut cache) = opencode_session_cache().lock() else {
        return;
    };

    if cache.by_cwd.len() >= 256 {
        cache.by_cwd.clear();
    }
    cache
        .by_cwd
        .insert(cache_key, OpenCodeSessionCacheEntry { stamp, state });
}

/// 清理缓存。
pub(super) fn evict_cached_opencode_state(cache_key: &str) {
    let Ok(mut cache) = opencode_session_cache().lock() else {
        return;
    };
    cache.by_cwd.remove(cache_key);
}

/// 获取全局缓存实例。
fn opencode_session_cache() -> &'static Mutex<OpenCodeSessionCache> {
    OPENCODE_SESSION_CACHE.get_or_init(|| Mutex::new(OpenCodeSessionCache::default()))
}
