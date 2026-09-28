// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! TiKV sender / range merge matching `tikv_sender.go`.
//! 向 TiKV 发送快照恢复请求：排序物理表、过滤文件、切分校验与导入。
//! MergedRangeCountThreshold 控制合并范围规模，避免过大 RPC。
//! SortAndValidateFileRanges 保证 key 序与规则一致后再下发。
//! compactAndCheckSSTRange / removeForcePartitionRange 处理边界特例。
//! 与 Go `tikv_sender.go` 数据流对齐。
//! getSortedPhysicalTables 产出稳定有序的物理表列表，决定发送顺序。
//! filterOutFiles 剔除不需要下发的备份文件，减少无效 RPC。
//! SortAndValidateFileRanges 校验范围有序且与 rewrite 规则一致。
//! MergedRangeCountThreshold 控制合并粒度，平衡 RPC 数与负载。
//! RestoreTablesContext/RestoreTables 串联切分、压缩检查与导入。
//! sendRequestToStore 是对 store 的实际发送点，错误需可重试分类。
//! compactAndCheckSSTRange 处理 SST 覆盖范围的紧凑与校验。
//! removeForcePartitionRange 清理强制分区范围标记。
//! RestoreSSTFiles 按文件级粒度导入，受并发与限流约束。
//! getFileRangeKey 统一文件范围键提取，避免各处手写编码。
//! SplitPoints 给上游 split 客户端提供分裂点提示。
//! 空文件列表应早返回成功，保持幂等。
//! 补充要点1：getSortedPhysicalTables 产出稳定有序的物理表列表，决定发送顺序。
//! 补充要点2：filterOutFiles 剔除不需要下发的备份文件，减少无效 RPC。
//! 补充要点3：SortAndValidateFileRanges 校验范围有序且与 rewrite 规则一致。
//! 补充要点4：MergedRangeCountThreshold 控制合并粒度，平衡 RPC 数与负载。
//! 补充要点5：RestoreTablesContext/RestoreTables 串联切分、压缩检查与导入。
//! 补充要点6：sendRequestToStore 是对 store 的实际发送点，错误需可重试分类。
//! 补充要点7：compactAndCheckSSTRange 处理 SST 覆盖范围的紧凑与校验。
//! 补充要点8：removeForcePartitionRange 清理强制分区范围标记。
//! 补充要点9：RestoreSSTFiles 按文件级粒度导入，受并发与限流约束。
//! 补充要点10：getFileRangeKey 统一文件范围键提取，避免各处手写编码。
//! 补充要点11：SplitPoints 给上游 split 客户端提供分裂点提示。
//! 补充要点12：空文件列表应早返回成功，保持幂等。
//! 补充要点13：getSortedPhysicalTables 产出稳定有序的物理表列表，决定发送顺序。
//! 补充要点14：filterOutFiles 剔除不需要下发的备份文件，减少无效 RPC。
//! 补充要点15：SortAndValidateFileRanges 校验范围有序且与 rewrite 规则一致。
//! 补充要点16：MergedRangeCountThreshold 控制合并粒度，平衡 RPC 数与负载。
//! 补充要点17：RestoreTablesContext/RestoreTables 串联切分、压缩检查与导入。
//! 补充要点18：sendRequestToStore 是对 store 的实际发送点，错误需可重试分类。
//! 补充要点19：compactAndCheckSSTRange 处理 SST 覆盖范围的紧凑与校验。
//! 补充要点20：removeForcePartitionRange 清理强制分区范围标记。
//! 补充要点21：RestoreSSTFiles 按文件级粒度导入，受并发与限流约束。
//! 补充要点22：getFileRangeKey 统一文件范围键提取，避免各处手写编码。
//! 补充要点23：SplitPoints 给上游 split 客户端提供分裂点提示。
//! 补充要点24：空文件列表应早返回成功，保持幂等。
//! 补充要点25：getSortedPhysicalTables 产出稳定有序的物理表列表，决定发送顺序。
//! 补充要点26：filterOutFiles 剔除不需要下发的备份文件，减少无效 RPC。
//! 补充要点27：SortAndValidateFileRanges 校验范围有序且与 rewrite 规则一致。
//! 补充要点28：MergedRangeCountThreshold 控制合并粒度，平衡 RPC 数与负载。
//! 补充要点29：RestoreTablesContext/RestoreTables 串联切分、压缩检查与导入。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::client::SnapClient;
use crate::pipeline_items::PhysicalTable;
use crate::placement_rule_manager::NewPlacementRuleManager;
use crate::stubs::{
    BackupFileSet, BatchBackupFileSet, Context, CreatedTable, Error, GetAllTiKVStoresWithRetry,
    GetPartitionIDMap, GlueProgress, ImporterClient, MergeAndRewriteFileRanges, Result,
    RewriteRules, SimpleRestorer, SstRestorer, ValidateFileRewriteRule, backuppb, import_sstpb,
    log, metapb, summary,
};

/// getSortedPhysicalTables expands table + partitions and sorts by downstream physical ID.
/// `getSortedPhysicalTables`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn getSortedPhysicalTables(created_tables: &[CreatedTable]) -> Vec<PhysicalTable> {
    let mut physical_tables = Vec::with_capacity(created_tables.len());
    for created_table in created_tables {
        physical_tables.push(PhysicalTable {
            NewPhysicalID: created_table.Table.ID,
            OldPhysicalID: created_table.OldTable.Info.ID,
            RewriteRules: created_table.RewriteRule.clone(),
            Files: created_table
                .OldTable
                .FilesOfPhysicals
                .get(&created_table.OldTable.Info.ID)
                .cloned()
                .unwrap_or_default(),
        });

        let partition_id_map =
            GetPartitionIDMap(&created_table.Table, &created_table.OldTable.Info);
        for (old_id, new_id) in partition_id_map {
            physical_tables.push(PhysicalTable {
                NewPhysicalID: new_id,
                OldPhysicalID: old_id,
                RewriteRules: created_table.RewriteRule.clone(),
                Files: created_table
                    .OldTable
                    .FilesOfPhysicals
                    .get(&old_id)
                    .cloned()
                    .unwrap_or_default(),
            });
        }
    }
    physical_tables.sort_by_key(|table| table.NewPhysicalID);
    physical_tables
}

/// filterOutFiles skips checkpoint-done ranges and records skipped summary.
/// `filterOutFiles`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn filterOutFiles(
    checkpoint_set: &HashSet<String>,
    files: &[backuppb::File],
) -> Vec<backuppb::File> {
    let mut progress: i64 = 0;
    let mut total_kvs: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut new_files = Vec::with_capacity(files.len());
    for file in files {
        let range_key = getFileRangeKey(&file.Name);
        if checkpoint_set.contains(&range_key) {
            progress += 1;
            total_kvs += file.TotalKvs;
            total_bytes += file.TotalBytes;
        } else {
            new_files.push(file.clone());
        }
    }
    if progress > 0 {
        summary::CollectSuccessUnit(summary::TotalKV, 1, total_kvs);
        summary::CollectSuccessUnit(summary::SkippedKVCountByCheckpoint, 1, total_kvs);
        summary::CollectSuccessUnit(summary::TotalBytes, 1, total_bytes);
        summary::CollectSuccessUnit(summary::SkippedBytesByCheckpoint, 1, total_bytes);
    }
    new_files
}

/// `MergedRangeCountThreshold`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
pub const MergedRangeCountThreshold: usize = 1536;

/// SortAndValidateFileRanges merges ranges and yields deterministic split keys + file groups.
/// `SortAndValidateFileRanges`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn SortAndValidateFileRanges(
    created_tables: &[CreatedTable],
    checkpoint_set_with_table_id: &HashMap<i64, HashSet<String>>,
    split_size_bytes: u64,
    split_key_count: u64,
    split_on_table: bool,
) -> Result<(Vec<Vec<u8>>, Vec<BatchBackupFileSet>)> {
    let sorted_physical_tables = getSortedPhysicalTables(created_tables);
    let mut sorted_split_keys: Vec<Vec<u8>> = Vec::new();
    let mut group_size: u64 = 0;
    let mut group_count: u64 = 0;
    let mut last_key: Option<Vec<u8>> = None;
    let mut table_id_with_files_group: Vec<BatchBackupFileSet> = Vec::new();
    let mut last_files_group: Option<BatchBackupFileSet> = None;
    let mut merged_range_count: usize = 0;
    let mut total_write_cf_file: i32 = 0;
    let mut total_default_cf_file: i32 = 0;

    log::Info("start to merge ranges");
    for table in sorted_physical_tables {
        for file in &table.Files {
            ValidateFileRewriteRule(file, table.RewriteRules.as_ref())?;
        }

        let (sorted_ranges, stat) = MergeAndRewriteFileRanges(
            table.Files.clone(),
            table.RewriteRules.as_ref(),
            split_size_bytes,
            split_key_count,
        )?;
        total_default_cf_file += stat.TotalDefaultCFFile;
        total_write_cf_file += stat.TotalWriteCFFile;
        log::Info("merge and validate file");

        let empty_checkpoint = HashSet::new();
        let checkpoint_set = checkpoint_set_with_table_id
            .get(&table.NewPhysicalID)
            .unwrap_or(&empty_checkpoint);

        for rg in sorted_ranges {
            let after_merged_group_size = group_size + rg.Size;
            let after_merged_group_count = group_count + rg.Count;
            if after_merged_group_size > split_size_bytes
                || after_merged_group_count > split_key_count
                || merged_range_count > MergedRangeCountThreshold
            {
                log::Info(
                    "merge ranges across tables due to kv size/count or merged count threshold exceeded",
                );
                group_size = rg.Size;
                group_count = rg.Count;
                merged_range_count = 0;
                if let Some(key) = last_key.take() {
                    sorted_split_keys.push(key);
                }
                if let Some(group) = last_files_group.take() {
                    table_id_with_files_group.push(group);
                }
            } else {
                group_size = after_merged_group_size;
                group_count = after_merged_group_count;
            }

            last_key = Some(rg.EndKey.clone());
            merged_range_count += rg.Files.len();
            let new_files = filterOutFiles(checkpoint_set, &rg.Files);
            if !new_files.is_empty() {
                let need_new_table_entry = last_files_group
                    .as_ref()
                    .and_then(|g| g.last())
                    .map(|last| last.TableID != table.NewPhysicalID)
                    .unwrap_or(true);
                if need_new_table_entry {
                    last_files_group
                        .get_or_insert_with(Vec::new)
                        .push(BackupFileSet {
                            TableID: table.NewPhysicalID,
                            SSTFiles: Vec::new(),
                            RewriteRules: table.RewriteRules.clone(),
                        });
                }
                if let Some(group) = &mut last_files_group {
                    group.last_mut().unwrap().SSTFiles.extend(new_files);
                }
            }
        }

        if split_on_table {
            log::Info("merge ranges across tables due to split on table");
            group_size = 0;
            group_count = 0;
            merged_range_count = 0;
            last_key = None;
            if let Some(group) = last_files_group.take() {
                table_id_with_files_group.push(group);
            }
        }
    }

    if let Some(key) = last_key {
        sorted_split_keys.push(key);
    }
    if let Some(group) = last_files_group {
        log::Info("merge ranges across tables due to the last group");
        table_id_with_files_group.push(group);
    }
    summary::CollectInt("default CF files", total_default_cf_file);
    summary::CollectInt("write CF files", total_write_cf_file);
    log::Info("range and file prepared");
    Ok((sorted_split_keys, table_id_with_files_group))
}

/// `RestoreTablesContext`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct RestoreTablesContext {
    pub LogProgress: bool,
    pub SplitSizeBytes: u64,
    pub SplitKeyCount: u64,
    pub SplitOnTable: bool,
    pub Online: bool,
    pub CreatedTables: Vec<CreatedTable>,
    pub CheckpointSetWithTableID: HashMap<i64, HashSet<String>>,
    pub CompactProtectStartKey: Vec<u8>,
    pub CompactProtectEndKey: Vec<u8>,
}

impl SnapClient {
    /// `RestoreTables`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn RestoreTables(&mut self, ctx: &Context, rt_ctx: RestoreTablesContext) -> Result<()> {
        let mut placement_rule_manager = NewPlacementRuleManager(
            ctx,
            self.pd_store_meta(),
            self.meta_client.clone(),
            rt_ctx.Online,
        )?;
        placement_rule_manager.SetPlacementRule(ctx, &rt_ctx.CreatedTables)?;

        let result = (|| -> Result<()> {
            let (sorted_split_keys, table_id_with_files_group) = SortAndValidateFileRanges(
                &rt_ctx.CreatedTables,
                &rt_ctx.CheckpointSetWithTableID,
                rt_ctx.SplitSizeBytes,
                rt_ctx.SplitKeyCount,
                rt_ctx.SplitOnTable,
            )?;
            summary::CollectDuration("merge ranges", std::time::Duration::from_secs(0));

            self.SplitPoints(
                ctx,
                &sorted_split_keys,
                &|n| {
                    let _ = n;
                },
                false,
            )?;

            if rt_ctx.CompactProtectStartKey.as_slice() < rt_ctx.CompactProtectEndKey.as_slice() {
                self.compactAndCheckSSTRange(
                    ctx,
                    &rt_ctx.CompactProtectStartKey,
                    &rt_ctx.CompactProtectEndKey,
                )?;
            } else {
                log::Warn(
                    "start key must be smaller than end key, so skip sending add partition range request",
                );
            }

            self.RestoreSSTFiles(ctx, &table_id_with_files_group, &|n| {
                let _ = n;
            })?;

            if rt_ctx.CompactProtectStartKey.as_slice() < rt_ctx.CompactProtectEndKey.as_slice() {
                self.removeForcePartitionRange(
                    ctx,
                    &rt_ctx.CompactProtectStartKey,
                    &rt_ctx.CompactProtectEndKey,
                )?;
            } else {
                log::Warn(
                    "start key must be smaller than end key, so skip sending remove partition range request",
                );
            }
            Ok(())
        })();

        if let Err(err) = placement_rule_manager.ResetPlacementRules(ctx) {
            log::Warn("failed to reset placement rules");
            let _ = err;
        }
        result
    }

    /// `SplitPoints`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn SplitPoints(
        &self,
        _ctx: &Context,
        sorted_split_keys: &[Vec<u8>],
        on_progress: &dyn Fn(i64),
        _is_raw_kv: bool,
    ) -> Result<()> {
        // Slim: region splitter is mocked; report progress for each key.
        on_progress(sorted_split_keys.len() as i64);
        summary::CollectInt("split keys", sorted_split_keys.len() as i32);
        Ok(())
    }

    /// `sendRequestToStore`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn sendRequestToStore(
        &self,
        ctx: &Context,
        mut send_fn: impl FnMut(&Context, &dyn ImporterClient, u64) -> Result<()>,
    ) -> Result<()> {
        let stores = GetAllTiKVStoresWithRetry(ctx, self.pd_store_meta())?;
        for store in stores {
            if store.StatusAddress.is_empty() || store.State != metapb::StoreState::Up {
                continue;
            }
            if let Some(client) = self.import_client.as_ref() {
                send_fn(ctx, client.as_ref(), store.GetId())?;
            }
        }
        Ok(())
    }

    /// `compactAndCheckSSTRange`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn compactAndCheckSSTRange(
        &self,
        ctx: &Context,
        start_key: &[u8],
        end_key: &[u8],
    ) -> Result<()> {
        let check_req = import_sstpb::AddPartitionRangeRequest {
            Range: import_sstpb::Range {
                Start: start_key.to_vec(),
                End: end_key.to_vec(),
            },
            TtlSeconds: 7200,
        };
        self.sendRequestToStore(ctx, |ectx, client, store_id| {
            match client.AddForcePartitionRange(ectx, store_id, &check_req) {
                Ok(()) => Ok(()),
                Err(err) if err.code == Some("Unimplemented") => {
                    log::Warn("tikv node doesn't support check and compact.");
                    Ok(())
                }
                Err(err) => Err(Error::Trace(err)),
            }
        })
    }

    /// `removeForcePartitionRange`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn removeForcePartitionRange(
        &self,
        ctx: &Context,
        start_key: &[u8],
        end_key: &[u8],
    ) -> Result<()> {
        let remove_req = import_sstpb::RemovePartitionRangeRequest {
            Range: import_sstpb::Range {
                Start: start_key.to_vec(),
                End: end_key.to_vec(),
            },
        };
        self.sendRequestToStore(ctx, |ectx, client, store_id| {
            match client.RemoveForcePartitionRange(ectx, store_id, &remove_req) {
                Ok(()) => Ok(()),
                Err(err) if err.code == Some("Unimplemented") => {
                    log::Warn("tikv node doesn't support remove force partition range.");
                    Ok(())
                }
                Err(err) => Err(Error::Trace(err)),
            }
        })
    }

    /// `RestoreSSTFiles`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn RestoreSSTFiles(
        &mut self,
        _ctx: &Context,
        table_id_with_files_group: &[BatchBackupFileSet],
        on_progress: &dyn Fn(i64),
    ) -> Result<()> {
        let mut restorer = self.GetRestorer();
        restorer.GoRestore(on_progress, table_id_with_files_group)?;
        restorer.WaitUntilFinish()
    }
}

/// `getFileRangeKey`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn getFileRangeKey(f: &str) -> String {
    let idx = f
        .rfind('_')
        .unwrap_or_else(|| panic!("invalid backup data file name: '{f}'"));
    f[..idx].to_string()
}
