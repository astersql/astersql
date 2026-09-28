// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Import Into 的 collect-conflicts 子任务执行器。
//
// 在冲突解决（conflict resolution）流程的前半段，按 KV 组串行消费冲突元信息：
// 对每组冲突 KV 创建编码器、收集冲突行与 checksum（校验和），
// 并在子任务结束时把结果写回步骤元数据，供后续 resolve 与远程校验使用。

#![allow(non_camel_case_types, non_snake_case)]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use astersql_dxf_framework_taskexecutor_execute::{Collector, SubtaskSummary};
use astersql_dxf_importinto_conflictedkv::{
    BoundedHandleSet, CollectResult, NewBoundedHandleSet, NewCollectResult,
};
use astersql_errors as errors;
use astersql_executor_importer::{TableImporter, TableKVEncoder};
use astersql_ingestor_engineapi::ConflictInfo;

use crate::conflict_resolution::createEncoders;
use crate::proto::{CollectConflictsStepMeta, newFromKVChecksum};

/// Per-subtask aggregation state for conflict collection. Object-store and
/// cluster-store adapters are supplied by the caller, while this type retains
/// the same memory cap and checksum de-duplication state as Go.
/// 每个子任务的冲突收集聚合状态。对象存储与集群存储适配器由调用方注入；
/// 本类型保留与 Go 一致的内存上限，以及基于 handle 的 checksum 去重状态。
pub struct collectConflictsStepExecutor {
    pub taskID: i64,
    pub tableImporter: TableImporter,
    pub currSubtaskID: i64,
    /// 索引侧已收集的 handle 占用字节数（跨组共享）。
    pub sizeOfHandlesFromIndex: Arc<AtomicI64>,
    /// handle 集合的内存上限（通常为子任务内存容量的一半）。
    pub sizeLimitOfHandlesFromIndex: i64,
    /// 已写出冲突行文件的总大小（跨 worker 共享）。
    pub sizeOfConflictRowFiles: Arc<AtomicI64>,
    pub result: CollectResult,
    /// 有界 handle 集合：用于唯一索引冲突行的 checksum 去重。
    pub sharedHandleSet: BoundedHandleSet,
    pub summary: SubtaskSummary,
}

impl collectConflictsStepExecutor {
    /// 构造执行器；初始 handle 上限为 0，需在子任务开始时 `resetForNewSubtask`。
    pub fn new(task_id: i64, table_importer: TableImporter) -> Self {
        let handle_size = Arc::new(AtomicI64::new(0));
        let result = NewCollectResult(&table_importer.GetKeySpace());
        Self {
            taskID: task_id,
            tableImporter: table_importer,
            currSubtaskID: 0,
            sizeOfHandlesFromIndex: Arc::clone(&handle_size),
            sizeLimitOfHandlesFromIndex: 0,
            sizeOfConflictRowFiles: Arc::new(AtomicI64::new(0)),
            result,
            sharedHandleSet: NewBoundedHandleSet(handle_size, 0),
            summary: SubtaskSummary::default(),
        }
    }

    /// 为新子任务重置计数器、结果与有界 handle 集合。
    /// `memory_capacity` 的一半用作索引 handle 去重的内存上限。
    pub fn resetForNewSubtask(&mut self, subtask_id: i64, memory_capacity: i64) {
        self.currSubtaskID = subtask_id;
        self.sizeOfHandlesFromIndex.store(0, Ordering::Release);
        self.sizeOfConflictRowFiles.store(0, Ordering::Release);
        self.sizeLimitOfHandlesFromIndex = memory_capacity / 2;
        self.result = NewCollectResult(&self.tableImporter.GetKeySpace());
        self.sharedHandleSet = NewBoundedHandleSet(
            Arc::clone(&self.sizeOfHandlesFromIndex),
            self.sizeLimitOfHandlesFromIndex,
        );
    }

    /// Collect groups serially because one row can conflict in several unique
    /// index groups. The callback may fan each group out to `concurrency`
    /// workers after all encoders have been initialized.
    /// 串行处理各 KV 组：同一行可能在多个唯一索引组中冲突。
    /// 回调可在编码器全部初始化后，将该组扇出到 `concurrency` 个 worker。
    pub fn RunGroups<F>(
        &mut self,
        meta: &CollectConflictsStepMeta,
        concurrency: i32,
        mut collect: F,
    ) -> Result<(), errors::SharedError>
    where
        F: FnMut(
            &str,
            &ConflictInfo,
            Vec<TableKVEncoder>,
            &mut BoundedHandleSet,
        ) -> Result<CollectResult, errors::SharedError>,
    {
        let concurrency = concurrency.max(1);
        // 按组串行：先创建编码器，再交给调用方完成实际收集并合并结果。
        for (kv_group, conflict_info) in &meta.Infos.ConflictInfos {
            let encoders = createEncoders(concurrency, &self.tableImporter)?;
            let result = collect(kv_group, conflict_info, encoders, &mut self.sharedHandleSet)?;
            self.result.Merge(Some(&result));
        }
        Ok(())
    }

    /// 子任务结束时把 checksum、冲突行数/文件名与截断标记写回步骤元数据。
    pub fn onFinished(&self, meta: &mut CollectConflictsStepMeta) {
        applyCollectResult(meta, &self.result, self.sharedHandleSet.BoundExceeded());
    }

    /// 关闭底层 TableImporter，释放导入相关资源。
    pub fn Cleanup(&mut self) {
        self.tableImporter.Close();
    }

    /// 刷新并返回实时子任务进度摘要。
    pub fn RealtimeSummary(&mut self) -> &SubtaskSummary {
        self.summary.Update();
        &self.summary
    }

    /// 重置进度计数，供下一子任务复用。
    pub fn ResetSummary(&mut self) {
        self.summary.Reset();
    }
}

/// Apply the aggregate result to subtask metadata exactly as Go's
/// `onFinished` does. Keeping this transformation independent from the live
/// importer makes every persisted field directly testable without replacing
/// the object-store or cluster-store boundary with a weaker mock.
pub(crate) fn applyCollectResult(
    meta: &mut CollectConflictsStepMeta,
    result: &CollectResult,
    too_many_conflicts_from_index: bool,
) {
    meta.Checksum = Some(newFromKVChecksum(&result.Checksum));
    meta.ConflictedRowCount = result.RowCount;
    meta.ConflictedRowFilenames.clone_from(&result.Filenames);
    meta.ConflictedRowRecordingCapped = result.RowRecordingCapped;
    meta.TooManyConflictsFromIndex = too_many_conflicts_from_index;
}

impl Collector for collectConflictsStepExecutor {
    fn Accepted(&self, _accepted: i64) {}

    /// 累加已处理的冲突 KV 条数到子任务进度。
    fn Processed(&self, processed_conflict_kvs: i64, _bytes: i64) {
        self.summary
            .Processed
            .fetch_add(processed_conflict_kvs, Ordering::Relaxed);
    }
}

/// Conflict row files deliberately live outside `<task-id>/`, because cleanup
/// removes that directory while users must be able to inspect these files.
/// 冲突行文件刻意放在 `<task-id>/` 之外：清理会删除任务目录，
/// 但用户仍需能事后检查这些冲突行文件。
pub fn getConflictRowFilenamePrefix(taskID: i64, subtaskID: i64, uuid: &str) -> String {
    Path::new("conflicted-rows")
        .join(taskID.to_string())
        .join(format!("{subtaskID}-{uuid}"))
        .to_string_lossy()
        .into_owned()
}
