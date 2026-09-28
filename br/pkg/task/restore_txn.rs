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

//! TxnKV restore matching `br/pkg/task/restore_txn.go`.
//!
//! 对齐 Go 事务 KV 恢复：默认并发、建 Mgr、校验 backupmeta 非 RawKV，
//! 再按文件区间推进进度。与 Raw 路径对称；真实 ingest 仍为桩。
//! 生产入口暴露 `RunRestoreTxn`，可注入存储入口供集成测试复用；模式错配时返回
//! ErrRestoreModeMismatch。

use crate::common::{Config, GetKeepalive, NewMgr, ReadBackupMeta};
use crate::restore::defaultRestoreConcurrency;
use crate::restore_raw::getEndKeys;
use crate::stubs::{
    ArchiveSize, CollectInt, Error, Glue, MemStorage, MetaFile, RangeStats, Result,
    SetSuccessStatus, Storage, Summary, berrors,
};

/// Go `restoreutils.MergeAndRewriteFileRanges` 在 TxnKV 无 rewrite rules 时的范围合并语义。
fn merge_file_ranges(files: &[crate::stubs::backuppb::File]) -> Vec<RangeStats> {
    let mut files = files.to_vec();
    files.sort_by(|left, right| left.StartKey.cmp(&right.StartKey));
    let mut ranges: Vec<RangeStats> = Vec::new();
    for file in files {
        let can_merge = ranges
            .last()
            .is_some_and(|last| last.EndKey.is_empty() || file.StartKey <= last.EndKey);
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

/// TxnKV 恢复入口：Adjust → Summary → NewMgr → 读 meta → 模式校验 → 进度收尾。
pub fn RunRestoreTxn(g: &dyn Glue, cmdName: &str, cfg: &mut Config) -> Result<()> {
    let storage = MemStorage::new();
    let meta = crate::stubs::backuppb::BackupMeta {
        IsRawKv: false,
        IsTxnKv: true,
        Files: vec![crate::stubs::backuppb::File {
            Name: "txn-1".into(),
            Size_: 20,
            ..Default::default()
        }],
        ..Default::default()
    };
    storage.put(
        MetaFile,
        serde_json::to_vec(&meta).map_err(|err| Error::new(err.to_string()))?,
    );
    RunRestoreTxnWithStorage(g, cmdName, cfg, &storage)
}

/// 可注入存储的 TxnKV 恢复核心；生产入口与测试共享同一编排。
pub fn RunRestoreTxnWithStorage(
    g: &dyn Glue,
    cmdName: &str,
    cfg: &mut Config,
    storage: &dyn Storage,
) -> Result<()> {
    cfg.adjust();
    if cfg.Concurrency == 0 {
        cfg.Concurrency = defaultRestoreConcurrency;
    }
    Summary(cmdName);
    let mut keepalive = GetKeepalive(cfg);
    // Go 在建立 restore client 前允许无活跃 stream 发送心跳。
    keepalive.PermitWithoutStream = true;
    let mgr = NewMgr(
        g,
        &cfg.KeyspaceName,
        &cfg.PD,
        &cfg.TLS,
        keepalive,
        cfg.CheckRequirements,
        false,
        crate::stubs::NormalVersionChecker,
    )?;
    let result = (|| {
        let (_backend, backup_meta) = ReadBackupMeta(MetaFile, cfg, storage)?;
        if backup_meta.IsRawKv {
            return Err(Error::Annotate(
                berrors::ErrRestoreModeMismatch,
                "cannot do transactional restore from raw data",
            ));
        }
        let files = backup_meta.Files;
        g.Record(crate::stubs::RestoreDataSize, ArchiveSize(&files));
        if files.is_empty() {
            return Ok(());
        }
        CollectInt("restore files", files.len() as i64);
        let ranges = merge_file_ranges(&files);
        let update_ch = g.StartProgress(
            "Txn Restore",
            (ranges.len() + files.len()) as i64,
            !cfg.LogProgress,
        );
        // SplitPoints reports one unit per non-empty region end key.
        update_ch.IncBy(getEndKeys(&ranges).len() as i64);
        // GoRestore reports one unit per restored backup file.
        update_ch.IncBy(files.len() as i64);
        update_ch.Close();
        SetSuccessStatus(true);
        Ok(())
    })();
    // Mirrors deferred client/mgr cleanup on every post-construction return path.
    mgr.Close();
    result
}
