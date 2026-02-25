//! 本地状态存储模块职责：
//! 1. 维护工具白名单（接入/断开）持久化。
//! 2. 维护控制端设备白名单（授权绑定）持久化。
//! 3. 提供最小化文件读写封装，保证主流程只关心业务语义。

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

fn openclaw_identity_hash(tool_id: &str) -> Option<&str> {
    let rest = tool_id.strip_prefix("openclaw_")?;

    if let Some(hash) = rest.strip_suffix("_gw")
        && !hash.trim().is_empty()
    {
        return Some(hash);
    }

    if let Some((hash, pid_text)) = rest.rsplit_once("_p")
        && !hash.trim().is_empty()
        && !pid_text.trim().is_empty()
        && pid_text.chars().all(|ch| ch.is_ascii_digit())
    {
        return Some(hash);
    }

    None
}

/// 工具白名单文件结构。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolWhitelistFile {
    /// 已接入工具 ID 列表。
    #[serde(default)]
    tool_ids: Vec<String>,
}

/// 工具白名单存储。
#[derive(Debug, Clone)]
pub(crate) struct ToolWhitelistStore {
    /// 白名单文件路径；为空时表示无法落盘（例如 HOME 缺失）。
    path: Option<PathBuf>,
    /// 内存中的白名单集合。
    ids: HashSet<String>,
}

impl ToolWhitelistStore {
    /// 从本地文件加载白名单；解析失败时回退为空集合。
    pub(crate) fn load() -> Self {
        let path = tool_whitelist_path();
        let Some(path_ref) = path.as_ref() else {
            return Self {
                path: None,
                ids: HashSet::new(),
            };
        };

        let bytes = match fs::read(path_ref) {
            Ok(value) => value,
            Err(_) => {
                return Self {
                    path,
                    ids: HashSet::new(),
                };
            }
        };

        let parsed = serde_json::from_slice::<ToolWhitelistFile>(&bytes).unwrap_or_else(|err| {
            warn!("load tool whitelist failed: {err}");
            ToolWhitelistFile::default()
        });

        Self {
            path,
            ids: parsed
                .tool_ids
                .into_iter()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect(),
        }
    }

    /// 判断工具是否已在白名单中。
    pub(crate) fn contains(&self, tool_id: &str) -> bool {
        self.ids.contains(tool_id)
    }

    /// 返回当前白名单工具 ID（已排序），用于快照补齐离线占位项。
    pub(crate) fn list_ids(&self) -> Vec<String> {
        let mut ids = self.ids.iter().cloned().collect::<Vec<String>>();
        ids.sort();
        ids
    }

    /// 兼容 OpenClaw 的实例 ID 漂移：
    /// `openclaw_<hash>_gw` 与 `openclaw_<hash>_p<pid>` 视为同一逻辑身份。
    /// 用于避免重启或 PID 漂移后卡片重复/离线。
    pub(crate) fn contains_compatible(&self, tool_id: &str) -> bool {
        if self.contains(tool_id) {
            return true;
        }

        let Some(hash) = openclaw_identity_hash(tool_id) else {
            return false;
        };

        if self
            .ids
            .iter()
            .filter_map(|id| openclaw_identity_hash(id))
            .any(|existing_hash| existing_hash == hash)
        {
            return true;
        }

        // 单宿主 OpenClaw 单实例策略：当白名单里仅有一个 OpenClaw 身份时，
        // 允许 hash 迁移（例如网关重启后 cwd 变化），避免出现“已接入但卡片离线占位”。
        self.ids
            .iter()
            .filter_map(|id| openclaw_identity_hash(id))
            .count()
            == 1
    }

    /// 将工具加入白名单并立即落盘；返回是否实际发生变更。
    pub(crate) fn add(&mut self, tool_id: &str) -> anyhow::Result<bool> {
        let before = self.ids.clone();

        if openclaw_identity_hash(tool_id).is_some() {
            // OpenClaw 按“单实例”维护：新实例接入时覆盖旧身份，避免残留旧 hash。
            self.ids
                .retain(|existing| openclaw_identity_hash(existing).is_none());
        }

        self.ids.insert(tool_id.to_string());
        if self.ids == before {
            return Ok(false);
        }
        self.save()?;
        Ok(true)
    }

    /// 将工具移出白名单并立即落盘；返回是否实际发生变更。
    pub(crate) fn remove(&mut self, tool_id: &str) -> anyhow::Result<bool> {
        let before = self.ids.clone();

        if self.ids.remove(tool_id) {
            self.save()?;
            return Ok(true);
        }

        if let Some(hash) = openclaw_identity_hash(tool_id) {
            let mut removed_by_hash = false;
            self.ids.retain(|existing| {
                let matched = openclaw_identity_hash(existing) == Some(hash);
                if matched {
                    removed_by_hash = true;
                }
                !matched
            });

            if !removed_by_hash {
                // 当 hash 已漂移时（例如 gateway 身份变更），兜底移除唯一 OpenClaw 绑定。
                let openclaw_ids = self
                    .ids
                    .iter()
                    .filter(|id| openclaw_identity_hash(id).is_some())
                    .cloned()
                    .collect::<Vec<String>>();
                if openclaw_ids.len() == 1 {
                    self.ids.remove(&openclaw_ids[0]);
                }
            }
        }

        if self.ids == before {
            return Ok(false);
        }
        self.save()?;
        Ok(true)
    }

    /// 清空白名单并落盘；返回本次移除的工具数量。
    pub(crate) fn clear(&mut self) -> anyhow::Result<usize> {
        let removed = self.ids.len();
        if removed == 0 {
            return Ok(0);
        }
        self.ids.clear();
        self.save()?;
        Ok(removed)
    }

    /// 持久化白名单：创建目录、排序后写入 JSON。
    fn save(&self) -> anyhow::Result<()> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut tool_ids = self
            .ids
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<String>>();
        tool_ids.sort();

        let bytes = serde_json::to_vec_pretty(&ToolWhitelistFile { tool_ids })?;
        fs::write(path, bytes)?;
        Ok(())
    }

    #[cfg(test)]
    /// 测试辅助：从给定工具 ID 构造内存白名单（不落盘）。
    pub(crate) fn from_ids_for_test(ids: &[&str]) -> Self {
        Self {
            path: None,
            ids: ids
                .iter()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect(),
        }
    }
}

/// 控制设备白名单文件结构。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ControllerDevicesFile {
    /// 允许发控制命令的设备 ID 列表。
    #[serde(default)]
    device_ids: Vec<String>,
}

/// 控制设备白名单存储。
#[derive(Debug, Clone)]
pub(crate) struct ControllerDevicesStore {
    /// 存储文件路径。
    path: Option<PathBuf>,
    /// 内存集合，避免重复查询文件。
    ids: HashSet<String>,
}

impl ControllerDevicesStore {
    /// 从本地文件加载控制设备列表；失败时返回空集合。
    pub(crate) fn load() -> Self {
        let path = controller_devices_path();
        let Some(path_ref) = path.as_ref() else {
            return Self {
                path: None,
                ids: HashSet::new(),
            };
        };

        let bytes = match fs::read(path_ref) {
            Ok(value) => value,
            Err(_) => {
                return Self {
                    path,
                    ids: HashSet::new(),
                };
            }
        };

        let parsed =
            serde_json::from_slice::<ControllerDevicesFile>(&bytes).unwrap_or_else(|err| {
                warn!("load controller devices failed: {err}");
                ControllerDevicesFile::default()
            });

        Self {
            path,
            ids: parsed
                .device_ids
                .into_iter()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect(),
        }
    }

    /// 校验命令来源是否有权限；必要时执行首次绑定。
    pub(crate) fn authorize_or_bind(
        &mut self,
        source_client_type: &str,
        source_device_id: &str,
        allow_first_bind: bool,
    ) -> anyhow::Result<(bool, String)> {
        // 仅允许 app 客户端发控制命令，sidecar/其他类型全部拒绝。
        if source_client_type != "app" {
            return Ok((false, "仅接受 app 客户端控制命令。".to_string()));
        }

        let device_id = source_device_id.trim();
        if device_id.is_empty() {
            return Ok((false, "缺少来源设备标识。".to_string()));
        }

        // 未绑定任何设备时可按配置自动绑定首个设备，降低首启门槛。
        if self.ids.is_empty() {
            if !allow_first_bind {
                return Ok((
                    false,
                    "当前未绑定控制设备。请在 sidecar 环境变量设置 CONTROLLER_DEVICE_IDS，或开启 ALLOW_FIRST_CONTROLLER_BIND。"
                        .to_string(),
                ));
            }
            self.ids.insert(device_id.to_string());
            self.save()?;
            info!("controller device bound: {device_id}");
            return Ok((true, String::new()));
        }

        if self.ids.contains(device_id) {
            return Ok((true, String::new()));
        }

        Ok((false, "该设备未被授权控制当前 sidecar。".to_string()))
    }

    /// 用环境变量预置设备 ID 初始化白名单。
    pub(crate) fn seed(&mut self, device_ids: &[String]) -> anyhow::Result<()> {
        let mut changed = false;
        for device_id in device_ids {
            let value = device_id.trim();
            if value.is_empty() {
                continue;
            }
            if self.ids.insert(value.to_string()) {
                changed = true;
            }
        }
        if changed {
            self.save()?;
        }
        Ok(())
    }

    /// 把控制端白名单重绑为单个设备（覆盖原集合）。
    pub(crate) fn rebind(&mut self, device_id: &str) -> anyhow::Result<bool> {
        let value = device_id.trim();
        if value.is_empty() {
            return Ok(false);
        }

        let unchanged = self.ids.len() == 1 && self.ids.contains(value);
        if unchanged {
            return Ok(false);
        }

        self.ids.clear();
        self.ids.insert(value.to_string());
        self.save()?;
        info!("controller device rebound: {value}");
        Ok(true)
    }

    /// 持久化控制设备列表。
    fn save(&self) -> anyhow::Result<()> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut device_ids = self
            .ids
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<String>>();
        device_ids.sort();
        let bytes = serde_json::to_vec_pretty(&ControllerDevicesFile { device_ids })?;
        fs::write(path, bytes)?;
        Ok(())
    }
}

/// 工具白名单文件路径：`~/.config/yourconnector/sidecar/tool-whitelist.json`。
fn tool_whitelist_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    if home.trim().is_empty() {
        return None;
    }
    Some(
        Path::new(&home)
            .join(".config")
            .join("yourconnector")
            .join("sidecar")
            .join("tool-whitelist.json"),
    )
}

/// 控制设备白名单文件路径：`~/.config/yourconnector/sidecar/controller-devices.json`。
fn controller_devices_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    if home.trim().is_empty() {
        return None;
    }
    Some(
        Path::new(&home)
            .join(".config")
            .join("yourconnector")
            .join("sidecar")
            .join("controller-devices.json"),
    )
}

#[cfg(test)]
mod tests {
    use super::{ToolWhitelistStore, openclaw_identity_hash};

    #[test]
    fn openclaw_identity_hash_should_support_gateway_and_pid_variants() {
        assert_eq!(
            openclaw_identity_hash("openclaw_abcd1234ef56_gw"),
            Some("abcd1234ef56")
        );
        assert_eq!(
            openclaw_identity_hash("openclaw_abcd1234ef56_p1024"),
            Some("abcd1234ef56")
        );
        assert_eq!(openclaw_identity_hash("opencode_xxx_p1"), None);
    }

    #[test]
    fn contains_compatible_should_match_openclaw_pid_drift() {
        let whitelist = ToolWhitelistStore::from_ids_for_test(&["openclaw_abcd1234ef56_p1024"]);
        assert!(whitelist.contains_compatible("openclaw_abcd1234ef56_p2048"));
        assert!(whitelist.contains_compatible("openclaw_abcd1234ef56_gw"));
        assert!(whitelist.contains_compatible("openclaw_ffffeeee1111_p2048"));
    }

    #[test]
    fn contains_compatible_should_require_hash_match_when_multiple_openclaw_bound() {
        let whitelist = ToolWhitelistStore::from_ids_for_test(&[
            "openclaw_abcd1234ef56_p1024",
            "openclaw_ffffeeee1111_p2048",
        ]);
        assert!(whitelist.contains_compatible("openclaw_abcd1234ef56_gw"));
        assert!(!whitelist.contains_compatible("openclaw_deadbeef9999_gw"));
    }

    #[test]
    fn remove_should_drop_openclaw_compatible_identity() {
        let mut whitelist = ToolWhitelistStore::from_ids_for_test(&["openclaw_abcd1234ef56_p1024"]);
        let changed = whitelist
            .remove("openclaw_abcd1234ef56_p2048")
            .expect("remove should succeed");
        assert!(changed);
        assert!(!whitelist.contains_compatible("openclaw_abcd1234ef56_gw"));
    }

    #[test]
    fn add_should_replace_old_openclaw_identity_under_single_instance_policy() {
        let mut whitelist = ToolWhitelistStore::from_ids_for_test(&["openclaw_abcd1234ef56_gw"]);
        let changed = whitelist
            .add("openclaw_ffffeeee1111_gw")
            .expect("add should succeed");
        assert!(changed);

        let ids = whitelist.list_ids();
        assert_eq!(ids, vec!["openclaw_ffffeeee1111_gw".to_string()]);
    }

    #[test]
    fn remove_should_drop_single_openclaw_even_when_hash_drifted() {
        let mut whitelist = ToolWhitelistStore::from_ids_for_test(&["openclaw_abcd1234ef56_gw"]);
        let changed = whitelist
            .remove("openclaw_ffffeeee1111_gw")
            .expect("remove should succeed");
        assert!(changed);
        assert!(whitelist.list_ids().is_empty());
    }
}
