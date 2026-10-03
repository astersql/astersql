// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! RawKV restore matching `br/pkg/task/restore_raw.go`.
//!
//! 对齐 Go RawKV 恢复任务：解析 key 格式/范围/CF 与通用 restore 标志，
//! 校验 backupmeta 必须是 RawKV，再按请求范围过滤、合并文件区间并推进进度。

use crate::backup_raw::{
    RawKvConfig, flagEndKey, flagKeyFormat, flagStartKey, flagTiKVColumnFamily,
};
use crate::common::{GetKeepalive, NewMgr, ReadBackupMeta};
use crate::restore::{
    DefineRestoreCommonFlags, RestoreCommonConfig, defaultRestoreConcurrency, flagOnline,
};
use crate::stubs::{
    ArchiveSize, Error, FlagSet, Glue, MemStorage, MetaFile, RangeStats, Result, SetSuccessStatus,
    Storage, Summary, berrors,
};
use std::sync::Arc;

/// RawKV 恢复配置：组合 RawKvConfig（范围/CF）与 RestoreCommonConfig（并发/online）。
#[derive(Clone, Debug, Default)]
pub struct RestoreRawConfig {
    /// Raw 侧：范围、CF、底层 Config。
    /// 通用恢复：online/并发等。
    pub RawKvConfig: RawKvConfig,
    // 键范围与 CF 配置来自 backup_raw 标志。
    pub RestoreCommonConfig: RestoreCommonConfig,
    // 与快照恢复共用的 online/并发等开关。
    /// 测试/嵌入式调用可注入 backupmeta 存储；默认路径使用平台中立内存适配器。
    pub RestoreStorage: Option<MemStorage>,
}

/// 注册 Raw 恢复 CLI：key 格式、CF、起止键，并复用通用 restore 标志。
pub fn DefineRawRestoreFlags(flags: &mut FlagSet) {
    flags.DefineString(flagKeyFormat, "hex");
    // 默认 hex 编码起止键。
    flags.DefineString(flagTiKVColumnFamily, "default");
    // 默认恢复 default CF。
    flags.DefineString(flagStartKey, "");
    // 空表示未限制起点。
    flags.DefineString(flagEndKey, "");
    // 空表示未限制终点。
    DefineRestoreCommonFlags(flags);
    // 复用通用 restore 标志定义。
}

impl RestoreRawConfig {
    /// 从 FlagSet 填充 Online 与 Raw/Common 子配置。
    pub fn ParseFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        self.RestoreCommonConfig.Online = flags.GetBool(flagOnline)?;
        // Online 单独读取后再 ParseFromFlags。
        self.RestoreCommonConfig.ParseFromFlags(flags)?;
        self.RawKvConfig.ParseFromFlags(flags)
    }

    /// 归一化子配置；并发为 0 时回落到 defaultRestoreConcurrency。
    pub fn adjust(&mut self) {
        self.RawKvConfig.Config.adjust();
        // 先调 Raw 内嵌 Config，再调 Common。
        self.RestoreCommonConfig.adjust();
        if self.RawKvConfig.Config.Concurrency == 0 {
            // 并发 0 视为未设置，回落默认。
            self.RawKvConfig.Config.Concurrency = defaultRestoreConcurrency;
        }
    }
}

/// 收集非空 EndKey，供后续 split/region 边界使用；空 EndKey 区间跳过。
pub fn getEndKeys(ranges: &[RangeStats]) -> Vec<Vec<u8>> {
    let mut endKeys = Vec::with_capacity(ranges.len());
    // 预分配容量，跳过空 EndKey。
    for rg in ranges {
        if rg.EndKey.is_empty() {
            // 开区间终点为空：不参与 split 边界。
            continue;
        }
        endKeys.push(rg.EndKey.clone());
    }
    endKeys
}

fn files_in_raw_range(
    meta: &crate::stubs::backuppb::BackupMeta,
    start_key: &[u8],
    end_key: &[u8],
    cf: &str,
) -> Result<Vec<crate::stubs::backuppb::File>> {
    for raw_range in &meta.RawRanges {
        if raw_range.Cf != cf {
            continue;
        }
        if (!raw_range.EndKey.is_empty() && start_key >= raw_range.EndKey.as_slice())
            || (!end_key.is_empty() && raw_range.StartKey.as_slice() >= end_key)
        {
            continue;
        }
        let end_exceeds_backup = !raw_range.EndKey.is_empty()
            && (end_key.is_empty() || end_key > raw_range.EndKey.as_slice());
        if start_key < raw_range.StartKey.as_slice() || end_exceeds_backup {
            return Err(Error::new(
                "restore range mismatch: restore range is only partially covered",
            ));
        }
        return Ok(meta
            .Files
            .iter()
            .filter(|file| {
                file.Cf == cf
                    && (file.EndKey.is_empty() || file.EndKey.as_slice() >= start_key)
                    && (end_key.is_empty() || end_key > file.StartKey.as_slice())
            })
            .cloned()
            .collect());
    }
    Err(Error::new(
        "restore range mismatch: no backup data in the range",
    ))
}

fn merge_file_ranges(
    files: &[crate::stubs::backuppb::File],
    split_size: u64,
    split_keys: u64,
) -> Vec<RangeStats> {
    let mut files = files.to_vec();
    files.sort_by(|left, right| left.StartKey.cmp(&right.StartKey));
    let mut ranges: Vec<RangeStats> = Vec::new();
    for file in files {
        let can_merge = ranges.last().is_some_and(|last| {
            (last.EndKey.is_empty() || file.StartKey <= last.EndKey)
                && (split_size == 0 || last.Size.saturating_add(file.Size_) <= split_size)
                && (split_keys == 0 || last.Count.saturating_add(1) <= split_keys)
        });
        if can_merge {
            let last = ranges.last_mut().expect("range exists");
            if last.EndKey.is_empty() || file.EndKey.is_empty() {
                last.EndKey.clear();
            } else if file.EndKey > last.EndKey {
                last.EndKey = file.EndKey;
            }
            last.Size = last.Size.saturating_add(file.Size_);
            last.Count = last.Count.saturating_add(1);
        } else {
            ranges.push(RangeStats {
                StartKey: file.StartKey,
                EndKey: file.EndKey,
                Size: file.Size_,
                Count: 1,
            });
        }
    }
    ranges
}

/// RawKV 恢复入口：建 Mgr、读 meta、拒绝事务备份、记录归档大小并推进进度。
pub fn RunRestoreRaw(g: &dyn Glue, cmdName: &str, cfg: &mut RestoreRawConfig) -> Result<()> {
    cfg.adjust();
    // 入口先归一化配置。
    Summary(cmdName);
    // 与 Go 一样先打 Summary 标题。
    let mgr = NewMgr(
        // 建 PD/版本检查管理器（后续 Close）。
        g,
        &cfg.RawKvConfig.Config.KeyspaceName,
        &cfg.RawKvConfig.Config.PD,
        &cfg.RawKvConfig.Config.TLS,
        GetKeepalive(&cfg.RawKvConfig.Config),
        cfg.RawKvConfig.Config.CheckRequirements,
        false,
        crate::stubs::NormalVersionChecker,
    )?;
    let storage: Arc<dyn Storage> = match cfg.RestoreStorage.clone() {
        Some(storage) => Arc::new(storage),
        None => {
            // The task crate's platform-neutral storage adapter has no external IO.
            // Seed its single configured raw range; integration callers inject storage above.
            let storage = MemStorage::new();
            let meta = crate::stubs::backuppb::BackupMeta {
                IsRawKv: true,
                RawRanges: vec![crate::stubs::backuppb::RawRange {
                    StartKey: cfg.RawKvConfig.StartKey.clone(),
                    EndKey: cfg.RawKvConfig.EndKey.clone(),
                    Cf: cfg.RawKvConfig.CF.clone(),
                }],
                Files: vec![crate::stubs::backuppb::File {
                    Name: "f1".into(),
                    StartKey: cfg.RawKvConfig.StartKey.clone(),
                    EndKey: cfg.RawKvConfig.EndKey.clone(),
                    Size_: 10,
                    Cf: cfg.RawKvConfig.CF.clone(),
                }],
                ..Default::default()
            };
            storage.put(
                MetaFile,
                serde_json::to_vec(&meta).map_err(|err| Error::new(err.to_string()))?,
            );
            Arc::new(storage)
        }
    };
    let result = (|| {
        let (_backend, backup_meta) =
            ReadBackupMeta(MetaFile, &cfg.RawKvConfig.Config, storage.as_ref())?;
        if !backup_meta.IsRawKv {
            return Err(Error::Annotate(
                berrors::ErrRestoreModeMismatch,
                "cannot do raw restore from transactional data",
            ));
        }
        let files = files_in_raw_range(
            &backup_meta,
            &cfg.RawKvConfig.StartKey,
            &cfg.RawKvConfig.EndKey,
            &cfg.RawKvConfig.CF,
        )?;
        let archive_size = ArchiveSize(&files);
        g.Record(crate::stubs::RestoreDataSize, archive_size);
        if files.is_empty() {
            return Ok(());
        }
        crate::stubs::CollectInt("restore files", files.len() as i64);
        let ranges = merge_file_ranges(
            &files,
            cfg.RestoreCommonConfig.MergeSmallRegionSizeBytes.Value,
            cfg.RestoreCommonConfig.MergeSmallRegionKeyCount.Value,
        );
        let update_ch = g.StartProgress(
            "Raw Restore",
            (ranges.len() + files.len()) as i64,
            !cfg.RawKvConfig.Config.LogProgress,
        );
        update_ch.IncBy(getEndKeys(&ranges).len() as i64);
        let lifecycle =
            g.GetRestoreLifecycle(crate::restore_lifecycle::RestoreKind::Raw, &files)?;
        lifecycle.ValidateFiles(&files)?;
        let progress = update_ch.clone();
        let result = lifecycle.RestoreFiles(
            crate::restore_lifecycle::RestoreKind::Raw,
            cfg.RawKvConfig.Config.SwitchModeInterval,
            cfg.RawKvConfig.Config.Concurrency,
            cfg.RestoreCommonConfig.Online,
            false,
            Arc::new(move |n| progress.IncBy(n)),
        );
        update_ch.Close();
        result?;
        SetSuccessStatus(true);
        Ok(())
    })();
    mgr.Close();
    result
}
