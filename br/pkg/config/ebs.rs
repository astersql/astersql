// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// See the License for the specific language governing permissions and
// limitations under the License.

//! EBS-based backup/restore metadata ported from `br/pkg/config/ebs.go`.
//!
//! EBS 卷级全量备份/恢复的元数据模型与校验逻辑，对照 Go `br/pkg/config/ebs.go`。
//! 部署工具（TiDB Operator / 未来 TiUP）产出 JSON；BR 在此解析、补全快照 ID/AZ，
//! 并在落盘或读外部存储后做版本、resolved-ts、TiKV store 完整性检查。
//! Kubernetes 侧 PV/PVC/CRD 在 Rust 中以不透明 JSON 保留，避免强依赖 k8s 类型。

use std::collections::HashMap;

use astersql_objstore_storeapi::{Context, Storage};
use serde::{Deserialize, Serialize};

/// Error type mirroring the `github.com/pingcap/errors` values Go returns here.
/// 轻量错误包装：消息字符串与 Go `errors.New` / `Annotatef` 文本对齐，便于契约测试。
#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// 与 Go `EBSVolumeType` 字符串别名等价；校验走独立函数而非方法。
pub type EBSVolumeType = String;

/// AWS gp3 通用型；BR 当前支持的三种 EBS 类型之一。
pub const GP3Volume: &str = "gp3";
/// 预置 IOPS io1。
pub const IO1Volume: &str = "io1";
/// 预置 IOPS io2。
pub const IO2Volume: &str = "io2";

/// Valid reports whether the volume type is one of the supported EBS types.
/// 对照 Go `(EBSVolumeType).Valid`：仅 gp3/io1/io2 为真，其余（如 gp2）拒绝。
pub fn EBSVolumeType_Valid(volume_type: &str) -> bool {
    volume_type == GP3Volume || volume_type == IO1Volume || volume_type == IO2Volume
}

// EBSVolume is passed by TiDB deployment tools: TiDB Operator and TiUP(in future)
// we should do snapshot inside BR, because we need some logic to determine the order of snapshot starts.
// TODO finish the info with TiDB Operator developer.
/// 单块 EBS 卷描述：部署工具传入 volume_id/type，BR 在快照流程中回填
/// SnapshotID / RestoreVolumeId / VolumeAZ；JSON 字段名与 Go struct tag 一致。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EBSVolume {
    #[serde(rename = "volume_id", default)]
    pub ID: String,
    #[serde(rename = "type", default)]
    pub Type: String,
    #[serde(rename = "snapshot_id", default)]
    pub SnapshotID: String,
    #[serde(rename = "restore_volume_id", default)]
    pub RestoreVolumeId: String,
    #[serde(rename = "volume_az", default)]
    pub VolumeAZ: String,
    #[serde(rename = "status", default)]
    pub Status: String,
}

/// 一个 TiKV store 及其挂载的 EBS 卷列表；StoreID 对应 PD store id。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EBSStore {
    #[serde(rename = "store_id", default)]
    pub StoreID: u64,
    #[serde(
        rename = "volumes",
        default,
        deserialize_with = "deserialize_null_default"
    )]
    pub Volumes: Vec<EBSVolume>,
}

// ClusterInfo represents the tidb cluster level meta infos. such as
// pd cluster id/alloc id, cluster resolved ts and tikv configuration.
/// 集群级元信息：版本、全备类型、resolved-ts 与副本计数等。
/// `Version` 同时接受 `cluster_version` 与历史别名 `version`。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ClusterInfo {
    #[serde(rename = "cluster_version", alias = "version", default)]
    pub Version: String,
    #[serde(rename = "full_backup_type", default)]
    pub FullBackupType: String,
    #[serde(rename = "resolved_ts", default)]
    pub ResolvedTS: u64,
    #[serde(
        rename = "replicas",
        default,
        deserialize_with = "deserialize_null_default"
    )]
    pub Replicas: HashMap<String, u64>,
}

/// Kubernetes metadata. The Go type embeds full corev1 PV/PVC objects; the
/// deployment tools emit them as opaque JSON, kept verbatim here.
/// Go 嵌入 corev1 强类型；Rust 用 `serde_json::Value` 原样往返，避免 k8s crate 依赖。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Kubernetes {
    #[serde(rename = "pvs", default, deserialize_with = "deserialize_null_default")]
    pub PVs: Vec<serde_json::Value>,
    #[serde(
        rename = "pvcs",
        default,
        deserialize_with = "deserialize_null_default"
    )]
    pub PVCs: Vec<serde_json::Value>,
    #[serde(rename = "crd_tidb_cluster", default)]
    pub CRD: serde_json::Value,
    #[serde(
        rename = "options",
        default,
        deserialize_with = "deserialize_null_default"
    )]
    pub Options: HashMap<String, serde_json::Value>,
}

/// TiKV 组件拓扑：副本数与各 store 的卷清单。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TiKVComponent {
    #[serde(rename = "replicas", default)]
    pub Replicas: i64,
    #[serde(
        rename = "stores",
        default,
        deserialize_with = "deserialize_null_default"
    )]
    pub Stores: Vec<EBSStore>,
}

/// PD 组件副本元数据（当前仅 replicas）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PDComponent {
    #[serde(rename = "replicas", default)]
    pub Replicas: i64,
}

/// TiDB 组件副本元数据（当前仅 replicas）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TiDBComponent {
    #[serde(rename = "replicas", default)]
    pub Replicas: i64,
}

/// EBS 全备元数据根对象：聚合集群信息、各组件拓扑、K8s 元数据与 AWS region。
/// JSON 键名与 Go `EBSBasedBRMeta` 一致，供 Operator 产物与 BR 互读。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EBSBasedBRMeta {
    #[serde(rename = "cluster_info", default)]
    pub ClusterInfo: Option<ClusterInfo>,
    #[serde(rename = "tikv", default)]
    pub TiKVComponent: Option<TiKVComponent>,
    #[serde(rename = "tidb", default)]
    pub TiDBComponent: Option<TiDBComponent>,
    #[serde(rename = "pd", default)]
    pub PDComponent: Option<PDComponent>,
    #[serde(rename = "kubernetes", default)]
    pub KubernetesMeta: Option<Kubernetes>,
    #[serde(
        rename = "options",
        default,
        deserialize_with = "deserialize_null_default"
    )]
    pub Options: HashMap<String, serde_json::Value>,
    #[serde(rename = "region", default)]
    pub Region: String,
}

impl EBSBasedBRMeta {
    /// 返回 TiKV store 个数；组件缺失时为 0（与 Go 一致）。
    pub fn GetStoreCount(&self) -> u64 {
        match &self.TiKVComponent {
            None => 0,
            Some(tikv) => tikv.Stores.len() as u64,
        }
    }

    /// 假设各 TiKV 节点卷布局对称，取首个 store 的卷数作为每节点卷数。
    pub fn GetTiKVVolumeCount(&self) -> u64 {
        match &self.TiKVComponent {
            Some(tikv) if !tikv.Stores.is_empty() => {
                // Assume TiKV nodes are symmetric
                tikv.Stores[0].Volumes.len() as u64
            }
            _ => 0,
        }
    }

    /// 序列化为 JSON；失败时返回 `"<nil>"`，对齐 Go `String()` 容错行为。
    pub fn String(&self) -> String {
        match serde_json::to_string(self) {
            Ok(cfg) => cfg,
            Err(_) => "<nil>".to_string(),
        }
    }

    // ConfigFromFile loads config from file.
    /// 从本地文件读取并按 Go `json.Unmarshal` 语义更新 `self`；缺失字段保持原值。
    pub fn ConfigFromFile(&mut self, path: &str) -> Result<(), Error> {
        let data = std::fs::read(path).map_err(|err| Error(err.to_string()))?;
        let incoming: serde_json::Value =
            serde_json::from_slice(&data).map_err(|err| Error(err.to_string()))?;
        // Go json.Unmarshal into a non-nil struct preserves fields absent from
        // the input object. Merge first so repeated ConfigFromFile calls have
        // the same behavior instead of replacing the complete Rust value.
        if !incoming.is_null() {
            let mut merged = serde_json::to_value(&*self).map_err(|err| Error(err.to_string()))?;
            merge_json_value(&mut merged, incoming);
            *self = serde_json::from_value(merged).map_err(|err| Error(err.to_string()))?;
        }
        Ok(())
    }

    /// Setter 前置：若 `ClusterInfo` 为空则惰性初始化，避免空指针式 panic。
    pub fn CheckClusterInfo(&mut self) {
        if self.ClusterInfo.is_none() {
            self.ClusterInfo = Some(ClusterInfo::default());
        }
    }

    /// 完整性校验，错误顺序与 Go `checkEBSBRMeta` 对齐：
    /// 缺集群信息 → 版本非法 → resolved-ts 为 0 → TiKV store 为空。
    /// 版本格式按 Masterminds/semver v1 `NewVersion` 的宽松规则校验。
    pub(crate) fn checkEBSBRMeta(&self) -> Result<(), Error> {
        let Some(cluster_info) = &self.ClusterInfo else {
            return Err(Error("no cluster info".to_string()));
        };
        if !masterminds_semver_is_valid(&cluster_info.Version) {
            return Err(Error(format!(
                "invalid cluster version: {}",
                cluster_info.Version
            )));
        }
        if cluster_info.ResolvedTS == 0 {
            return Err(Error("invalid resolved ts".to_string()));
        }
        if self.GetStoreCount() == 0 {
            return Err(Error("tikv info is empty".to_string()));
        }
        Ok(())
    }

    /// 写入 resolved-ts；必要时先 `CheckClusterInfo`。
    pub fn SetResolvedTS(&mut self, id: u64) {
        self.CheckClusterInfo();
        self.ClusterInfo.as_mut().expect("cluster info").ResolvedTS = id;
    }

    /// 读取 resolved-ts；调用方须保证 ClusterInfo 已初始化。
    pub fn GetResolvedTS(&self) -> u64 {
        self.ClusterInfo.as_ref().expect("cluster info").ResolvedTS
    }

    /// 设置全备类型（如 `aws-ebs`），供恢复路径区分策略。
    pub fn SetFullBackupType(&mut self, backup_type: String) {
        self.CheckClusterInfo();
        self.ClusterInfo
            .as_mut()
            .expect("cluster info")
            .FullBackupType = backup_type;
    }

    /// 返回全备类型字符串副本。
    pub fn GetFullBackupType(&self) -> String {
        self.ClusterInfo
            .as_ref()
            .expect("cluster info")
            .FullBackupType
            .clone()
    }

    /// 写入集群版本字符串（通常含或不含 `v` 前缀均可，校验时再规范化）。
    pub fn SetClusterVersion(&mut self, version: String) {
        self.CheckClusterInfo();
        self.ClusterInfo.as_mut().expect("cluster info").Version = version;
    }

    /// 按 volume_id → snapshot_id 映射回填所有 store 的快照 ID；缺键写空串。
    /// 要求 `TiKVComponent` 已存在，否则 panic（与 Go 直接解引用一致）。
    pub fn SetSnapshotIDs(&mut self, idMap: &HashMap<String, String>) {
        for store in &mut self.TiKVComponent.as_mut().expect("tikv component").Stores {
            for volume in &mut store.Volumes {
                volume.SnapshotID = idMap.get(&volume.ID).cloned().unwrap_or_default();
            }
        }
    }

    /// 回填恢复目标卷 ID（volume_id → restore_volume_id）。
    pub fn SetRestoreVolumeIDs(&mut self, idMap: &HashMap<String, String>) {
        for store in &mut self.TiKVComponent.as_mut().expect("tikv component").Stores {
            for volume in &mut store.Volumes {
                volume.RestoreVolumeId = idMap.get(&volume.ID).cloned().unwrap_or_default();
            }
        }
    }

    /// 回填卷可用区（volume_id → volume_az）。
    pub fn SetVolumeAZs(&mut self, idMap: &HashMap<String, String>) {
        for store in &mut self.TiKVComponent.as_mut().expect("tikv component").Stores {
            for volume in &mut store.Volumes {
                volume.VolumeAZ = idMap.get(&volume.ID).cloned().unwrap_or_default();
            }
        }
    }
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

fn merge_json_value(current: &mut serde_json::Value, incoming: serde_json::Value) {
    match (current, incoming) {
        (serde_json::Value::Object(current), serde_json::Value::Object(incoming)) => {
            for (key, value) in incoming {
                match current.get_mut(&key) {
                    Some(existing) => merge_json_value(existing, value),
                    None => {
                        current.insert(key, value);
                    }
                }
            }
        }
        (current, incoming) => *current = incoming,
    }
}

/// Matches Masterminds/semver v1 `NewVersion`: lowercase `v` is optional,
/// minor and patch are optional, leading zeroes are accepted, and prerelease
/// and metadata consist of non-empty dot-separated ASCII identifiers.
fn masterminds_semver_is_valid(version: &str) -> bool {
    let version = version.strip_prefix('v').unwrap_or(version);
    let mut metadata_parts = version.split('+');
    let core_and_pre = metadata_parts.next().unwrap_or_default();
    let metadata = metadata_parts.next();
    if metadata_parts.next().is_some()
        || metadata.is_some_and(|part| !semver_identifiers_valid(part))
    {
        return false;
    }

    let (core, prerelease) = core_and_pre
        .split_once('-')
        .map_or((core_and_pre, None), |(core, pre)| (core, Some(pre)));
    if prerelease.is_some_and(|part| !semver_identifiers_valid(part)) {
        return false;
    }
    let components: Vec<&str> = core.split('.').collect();
    (1..=3).contains(&components.len())
        && components.iter().all(|component| {
            !component.is_empty()
                && component.bytes().all(|byte| byte.is_ascii_digit())
                && component.parse::<i64>().is_ok()
        })
}

fn semver_identifiers_valid(identifiers: &str) -> bool {
    !identifiers.is_empty()
        && identifiers.split('.').all(|identifier| {
            !identifier.is_empty()
                && identifier
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

/// NewMetaFromStorage reads `metautil::MetaFile` from external storage,
/// unmarshals and validates it, mirroring the Go error order.
/// 从外部存储读取 `MetaFile` → JSON 反序列化 → `checkEBSBRMeta`；
/// 任一步失败即返回，错误顺序与 Go `NewMetaFromStorage` 一致。
pub fn NewMetaFromStorage(ctx: &Context, storage: &dyn Storage) -> Result<EBSBasedBRMeta, Error> {
    let metaBytes = storage
        .ReadFile(ctx, astersql_br_pkg_metautil::metafile::MetaFile)
        .map_err(|err| Error(err.to_string()))?;
    let metaInfo: EBSBasedBRMeta =
        serde_json::from_slice(&metaBytes).map_err(|err| Error(err.to_string()))?;
    metaInfo.checkEBSBRMeta()?;
    Ok(metaInfo)
}
