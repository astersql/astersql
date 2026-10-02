// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// Delete Range（删除范围）模块：将 DDL 作业（如删表、删索引、删分区等）
// 转换为一组按键范围（key range）删除的任务。
//
// 背景知识：数据库中删除大量数据（例如 DROP TABLE）如果逐行删除会非常慢，
// 因此采用"范围删除"的方式——记录一段起止键 `[start_key, end_key)`，
// 交由后台 GC（垃圾回收）流程异步清理该范围内的所有 KV 数据。
// 该机制源自 TiDB 的 delete-range 设计：DDL 提交后先把待删范围写入队列，
// 待 GC safepoint（安全点，MVCC 中所有旧版本都可回收的时间戳）越过后再真正物理删除。

use crate::backfilling::Key;
use crate::delete_range_util::ElementIdAllocator;

/// 删除范围模拟器每个任务单批删除的最大 KV 条目数。
pub const DEL_RANGE_EMULATOR_TASK_DEL_BATCH: usize = 65_536;
/// 批量插入删除范围记录时的单批大小。
pub const BATCH_INSERT_DELETE_RANGE_SIZE: usize = 256;
/// 临时索引 ID 前缀标记（最高有效位之一置 1）。
/// 添加索引（AddIndex）过程中会先写入带此前缀的"临时索引"，
/// 回填（backfill）完成后再切换为正式索引；回滚或完成后需按该前缀清理临时数据。
pub const TEMPORARY_INDEX_PREFIX: i64 = astersql_tablecodec::TempIndexPrefix;

/// 会产生删除范围任务的 DDL 动作类型。
/// 每种动作对应不同的数据清理策略（整表范围或索引范围）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeleteRangeAction {
    /// 删除整个库（schema），需要清理库下所有物理表的数据。
    DropSchema,
    /// 删除表。
    DropTable,
    /// 清空表（TRUNCATE 会分配新表 ID，旧表数据整体清理）。
    TruncateTable,
    /// 删除分区。
    DropPartition,
    /// 重组分区（拆分/合并分区，旧分区数据需清理）。
    ReorganizePartition,
    /// 取消分区（分区表转普通表）。
    RemovePartitioning,
    /// 修改表的分区方式。
    AlterTablePartitioning,
    /// 清空分区。
    TruncatePartition,
    /// 添加索引（失败回滚或临时索引数据需清理）。
    AddIndex,
    /// 添加主键（内部实现与添加索引类似）。
    AddPrimaryKey,
    /// 删除索引。
    DropIndex,
    /// 删除主键。
    DropPrimaryKey,
    /// 删除列（连带清理该列相关索引数据）。
    DropColumn,
    /// 修改列类型（可能重建索引，旧索引数据需清理）。
    ModifyColumn,
    /// 多 schema 变更（一条 DDL 包含多个子作业）。
    MultiSchemaChange,
    /// 其他动作，不产生删除范围任务。
    Other,
}

/// 索引参数：描述一条待清理索引的关键信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexArgument {
    /// 索引 ID。
    pub index_id: i64,
    /// 是否为全局索引（分区表上跨所有分区的索引，键以表 ID 而非分区 ID 编码）。
    pub global: bool,
    /// 是否为列存（columnar）索引；列存索引不走 KV 范围删除。
    pub columnar: bool,
    /// 索引所属的表 ID。
    pub table_id: i64,
}

/// 删除范围作业：从 DDL 作业中抽取的、与数据清理相关的字段集合。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteRangeJob {
    /// DDL 作业 ID。
    pub id: i64,
    /// 目标表 ID。
    pub table_id: i64,
    /// DDL 动作类型，决定清理策略。
    pub action: DeleteRangeAction,
    /// 回滚是否已完成（AddIndex 回滚完成时需同时清理正式索引数据）。
    pub rollback_done: bool,
    /// 作业是否已取消；取消的作业不产生任何清理任务。
    pub cancelled: bool,
    /// 旧的物理表 ID 列表（如被 drop/truncate 的表或分区的物理 ID）。
    pub old_physical_table_ids: Vec<i64>,
    /// 分区 ID 列表；为空表示非分区表。
    pub partition_ids: Vec<i64>,
    /// 涉及的索引参数列表。
    pub index_arguments: Vec<IndexArgument>,
    /// 涉及的索引 ID 列表（DropColumn/ModifyColumn 使用）。
    pub index_ids: Vec<i64>,
    /// 旧的全局索引列表（分区重组类操作需要单独清理）。
    pub old_global_indexes: Vec<IndexArgument>,
    /// 子作业列表（MultiSchemaChange 时递归处理）。
    pub subjobs: Vec<DeleteRangeJob>,
}

/// 删除范围任务：一段待清理的键区间 `[start_key, end_key)`。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeleteRangeTask {
    /// 产生该任务的 DDL 作业 ID。
    pub job_id: i64,
    /// 元素 ID：同一作业内区分不同表/索引范围的唯一编号。
    pub element_id: i64,
    /// 范围起始键（含）。
    pub start_key: Key,
    /// 范围结束键（不含）。
    pub end_key: Key,
}

/// 将一个 DDL 作业转换为删除范围任务列表（使用新的元素 ID 分配器）。
pub fn add_delete_range_job(job: &DeleteRangeJob) -> Vec<DeleteRangeTask> {
    let mut allocator = ElementIdAllocator::default();
    insert_job_into_delete_range(job, &mut allocator)
}

/// 根据 DDL 作业的动作类型，生成对应的删除范围任务。
/// 核心分发逻辑：不同动作决定清理"整表范围"还是"索引范围"，
/// 以及物理 ID 取自分区列表还是表本身。
pub fn insert_job_into_delete_range(
    job: &DeleteRangeJob,
    allocator: &mut ElementIdAllocator,
) -> Vec<DeleteRangeTask> {
    // 已取消的作业没有实际写入数据，无需清理。
    if job.cancelled {
        return Vec::new();
    }
    match job.action {
        // 删库：清理库下所有旧物理表的整表范围。
        DeleteRangeAction::DropSchema => table_tasks(job, &job.old_physical_table_ids, allocator),
        // 删表/清空表：除旧物理 ID（分区）外，还要清理表自身的范围。
        DeleteRangeAction::DropTable | DeleteRangeAction::TruncateTable => {
            let mut ids = job.old_physical_table_ids.clone();
            ids.push(job.table_id);
            table_tasks(job, &ids, allocator)
        }
        // 删/清空分区：仅清理被移除分区的整表范围。
        DeleteRangeAction::DropPartition | DeleteRangeAction::TruncatePartition => {
            table_tasks(job, &job.old_physical_table_ids, allocator)
        }
        // 分区重组类操作：先清理旧的全局索引，再清理旧分区数据。
        DeleteRangeAction::ReorganizePartition
        | DeleteRangeAction::RemovePartitioning
        | DeleteRangeAction::AlterTablePartitioning => {
            let mut tasks = Vec::new();
            for index in &job.old_global_indexes {
                tasks.extend(index_tasks(
                    job,
                    index.table_id,
                    &[index.index_id],
                    allocator,
                ));
            }
            tasks.extend(table_tasks(job, &job.old_physical_table_ids, allocator));
            tasks
        }
        // 添加索引/主键：清理回填过程中写入的临时索引数据；
        // 若回滚已完成，还需清理正式索引数据。
        DeleteRangeAction::AddIndex | DeleteRangeAction::AddPrimaryKey => {
            // 分区表按各分区的物理 ID 清理，非分区表按表 ID 清理。
            let physical_ids = if job.partition_ids.is_empty() {
                vec![job.table_id]
            } else {
                job.partition_ids.clone()
            };
            let mut tasks = Vec::new();
            for index in &job.index_arguments {
                // 临时索引 ID = 前缀标记 | 原索引 ID。
                let mut index_ids = vec![TEMPORARY_INDEX_PREFIX | index.index_id];
                if job.rollback_done {
                    index_ids.insert(0, index.index_id);
                }
                if index.global {
                    // 全局索引的键以表 ID 编码，只需清理一次。
                    tasks.extend(index_tasks(job, job.table_id, &index_ids, allocator));
                } else {
                    // 普通索引按每个物理表（分区）分别清理。
                    for id in &physical_ids {
                        tasks.extend(index_tasks(job, *id, &index_ids, allocator));
                    }
                }
            }
            tasks
        }
        // 删除索引/主键：按物理表逐个清理该索引的键范围。
        DeleteRangeAction::DropIndex | DeleteRangeAction::DropPrimaryKey => {
            let Some(index) = job.index_arguments.first() else {
                return Vec::new();
            };
            // 列存索引数据不在行存 KV 中，无需范围删除。
            if index.columnar {
                return Vec::new();
            }
            let physical_ids = if job.partition_ids.is_empty() {
                vec![job.table_id]
            } else {
                job.partition_ids.clone()
            };
            physical_ids
                .into_iter()
                .flat_map(|id| index_tasks(job, id, &[index.index_id], allocator))
                .collect()
        }
        // 删列/改列：清理受影响索引（index_ids）在各物理表上的数据。
        DeleteRangeAction::DropColumn | DeleteRangeAction::ModifyColumn => {
            let physical_ids = if job.partition_ids.is_empty() {
                vec![job.table_id]
            } else {
                job.partition_ids.clone()
            };
            physical_ids
                .into_iter()
                .flat_map(|id| index_tasks(job, id, &job.index_ids, allocator))
                .collect()
        }
        // 多 schema 变更：对每个子作业递归生成任务。
        DeleteRangeAction::MultiSchemaChange => job
            .subjobs
            .iter()
            .flat_map(|subjob| {
                // Go converts each sub-job to a proxy job which keeps the
                // parent job identity and target table.
                let mut proxy_job = subjob.clone();
                proxy_job.id = job.id;
                proxy_job.table_id = job.table_id;
                insert_job_into_delete_range(&proxy_job, allocator)
            })
            .collect(),
        DeleteRangeAction::Other => Vec::new(),
    }
}

/// 为一组物理表 ID 生成"整表范围"的删除任务。
/// 范围为 `[t{id}, t{id+1})`，即覆盖该表编码前缀下的所有键。
fn table_tasks(
    job: &DeleteRangeJob,
    table_ids: &[i64],
    allocator: &mut ElementIdAllocator,
) -> Vec<DeleteRangeTask> {
    table_ids
        .iter()
        .map(|table_id| DeleteRangeTask {
            job_id: job.id,
            element_id: allocator.alloc_for_physical_id(*table_id),
            start_key: encode_table_prefix(*table_id),
            end_key: encode_table_prefix(table_id.wrapping_add(1)),
        })
        .collect()
}

/// 为某个物理表上的一组索引 ID 生成"索引范围"的删除任务。
/// 范围为 `[t{tid}_i{iid}, t{tid}_i{iid+1})`，仅覆盖该索引前缀下的键。
fn index_tasks(
    job: &DeleteRangeJob,
    table_id: i64,
    index_ids: &[i64],
    allocator: &mut ElementIdAllocator,
) -> Vec<DeleteRangeTask> {
    index_ids
        .iter()
        .map(|index_id| DeleteRangeTask {
            job_id: job.id,
            element_id: allocator.alloc_for_index_id(table_id, *index_id),
            start_key: encode_table_index_prefix(table_id, *index_id),
            end_key: encode_table_index_prefix(table_id, index_id.wrapping_add(1)),
        })
        .collect()
}

/// 编码表数据的键前缀：`t` + Go 有符号整数排序编码。
fn encode_table_prefix(table_id: i64) -> Key {
    astersql_tablecodec::EncodeTablePrefix(table_id).0
}
/// 编码索引数据的键前缀：表前缀 + `_i` + Go 有符号整数排序编码。
fn encode_table_index_prefix(table_id: i64, index_id: i64) -> Key {
    astersql_tablecodec::EncodeTableIndexPrefix(table_id, index_id).0
}

/// 删除范围管理器：维护待执行与已完成的删除任务队列，
/// 模拟 TiDB 中 delete-range 后台工作流（入队 -> 执行 -> 交由 GC 记录）。
#[derive(Clone, Debug, Default)]
pub struct DeleteRangeManager {
    /// 待执行的删除任务。
    pub pending: Vec<DeleteRangeTask>,
    /// 已完成删除、等待 GC 最终回收记录的任务。
    pub completed: Vec<DeleteRangeTask>,
    /// 管理器是否已启动；未启动时不执行任何删除。
    pub started: bool,
}

impl DeleteRangeManager {
    /// 启动管理器，允许执行删除工作。
    pub fn start(&mut self) {
        self.started = true;
    }
    /// 停止并清空待执行队列（已完成记录保留）。
    pub fn clear(&mut self) {
        self.started = false;
        self.pending.clear();
    }
    /// 将一个 DDL 作业转换为删除任务并加入待执行队列。
    pub fn add_job(&mut self, job: &DeleteRangeJob) {
        self.pending.extend(add_delete_range_job(job));
    }
    /// 执行一轮删除工作：对每个待执行任务调用 `delete` 回调，
    /// 返回 true 表示该范围删除完成（移入 completed），否则保留待下轮重试。
    pub fn do_delete_range_work(
        &mut self,
        mut delete: impl FnMut(&DeleteRangeTask, usize) -> bool,
    ) {
        if !self.started {
            return;
        }
        // 逐个执行：成功的任务归档到 completed，失败的保留在 pending 重试。
        let mut retained = Vec::new();
        for task in self.pending.drain(..) {
            if delete(&task, DEL_RANGE_EMULATOR_TASK_DEL_BATCH) {
                self.completed.push(task);
            } else {
                retained.push(task);
            }
        }
        self.pending = retained;
    }
    /// 从 GC 删除范围记录中移除指定作业的所有已完成任务
    /// （对应作业被彻底回收后清理其残留记录）。
    pub fn remove_from_gc_delete_range(&mut self, job_id: i64) {
        self.completed.retain(|task| task.job_id != job_id);
    }
}

/// Independent, autocommit system session used by Go's delRange manager.
/// A successful batch survives rollback of the enclosing DDL worker.
pub trait DeleteRangeExecutor {
    fn current_version(&mut self) -> Result<u64, String>;
    fn execute(&mut self, sql: &str) -> Result<(), String>;
}

/// Go JobNeedGC, including columnar-index and missing-field warning exclusions.
pub fn persistent_job_need_gc(job: &mut astersql_meta_model::group_3::Job) -> bool {
    use astersql_meta_model::group_3::*;
    if job.state == JobState::Cancelled
        || job
            .warning
            .as_ref()
            .is_some_and(|w| w.starts_with("[ddl:1091]"))
    {
        return false;
    }
    match job.tp {
        ACTION_DROP_SCHEMA
        | ACTION_DROP_TABLE
        | ACTION_DROP_MATERIALIZED_VIEW
        | ACTION_DROP_MATERIALIZED_VIEW_LOG
        | ACTION_DROP_MATERIALIZED_VIEW_SHADOW
        | ACTION_TRUNCATE_TABLE
        | ACTION_DROP_PRIMARY_KEY
        | ACTION_DROP_TABLE_PARTITION
        | ACTION_TRUNCATE_TABLE_PARTITION
        | ACTION_DROP_COLUMN
        | ACTION_MODIFY_COLUMN
        | ACTION_ADD_INDEX
        | ACTION_ADD_PRIMARY_KEY
        | ACTION_REORGANIZE_PARTITION
        | ACTION_REMOVE_PARTITIONING
        | ACTION_ALTER_TABLE_PARTITIONING
        | ACTION_MVIEW_REFRESH_OUT_OF_PLACE_CUTOVER => true,
        ACTION_CREATE_MATERIALIZED_VIEW => job.state == JobState::RollbackDone && job.table_id != 0,
        ACTION_DROP_INDEX => finished_index_args(job)
            .ok()
            .and_then(|a| a.IndexArgs.first().map(|i| !i.IsColumnar))
            .unwrap_or(false),
        ACTION_MULTI_SCHEMA_CHANGE => job.multi_schema_info.as_ref().is_some_and(|info| {
            info.sub_jobs
                .iter()
                .enumerate()
                .any(|(i, sub)| persistent_job_need_gc(&mut sub.to_proxy_job(job, i as i32)))
        }),
        _ => false,
    }
}

/// Register full Go finished arguments, sharing one element allocator across
/// sub-jobs. CurrentVersion is obtained once per proxy job, before decoding.
pub fn add_persistent_delete_range_job(
    executor: &mut dyn DeleteRangeExecutor,
    job: &mut astersql_meta_model::group_3::Job,
) -> Result<(), String> {
    let mut allocator = ElementIdAllocator::default();
    if let Some(info) = &job.multi_schema_info {
        for (i, sub) in info.sub_jobs.iter().enumerate() {
            let mut proxy = sub.to_proxy_job(job, i as i32);
            if persistent_job_need_gc(&mut proxy) {
                persist_job_ranges(executor, &mut proxy, &mut allocator)?;
            }
        }
    } else {
        persist_job_ranges(executor, job, &mut allocator)?;
    }
    Ok(())
}
fn persist_job_ranges(
    executor: &mut dyn DeleteRangeExecutor,
    job: &mut astersql_meta_model::group_3::Job,
    allocator: &mut ElementIdAllocator,
) -> Result<(), String> {
    let ts = executor.current_version()?;
    for batch in finished_range_batches(job, allocator)? {
        if batch.is_empty() {
            continue;
        }
        let values = batch
            .iter()
            .map(|task| {
                let hex = |b: &[u8]| b.iter().map(|b| format!("{b:02x}")).collect::<String>();
                format!(
                    "({},{},'{}','{}',{})",
                    task.job_id,
                    task.element_id,
                    hex(&task.start_key),
                    hex(&task.end_key),
                    ts
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        executor.execute(&format!("INSERT IGNORE INTO mysql.gc_delete_range (job_id,element_id,start_key,end_key,ts) VALUES {values}"))?;
    }
    Ok(())
}

fn finished_range_batches(
    job: &mut astersql_meta_model::group_3::Job,
    allocator: &mut ElementIdAllocator,
) -> Result<Vec<Vec<DeleteRangeTask>>, String> {
    use astersql_meta_model::{group_2::*, group_3::*};
    let mut legacy;
    let job = if job.version == JobVersion::V1 {
        legacy = normalized_legacy_gc_job(job)?;
        &mut legacy
    } else {
        job
    };
    // This identity-only value lets the existing range helpers allocate the same
    // table/index element IDs as Go, without replacing the full persisted Job.
    let identity = DeleteRangeJob {
        id: job.id,
        table_id: job.table_id,
        action: DeleteRangeAction::Other,
        rollback_done: false,
        cancelled: false,
        old_physical_table_ids: vec![],
        partition_ids: vec![],
        index_arguments: vec![],
        index_ids: vec![],
        old_global_indexes: vec![],
        subjobs: vec![],
    };
    let mut batches = Vec::new();
    match job.tp {
        ACTION_DROP_SCHEMA => {
            let args = GetFinishedDropSchemaArgs(job)?;
            for ids in args
                .AllDroppedTableIDs
                .chunks(BATCH_INSERT_DELETE_RANGE_SIZE)
            {
                batches.push(table_tasks(&identity, ids, allocator));
            }
        }
        ACTION_DROP_TABLE
        | ACTION_DROP_MATERIALIZED_VIEW
        | ACTION_DROP_MATERIALIZED_VIEW_LOG
        | ACTION_DROP_MATERIALIZED_VIEW_SHADOW
        | ACTION_TRUNCATE_TABLE => {
            let ids = if job.version == JobVersion::V1 {
                legacy_finished_table_ids(job)?
            } else if job.tp == ACTION_TRUNCATE_TABLE {
                GetFinishedTruncateTableArgs(job)?.OldPartitionIDs
            } else {
                GetFinishedDropTableArgs(job)?.OldPartitionIDs
            };
            if !ids.is_empty() {
                batches.push(table_tasks(&identity, &ids, allocator));
            }
            batches.push(table_tasks(&identity, &[job.table_id], allocator));
        }
        ACTION_CREATE_MATERIALIZED_VIEW => {
            if job.state == JobState::RollbackDone && job.table_id != 0 {
                batches.push(table_tasks(&identity, &[job.table_id], allocator));
            }
        }
        ACTION_MVIEW_REFRESH_OUT_OF_PLACE_CUTOVER => {
            let args = GetRefreshMaterializedViewCompleteOutOfPlaceCutoverArgs(job)?;
            batches.push(table_tasks(&identity, &[args.OldMViewID], allocator));
        }
        ACTION_DROP_TABLE_PARTITION
        | ACTION_REORGANIZE_PARTITION
        | ACTION_REMOVE_PARTITIONING
        | ACTION_ALTER_TABLE_PARTITIONING => {
            let args = GetFinishedTablePartitionArgs(job)?;
            if job.tp != ACTION_DROP_TABLE_PARTITION {
                for idx in args.OldGlobalIndexes {
                    batches.push(index_tasks(
                        &identity,
                        idx.TableID,
                        &[idx.IndexID],
                        allocator,
                    ));
                }
            }
            batches.push(table_tasks(&identity, &args.OldPhysicalTblIDs, allocator));
        }
        ACTION_TRUNCATE_TABLE_PARTITION => {
            let args = GetTruncateTableArgs(job)?;
            batches.push(table_tasks(&identity, &args.OldPartitionIDs, allocator));
        }
        ACTION_ADD_INDEX | ACTION_ADD_PRIMARY_KEY => {
            let args = finished_index_args(job)?;
            let physical = if args.PartitionIDs.is_empty() {
                vec![job.table_id]
            } else {
                args.PartitionIDs
            };
            for idx in args.IndexArgs {
                let temp = TEMPORARY_INDEX_PREFIX | idx.IndexID;
                let ids = if job.state == JobState::RollbackDone {
                    vec![idx.IndexID, temp]
                } else {
                    vec![temp]
                };
                if idx.IsGlobal {
                    batches.push(index_tasks(&identity, job.table_id, &ids, allocator));
                } else {
                    for pid in &physical {
                        batches.push(index_tasks(&identity, *pid, &ids, allocator));
                    }
                }
            }
        }
        ACTION_DROP_INDEX | ACTION_DROP_PRIMARY_KEY => {
            let args = finished_index_args(job)?;
            let idx = args
                .IndexArgs
                .first()
                .ok_or("missing finished index argument")?;
            let physical = if args.PartitionIDs.is_empty() {
                vec![job.table_id]
            } else {
                args.PartitionIDs
            };
            for pid in physical {
                batches.push(index_tasks(&identity, pid, &[idx.IndexID], allocator));
            }
        }
        ACTION_DROP_COLUMN | ACTION_MODIFY_COLUMN => {
            let (ids, partitions) = if job.tp == ACTION_DROP_COLUMN {
                crate::persistent_drop_column::finished_range_ids(job)?
            } else {
                crate::persistent_modify_column::finished_range_ids(job)?
            };
            if !ids.is_empty() {
                let physical = if partitions.is_empty() {
                    vec![job.table_id]
                } else {
                    partitions
                };
                for pid in physical {
                    batches.push(index_tasks(&identity, pid, &ids, allocator));
                }
            }
        }
        _ => {}
    }
    Ok(batches)
}

// Go's deprecated StartKey is []byte (JSON base64), not a numeric JSON array.
// Validate its wire type even though Go no longer uses it to form the range.
// Keep the original raw arguments intact for history and retry.
fn legacy_finished_table_ids(job: &astersql_meta_model::group_3::Job) -> Result<Vec<i64>, String> {
    use astersql_meta_model::{group_2::JobArgsCompat, group_3::ACTION_TRUNCATE_TABLE};
    let mut key = serde_json::Value::Null;
    let mut ids: Option<Vec<i64>> = None;
    let mut rules: Option<Vec<String>> = None;
    if job.tp == ACTION_TRUNCATE_TABLE {
        job.decodeArgs((&mut key, &mut ids))
            .map_err(|e| e.to_string())?;
    } else {
        job.decodeArgs((&mut key, &mut ids, &mut rules))
            .map_err(|e| e.to_string())?;
    }
    let valid = match key {
        serde_json::Value::Null => true,
        serde_json::Value::Array(bytes) => {
            bytes.iter().all(|v| v.as_u64().is_some_and(|v| v <= 255))
        }
        serde_json::Value::String(encoded) => {
            // Go base64.StdEncoding ignores CR/LF and permits noncanonical
            // trailing bits, but rejects misplaced padding and other bytes.
            let bytes: Vec<_> = encoded
                .bytes()
                .filter(|b| !matches!(b, b'\r' | b'\n'))
                .collect();
            let data = bytes.iter().position(|b| *b == b'=').unwrap_or(bytes.len());
            let padding = bytes.len() - data;
            bytes.len() % 4 == 0
                && padding <= 2
                && bytes[..data]
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/'))
                && bytes[data..].iter().all(|b| *b == b'=')
                && (padding == 0
                    || (padding == 1 && data % 4 == 3)
                    || (padding == 2 && data % 4 == 2))
        }
        _ => false,
    };
    if !valid {
        return Err("invalid Go finished StartKey byte encoding".into());
    }
    Ok(ids.unwrap_or_default())
}

fn finished_index_args(
    job: &mut astersql_meta_model::group_3::Job,
) -> Result<astersql_meta_model::group_2::ModifyIndexArgs, String> {
    use astersql_meta_model::{group_2::GetFinishedModifyIndexArgs, group_3::JobVersion};
    if job.version == JobVersion::V1 {
        GetFinishedModifyIndexArgs(&mut normalized_legacy_gc_job(job)?)
    } else {
        GetFinishedModifyIndexArgs(job)
    }
}

// encoding/json writes nil Go slices as null. Serde Vec expects an array;
// normalize only slice positions on a temporary job, preserving stored args.
fn normalized_legacy_gc_job(
    job: &mut astersql_meta_model::group_3::Job,
) -> Result<astersql_meta_model::group_3::Job, String> {
    use astersql_meta_model::group_3::*;
    let mut copy =
        Job::decode(&job.encode(false).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let mut value: serde_json::Value =
        serde_json::from_slice(&copy.raw_args).map_err(|e| e.to_string())?;
    let positions: &[usize] = match job.tp {
        ACTION_DROP_SCHEMA => &[0],
        ACTION_DROP_TABLE
        | ACTION_DROP_MATERIALIZED_VIEW
        | ACTION_DROP_MATERIALIZED_VIEW_LOG
        | ACTION_DROP_MATERIALIZED_VIEW_SHADOW => &[1, 2],
        ACTION_TRUNCATE_TABLE => &[1],
        ACTION_DROP_TABLE_PARTITION
        | ACTION_REORGANIZE_PARTITION
        | ACTION_REMOVE_PARTITIONING
        | ACTION_ALTER_TABLE_PARTITIONING
        | ACTION_TRUNCATE_TABLE_PARTITION => &[0, 1],
        ACTION_ADD_INDEX | ACTION_ADD_PRIMARY_KEY => &[2],
        ACTION_DROP_INDEX | ACTION_DROP_PRIMARY_KEY => &[3],
        ACTION_DROP_COLUMN => &[2, 3],
        ACTION_MODIFY_COLUMN => &[0, 1, 2],
        _ => &[],
    };
    if let Some(args) = value.as_array_mut() {
        for index in positions {
            if let Some(v) = args.get_mut(*index)
                && v.is_null()
            {
                *v = serde_json::json!([]);
            }
        }
    }
    copy.raw_args = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
    Ok(copy)
}
