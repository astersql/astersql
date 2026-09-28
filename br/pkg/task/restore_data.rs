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
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Resolve KV data (EBS) matching `br/pkg/task/restore_data.go`.
//!
//! 本文件对齐 Go `br/pkg/task/restore_data.go`：从 EBS 备份元数据读取
//! `resolved_ts` / 副本数，并驱动「解析 KV 数据」任务的生命周期。
//! 真实 TiKV resolve 在此路径上仍是桩；重点是元数据校验与进度条/Summary 收尾。

use crate::common::{FullBackupTypeEBS, GetKeepalive, NewMgr};
use crate::restore::RestoreConfig;
use crate::stubs::{CollectInt, Error, Glue, Mgr, Result, SetSuccessStatus, Storage, Summary};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
struct ClusterInfo {
    #[serde(default)]
    full_backup_type: String,
    #[serde(default)]
    resolved_ts: u64,
}

#[derive(Deserialize)]
struct TiKVMeta {
    #[serde(default)]
    replicas: i32,
}

#[derive(Deserialize)]
struct BackupMeta {
    #[serde(default)]
    cluster_info: Option<ClusterInfo>,
    // Older operator artifacts placed these two fields at the root. Keep
    // reading them when the backup type is explicit; never default to EBS.
    #[serde(default)]
    full_backup_type: String,
    #[serde(default)]
    resolved_ts: u64,
    #[serde(default)]
    tikv: Option<TiKVMeta>,
}

/// 读取 `backupmeta.json`，校验全量类型为 AWS-EBS，并返回 resolve TS 与副本数。
/// 与 Go 的 `GetFullBackupType` 检查一致：非 EBS 立即失败。
pub fn ReadBackupMetaData(storage: &dyn Storage) -> Result<(u64, i32)> {
    let raw = storage.ReadFile("backupmeta.json")?;
    let meta: BackupMeta = serde_json::from_slice(&raw).map_err(|e| Error::new(e.to_string()))?;
    let (full_backup_type, resolve_ts) = match meta.cluster_info.as_ref() {
        Some(info) => (info.full_backup_type.as_str(), info.resolved_ts),
        None => (meta.full_backup_type.as_str(), meta.resolved_ts),
    };
    if full_backup_type != FullBackupTypeEBS {
        return Err(Error::new("invalid meta file, only support aws-ebs now"));
    }
    let replicas = meta
        .tikv
        .as_ref()
        .map(|tikv| tikv.replicas)
        .unwrap_or_default();
    Ok((resolve_ts, replicas))
}

/// 入口：`br restore` 的 resolve-kv-data 路径；对齐 Go `RunResolveKvData`。
/// 会 Adjust 配置、写 Summary、建 PD/TiKV Mgr，再推进进度条并标记成功。
pub fn RunResolveKvData(
    g: &dyn Glue,
    cmdName: &str,
    cfg: &mut RestoreConfig,
    storage: Arc<dyn Storage>,
) -> Result<()> {
    cfg.Adjust();
    Summary(cmdName);
    let (resolveTS, numStores) = ReadBackupMetaData(storage.as_ref())?;
    // 指标侧记录 resolve-ts，便于与 Go CollectInt 对照。
    CollectInt("resolve-ts", resolveTS as i64);
    let mgr = NewMgr(
        g,
        &cfg.Config.KeyspaceName,
        &cfg.Config.PD,
        &cfg.Config.TLS,
        GetKeepalive(&cfg.Config),
        cfg.Config.CheckRequirements,
        false,
        crate::stubs::NormalVersionChecker,
    )?;
    struct MgrCloseGuard(Arc<dyn Mgr>);
    impl Drop for MgrCloseGuard {
        fn drop(&mut self) {
            self.0.Close();
        }
    }
    let _mgr_close = MgrCloseGuard(mgr);

    // The local task boundary has no live PD store enumeration. Its injected
    // metadata replica count is therefore the store count used by the mock
    // recovery lifecycle; preserve Go's read/send/iterate + two flashback units.
    let progress_total = i64::from(numStores).saturating_mul(3).saturating_add(2);
    let updateCh = g.StartProgress(cmdName, progress_total, !cfg.Config.LogProgress);
    updateCh.IncBy(progress_total);
    updateCh.Close();
    SetSuccessStatus(true);
    Ok(())
}
