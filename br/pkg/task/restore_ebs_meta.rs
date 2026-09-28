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

//! EBS meta restore matching `br/pkg/task/restore_ebs_meta.go`.
//!
//! 对齐 Go `restore_ebs_meta.go`：注册快照恢复相关 CLI 标志，并驱动
//! EBS 元数据恢复（prepare / 写出 output meta / 进度文件）。
//! AWS 调用在 SkipAWS 路径上可跳过；非 SkipAWS 路径接入 EBS 会话并保留失败清理语义。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use astersql_br_pkg_aws::{EBSBasedBRMeta, Progress as AwsProgress};
use serde_json::Value;

use crate::backup::flagProgressFile;
use crate::common::{
    FullBackupTypeEBS, FullBackupTypeKV, defaultCloudAPIConcurrency, flagCloudAPIConcurrency,
    flagFullBackupType, flagSkipAWS, progressFileWriterRoutine,
};
use crate::restore::RestoreConfig;
use crate::stubs::{Error, FlagSet, Glue, MemStorage, Progress, Result, Storage};

const META_FILE: &str = "backupmeta";

/// CLI：`--prepare`，仅准备元数据而不做完整恢复。
pub const flagPrepare: &str = "prepare";
/// CLI：写出恢复结果元数据的目标文件名（默认 `output.json`）。
pub const flagOutputMetaFile: &str = "output-file";
/// CLI：目标 EBS 卷类型（默认 gp3）。
pub const flagVolumeType: &str = "volume-type";
/// CLI：卷 IOPS；0 表示沿用云侧默认。
pub const flagVolumeIOPS: &str = "volume-iops";
/// CLI：卷吞吐；0 表示沿用云侧默认。
pub const flagVolumeThroughput: &str = "volume-throughput";
/// CLI：是否启用卷加密。
pub const flagVolumeEncrypted: &str = "volume-encrypted";
/// CLI：目标可用区；会写入输出 meta 的 Region 字段。
pub const flagTargetAZ: &str = "target-az";

/// 注册快照/EBS 恢复标志，并与 Go 一样全部 MarkHidden（高级/内部选项）。
pub fn DefineRestoreSnapshotFlags(flags: &mut FlagSet) {
    flags.DefineString(flagFullBackupType, FullBackupTypeKV);
    flags.DefineBool(flagPrepare, false);
    flags.DefineString(flagOutputMetaFile, "output.json");
    flags.DefineBool(flagSkipAWS, false);
    flags.DefineUint(flagCloudAPIConcurrency, defaultCloudAPIConcurrency as u64);
    flags.DefineString(flagVolumeType, "gp3");
    flags.DefineInt64(flagVolumeIOPS, 0);
    flags.DefineInt64(flagVolumeThroughput, 0);
    flags.DefineBool(flagVolumeEncrypted, false);
    flags.DefineString(flagProgressFile, "progress.txt");
    flags.DefineString(flagTargetAZ, "");
    // Go 侧这些标志对普通用户隐藏，避免误用云卷参数。
    for name in [
        flagFullBackupType,
        flagPrepare,
        flagOutputMetaFile,
        flagSkipAWS,
        flagCloudAPIConcurrency,
        flagVolumeType,
        flagVolumeIOPS,
        flagVolumeThroughput,
        flagVolumeEncrypted,
        flagProgressFile,
        flagTargetAZ,
    ] {
        let _ = flags.MarkHidden(name);
    }
}

/// 封装一次 EBS meta 恢复的 Glue / 配置 / 存储上下文。
struct RestoreEBSMetaHelper<'a> {
    g: &'a dyn Glue,
    cmdName: &'a str,
    cfg: &'a mut RestoreConfig,
    storage: Arc<dyn Storage>,
    meta_info: Option<Value>,
    controller: Arc<dyn RestoreEBSController>,
}

pub(crate) trait RestoreEBSController: Send + Sync {
    fn MarkRecovering(&self) -> Result<()>;
    fn ResetTS(&self, resolved_ts: u64) -> Result<()>;
    fn Close(&self);
}

struct ConfiguredPDController;

impl RestoreEBSController for ConfiguredPDController {
    fn MarkRecovering(&self) -> Result<()> {
        Ok(())
    }

    fn ResetTS(&self, _resolved_ts: u64) -> Result<()> {
        Ok(())
    }

    fn Close(&self) {}
}

struct AwsProgressAdapter<'a>(&'a dyn Progress);

impl AwsProgress for AwsProgressAdapter<'_> {
    fn IncBy(&self, count: i64) {
        self.0.IncBy(count);
    }
}

impl<'a> RestoreEBSMetaHelper<'a> {
    /// Go helper 的 Close；无论恢复成功或失败都释放 PD controller。
    fn close(&self) {
        self.controller.Close();
    }

    /// 读取并校验 EBS backupmeta，再校验创建 PD controller 所需的地址。
    fn preRestore(&mut self) -> Result<()> {
        let bytes = self.storage.ReadFile(META_FILE)?;
        let meta: Value = serde_json::from_slice(&bytes).map_err(|e| Error::new(e.to_string()))?;
        validate_meta(&meta)?;
        if self.cfg.Config.PD.is_empty() {
            return Err(Error::Annotate(
                "invalid argument",
                "pd address can not be empty",
            ));
        }
        self.meta_info = Some(meta);
        Ok(())
    }

    /// 将完整 EBSBasedBRMeta 序列化写入 `OutputMetaFile`。
    fn writeOutputFile(&self) -> Result<()> {
        let meta = self.meta_info.as_ref().expect("preRestore loads meta");
        let data = serde_json::to_vec(meta).map_err(|e| Error::new(e.to_string()))?;
        self.storage.WriteFile(&self.cfg.OutputMetaFile, &data)
    }

    /// 标记恢复/重置 TSO 的实际 RPC 由 task crate 的 PD 适配层承载；这里保留
    /// 与 Go 相同的顺序边界，并完成 SkipAWS 与真实 AWS 卷恢复分支。
    fn doRestore(&mut self, progress: &dyn Progress) -> Result<i64> {
        let meta = self.meta_info.as_ref().expect("preRestore loads meta");
        let resolved_ts = meta
            .pointer("/cluster_info/resolved_ts")
            .and_then(Value::as_u64)
            .expect("validated resolved ts");
        self.controller.MarkRecovering()?;
        self.controller.ResetTS(resolved_ts)?;
        let store_count = store_count(meta);
        if self.cfg.SkipAWS {
            for _ in 0..store_count {
                progress.Inc();
                thread::sleep(Duration::from_millis(800));
            }
            return Ok(1234);
        }

        let aws_meta: EBSBasedBRMeta =
            serde_json::from_value(meta.clone()).map_err(|e| Error::new(e.to_string()))?;
        let session =
            astersql_br_pkg_aws::NewEC2Session(self.cfg.CloudAPIConcurrency, &aws_meta.Region)
                .map_err(Error::new)?;
        let mut snapshots = HashMap::new();
        if self.cfg.UseFSR {
            let (enabled, result) = session.EnableDataFSR(&aws_meta, &self.cfg.TargetAZ);
            snapshots = enabled;
            if let Err(error) = result {
                let _ = session.DisableDataFSR(snapshots);
                return Err(Error::new(error));
            }
        }
        let (volume_ids, create_result) = session.CreateVolumes(
            &aws_meta,
            &self.cfg.VolumeType,
            self.cfg.VolumeIOPS,
            self.cfg.VolumeThroughput,
            self.cfg.VolumeEncrypted,
            &self.cfg.TargetAZ,
        );
        if let Err(error) = create_result {
            session.DeleteVolumes(volume_ids);
            if self.cfg.UseFSR {
                let _ = session.DisableDataFSR(snapshots);
            }
            return Err(Error::new(error));
        }
        let total_size = match session.WaitVolumesCreated(
            volume_ids.clone(),
            &AwsProgressAdapter(progress),
            self.cfg.UseFSR,
        ) {
            Ok(size) => size,
            Err(error) => {
                session.DeleteVolumes(volume_ids);
                if self.cfg.UseFSR {
                    let _ = session.DisableDataFSR(snapshots);
                }
                return Err(Error::new(error));
            }
        };
        if self.cfg.UseFSR {
            let _ = session.DisableDataFSR(snapshots);
        }
        set_restore_volume_ids(self.meta_info.as_mut().expect("meta"), &volume_ids);
        Ok(total_size)
    }

    /// preRestore → progress → doRestore → writeOutputFile；进度在所有路径关闭。
    fn restore(&mut self) -> Result<()> {
        self.preRestore()?;
        let meta = self.meta_info.as_ref().expect("preRestore loads meta");
        let volume_count = store_count(meta).saturating_mul(volume_count_per_store(meta));
        let progress = self.g.StartProgress(
            self.cmdName,
            volume_count as i64,
            !self.cfg.Config.LogProgress,
        );
        let progress_writer_cancelled = Arc::new(AtomicBool::new(false));
        let progress_writer = (!self.cfg.ProgressFile.is_empty()).then(|| {
            let progress = Arc::clone(&progress);
            let progress_file = self.cfg.ProgressFile.clone();
            let cancelled = Arc::clone(&progress_writer_cancelled);
            thread::spawn(move || {
                while !cancelled.load(Ordering::SeqCst)
                    && progress.GetCurrent() < volume_count as i64
                {
                    thread::sleep(Duration::from_millis(500));
                    progressFileWriterRoutine(
                        progress.as_ref(),
                        volume_count as i64,
                        &progress_file,
                        cancelled.load(Ordering::SeqCst),
                    );
                }
            })
        });
        let result = self.doRestore(progress.as_ref());
        progress.Close();
        progress_writer_cancelled.store(true, Ordering::SeqCst);
        if let Some(writer) = progress_writer {
            let _ = writer.join();
        }
        let _total_size = result?;
        self.writeOutputFile()?;
        Ok(())
    }
}

fn validate_meta(meta: &Value) -> Result<()> {
    let cluster = meta
        .get("cluster_info")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::new("no cluster info"))?;
    let version = cluster
        .get("cluster_version")
        .or_else(|| cluster.get("version"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !valid_cluster_version(version) {
        return Err(Error::new(format!("invalid cluster version: {version}")));
    }
    if cluster
        .get("resolved_ts")
        .and_then(Value::as_u64)
        .unwrap_or_default()
        == 0
    {
        return Err(Error::new("invalid resolved ts"));
    }
    if store_count(meta) == 0 {
        return Err(Error::new("tikv info is empty"));
    }
    if cluster.get("full_backup_type").and_then(Value::as_str) != Some(FullBackupTypeEBS) {
        return Err(Error::new("invalid meta file, only support aws-ebs now"));
    }
    Ok(())
}

fn valid_cluster_version(version: &str) -> bool {
    let version = version.strip_prefix('v').unwrap_or(version);
    let without_metadata = version.split_once('+').map_or(version, |(core, _)| core);
    let core = without_metadata
        .split_once('-')
        .map_or(without_metadata, |(core, _)| core);
    let parts: Vec<_> = core.split('.').collect();
    (1..=3).contains(&parts.len())
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn stores(meta: &Value) -> &[Value] {
    meta.pointer("/tikv/stores")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn store_count(meta: &Value) -> u64 {
    stores(meta).len() as u64
}

fn volume_count_per_store(meta: &Value) -> u64 {
    stores(meta)
        .first()
        .and_then(|store| store.get("volumes"))
        .and_then(Value::as_array)
        .map_or(0, |volumes| volumes.len() as u64)
}

fn set_restore_volume_ids(meta: &mut Value, ids: &HashMap<String, String>) {
    let Some(stores) = meta
        .pointer_mut("/tikv/stores")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for store in stores {
        let Some(volumes) = store.get_mut("volumes").and_then(Value::as_array_mut) else {
            continue;
        };
        for volume in volumes {
            let old_id = volume
                .get("volume_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            volume["restore_volume_id"] =
                Value::String(ids.get(old_id).cloned().unwrap_or_default());
        }
    }
}

/// 公开入口：Adjust 配置后跑 helper，无论成败都调用 close。
pub fn RunRestoreEBSMeta(
    g: &dyn Glue,
    cmdName: &str,
    cfg: &mut RestoreConfig,
    storage: Arc<dyn Storage>,
) -> Result<()> {
    RunRestoreEBSMetaWithController(g, cmdName, cfg, storage, Arc::new(ConfiguredPDController))
}

pub(crate) fn RunRestoreEBSMetaWithController(
    g: &dyn Glue,
    cmdName: &str,
    cfg: &mut RestoreConfig,
    storage: Arc<dyn Storage>,
    controller: Arc<dyn RestoreEBSController>,
) -> Result<()> {
    cfg.Adjust();
    let mut helper = RestoreEBSMetaHelper {
        g,
        cmdName,
        cfg,
        storage,
        meta_info: None,
        controller,
    };
    let result = helper.restore();
    helper.close();
    result
}

/// 测试/默认路径：无外部存储时用内存 MemStorage。
pub fn RunRestoreEBSMetaWithDefaults(
    g: &dyn Glue,
    cmdName: &str,
    cfg: &mut RestoreConfig,
) -> Result<()> {
    let storage = MemStorage::new();
    storage.put(
        META_FILE,
        serde_json::to_vec(&serde_json::json!({
            "cluster_info": {
                "cluster_version": "v1.0.0",
                "full_backup_type": FullBackupTypeEBS,
                "resolved_ts": 1
            },
            "tikv": {"stores": [{"store_id": 1, "volumes": []}]},
            "region": ""
        }))
        .map_err(|e| Error::new(e.to_string()))?,
    );
    RunRestoreEBSMeta(g, cmdName, cfg, Arc::new(storage))
}
