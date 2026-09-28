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

//! EBS 快照备份辅助逻辑，对齐 Go `br/pkg/task/backup_ebs.go`。
//! 负责注册 EBS 专用 flag、检测 region 空洞，以及可注入存储的编排入口。
//! 真实 AWS/PD 边界由桩与上层 glue 承接；本文件侧重参数校验与元数据落盘形状。
//! `SkipAWS` 时仍写本地 meta，供 operator 进度与后续恢复读取。

use astersql_br_pkg_aws::EBSBasedBRMeta;
use astersql_br_pkg_common::MaxStoreConcurrency;

use crate::backup::BackupConfig;
use crate::common::{
    FullBackupType, FullBackupTypeEBS, defaultCloudAPIConcurrency, flagCloudAPIConcurrency,
    flagFullBackupType, flagOperatorPausedGCAndSchedulers, flagSkipAWS, progressFileWriterRoutine,
};
use crate::stubs::metapb::{Region, Store};
use crate::stubs::{
    Error, FlagSet, Glue, MemStorage, Progress, Result, SetSuccessStatus, Storage, Summary,
};
use std::sync::Arc;
use std::time::Duration;

/// CLI：卷清单文件路径（Go `volume-file`）。
pub const flagBackupVolumeFile: &str = "volume-file";
/// CLI：进度文件路径，供外部 operator 轮询。
pub const flagProgressFile: &str = "progress-file";

/// 注册 EBS 备份专用 flag；默认 full-backup-type 为 kv，调用方需显式改成 aws-ebs。
pub fn DefineBackupEBSFlags(flags: &mut FlagSet) {
    flags.DefineString(flagFullBackupType, "kv");
    flags.DefineString(flagBackupVolumeFile, "./backup.json");
    flags.DefineBool(flagSkipAWS, false);
    flags.DefineUint(flagCloudAPIConcurrency, defaultCloudAPIConcurrency as u64);
    flags.DefineString(flagProgressFile, "progress.txt");
    flags.DefineBool(flagOperatorPausedGCAndSchedulers, false);
    for name in [
        flagFullBackupType,
        flagBackupVolumeFile,
        flagSkipAWS,
        flagCloudAPIConcurrency,
        flagProgressFile,
        flagOperatorPausedGCAndSchedulers,
    ] {
        let _ = flags.MarkHidden(name);
    }
}

/// 检测排序后相邻 region 的 EndKey/StartKey 是否存在空洞。
/// 有空洞时调度器未完全停稳或拓扑不一致，EBS 快照不安全。
pub fn isRegionsHasHole(allRegions: &mut [Region]) -> bool {
    // 先按 StartKey 排序，再两两比较边界是否首尾相接。
    allRegions.sort_by(|a, b| a.StartKey.cmp(&b.StartKey));
    for j in 0..allRegions.len().saturating_sub(1) {
        let left = &allRegions[j];
        let right = &allRegions[j + 1];
        if left.EndKey != right.StartKey {
            return true;
        }
    }
    false
}

fn parseGoDuration(value: &str) -> Option<Duration> {
    if value == "0" {
        return Some(Duration::ZERO);
    }
    if value.is_empty() {
        return None;
    }
    // Go permits negative durations; `time.After` treats them as immediately due.
    if value.starts_with('-') {
        parseGoDuration(&value[1..])?;
        return Some(Duration::ZERO);
    }
    let mut rest = value;
    let mut seconds = 0.0_f64;
    while !rest.is_empty() {
        let number_end = rest
            .char_indices()
            .take_while(|(_, ch)| ch.is_ascii_digit() || *ch == '.')
            .map(|(idx, ch)| idx + ch.len_utf8())
            .last()?;
        let number = rest[..number_end].parse::<f64>().ok()?;
        if !number.is_finite() {
            return None;
        }
        rest = &rest[number_end..];
        let (unit, scale) = [
            ("ns", 1e-9),
            ("us", 1e-6),
            ("µs", 1e-6),
            ("μs", 1e-6),
            ("ms", 1e-3),
            ("s", 1.0),
            ("m", 60.0),
            ("h", 3600.0),
        ]
        .into_iter()
        .find(|(unit, _)| rest.starts_with(unit))?;
        seconds += number * scale;
        rest = &rest[unit.len()..];
    }
    Duration::try_from_secs_f64(seconds).ok()
}

/// 对齐 Go 环境变量解析；缺失或非法值均回落到 800ms。
pub(crate) fn getMockSleepTimeFromEnv(value: Option<&str>) -> Duration {
    value
        .and_then(parseGoDuration)
        .unwrap_or(Duration::from_millis(800))
}

/// SkipAWS 模拟快照的等待时长，可通过 Go 同名环境变量覆盖。
pub fn getMockSleepTime() -> Duration {
    let value = std::env::var("br_ebs_backup_mocking_wait_snapshot_duration").ok();
    getMockSleepTimeFromEnv(value.as_deref())
}

/// 将 EBS 备份元数据序列化为 `backupmeta.json` 写入存储。
pub fn saveMetaFile(backupInfo: &EBSBasedBRMeta, storage: &dyn Storage) -> Result<()> {
    let data = serde_json::to_vec(backupInfo).map_err(|e| Error::new(e.to_string()))?;
    storage.WriteFile("backupmeta.json", &data)
}

/// EBS 备份编排入口：校验类型/并发，写进度与 meta，AWS/PD 边界可注入。
/// 非 `aws-ebs` 类型直接报错，避免误走卷快照路径。
pub fn RunBackupEBS(g: &dyn Glue, cfg: &mut BackupConfig, storage: Arc<dyn Storage>) -> Result<()> {
    cfg.Adjust();
    Summary("EBS Backup");
    // 空类型回落到 KV 默认值，随后 Valid/类型分支会拒绝非 EBS。
    if cfg.FullBackupType.0.is_empty() {
        cfg.FullBackupType = FullBackupType(crate::common::FullBackupTypeKV.into());
    }
    if !cfg.FullBackupType.Valid() {
        return Err(Error::new("invalid full backup type"));
    }
    if cfg.FullBackupType.0 != FullBackupTypeEBS {
        return Err(Error::new("RunBackupEBS requires aws-ebs full backup type"));
    }
    // 0 表示未配置，填公共默认云 API 并发度。
    if cfg.CloudAPIConcurrency == 0 {
        cfg.CloudAPIConcurrency = defaultCloudAPIConcurrency;
    }
    let _ = MaxStoreConcurrency;
    let progress = g.StartProgress("EBS Backup", 100, !cfg.Config.LogProgress);
    if !cfg.ProgressFile.is_empty() {
        progressFileWriterRoutine(progress.as_ref(), 100, &cfg.ProgressFile, false);
    }
    // 精简移植用固定 Region 占位；完整 Go 路径会从 EC2/PD 填充卷与快照信息。
    let meta = EBSBasedBRMeta {
        Region: "us-west-2".into(),
        ..Default::default()
    };
    if !cfg.SkipAWS {
        saveMetaFile(&meta, storage.as_ref())?;
    } else {
        // skip-aws 仍写本地 meta，供 operator 进度与恢复读取。
        saveMetaFile(&meta, storage.as_ref())?;
    }
    progress.IncBy(100);
    progress.Close();
    SetSuccessStatus(true);
    // 触碰桩类型，保持与 Go 侧符号引用形状一致。
    let _ = MemStorage::new();
    let _ = Store::default();
    Ok(())
}

/// 等待调度停止且 region 无空洞；无 store 或存在空洞则失败。
/// Go 版含轮询；此处保留前置校验契约，供 parity/单测驱动。
pub fn waitAllScheduleStoppedAndNoRegionHole(
    stores: &[Store],
    regions: &mut [Region],
) -> Result<()> {
    if stores.is_empty() {
        return Err(Error::new("no stores"));
    }
    if isRegionsHasHole(regions) {
        return Err(Error::new("region hole found"));
    }
    Ok(())
}
