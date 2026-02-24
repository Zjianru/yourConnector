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

    /// 兼容 OpenClaw gateway 的历史实例 ID：
    /// 老版本使用 `openclaw_<hash>_p<pid>`，新版本使用 `openclaw_<hash>_gw`。
    /// 该方法在白名单判断时提供平滑过渡，避免升级后工具“看似掉线”。
    pub(crate) fn contains_compatible(&self, tool_id: &str) -> bool {
        if self.contains(tool_id) {
            return true;
        }

        let Some(hash) = tool_id
            .strip_prefix("openclaw_")
            .and_then(|rest| rest.strip_suffix("_gw"))
        else {
            return false;
        };

        let legacy_prefix = format!("openclaw_{hash}_p");
        self.ids.iter().any(|id| id.starts_with(&legacy_prefix))
    }

    /// 将工具加入白名单并立即落盘；返回是否实际发生变更。
    pub(crate) fn add(&mut self, tool_id: &str) -> anyhow::Result<bool> {
        if !self.ids.insert(tool_id.to_string()) {
            return Ok(false);
        }
        self.save()?;
        Ok(true)
    }

    /// 将工具移出白名单并立即落盘；返回是否实际发生变更。
    pub(crate) fn remove(&mut self, tool_id: &str) -> anyhow::Result<bool> {
        if !self.ids.remove(tool_id) {
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
