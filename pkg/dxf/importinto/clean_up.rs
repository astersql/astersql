// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Import Into 任务完成后的清理逻辑。
//
// Import Into 是把外部数据批量导入表的分布式任务；全部 write-and-ingest
//（写入并排序后灌入存储）子任务结束后，本模块恢复表模式、清理云端排序文件，
// 并在 next-gen 成功路径上报计量（metering）数据。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use astersql_dxf_framework_proto::{Task, TaskStateSucceed};
use astersql_errors as errors;
use astersql_lightning_verification::DataKVGroupID;

use crate::proto::{PostProcessStepMeta, TaskMeta};
use crate::scheduler::redactSensitiveInfo;

#[derive(Debug)]
/// 恢复表模式（table mode）失败时的错误分类。
pub enum RestoreTableModeError {
    /// 目标表已不存在，清理时可忽略。
    TableNotFound,
    /// 其它需向上传递的错误。
    Other(errors::SharedError),
}

/// Cancellation visible to cleanup workers, without cancelling the caller's context.
#[derive(Clone)]
pub struct CleanupMeteringContext {
    parent: astersql_dxf_framework_metering::Context,
    cancelled: Arc<AtomicBool>,
}
impl CleanupMeteringContext {
    fn new(parent: &astersql_dxf_framework_metering::Context) -> Self {
        Self {
            parent: parent.clone(),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
    /// Whether the parent or another metering worker has cancelled this batch.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire) || self.parent.is_cancelled()
    }
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    fn check(&self) -> Result<(), errors::SharedError> {
        if self.is_cancelled() {
            let deadline_exceeded = !self.cancelled.load(Ordering::Acquire)
                && self
                    .parent
                    .deadline()
                    .is_some_and(|deadline| std::time::Instant::now() >= deadline);
            Err(errors::New(if deadline_exceeded {
                "context deadline exceeded"
            } else {
                "context canceled"
            }))
        } else {
            Ok(())
        }
    }
}

const CLEAN_METERING_CONCURRENCY: usize = 4;

/// 清理所需副作用的运行时抽象。
///
/// 具体服务适配器负责 DDL、任务表与对象存储；清理逻辑只统一调用顺序与错误策略。
/// Side effects required by cleanup. The concrete server adapter owns DDL,
/// task-table and object-store construction; cleanup keeps their ordering and
/// error policy in one place.
pub trait ImportCleanUpRuntime: Send + Sync {
    /// 是否经典（classic）内核部署。
    fn is_classic(&self) -> bool;
    /// 是否 next-gen 内核部署。
    fn is_nextgen(&self) -> bool;
    /// 将表从导入模式恢复为正常模式。
    fn restore_table_mode(
        &self,
        database_id: i64,
        table_id: i64,
    ) -> Result<(), RestoreTableModeError>;
    /// Open one store with the original URI, including live credentials.
    fn open_global_sort_store(
        &self,
        cloud_storage_uri: &str,
    ) -> Result<Box<dyn ImportCleanUpStorage>, errors::SharedError>;
    /// 读取后处理步骤元数据（含 checksum 等）。
    fn post_process_meta(
        &self,
        task_id: i64,
    ) -> Result<Option<PostProcessStepMeta>, errors::SharedError>;
    /// Context-aware metadata boundary for cancellable backend implementations.
    fn post_process_meta_with_context(
        &self,
        context: &CleanupMeteringContext,
        task_id: i64,
    ) -> Result<Option<PostProcessStepMeta>, errors::SharedError> {
        context.check()?;
        self.post_process_meta(task_id)
    }
    /// Context-aware send boundary; implementations with blocking IO should observe cancellation.
    fn send_meter_data_with_context(
        &self,
        context: &CleanupMeteringContext,
        task: &Task,
        row_count: i64,
        data_kv_size: i64,
        index_kv_size: i64,
    ) -> Result<(), errors::SharedError> {
        context.check()?;
        self.send_meter_data(task, row_count, data_kv_size, index_kv_size)
    }
    /// 上报导入行数与 KV 体积等到计量系统。
    fn send_meter_data(
        &self,
        task: &Task,
        row_count: i64,
        data_kv_size: i64,
        index_kv_size: i64,
    ) -> Result<(), errors::SharedError>;
}

/// Object-store boundary; cleanup always closes an opened store, even on error.
pub trait ImportCleanUpStorage: astersql_ingestor_globalsort::Storage {
    fn close(&self);
}
struct CleanupStoreGuard(Box<dyn ImportCleanUpStorage>);
impl Drop for CleanupStoreGuard {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Import Into 清理入口，持有运行时适配器。
pub struct ImportCleaner {
    /// 清理副作用实现。
    runtime: Arc<dyn ImportCleanUpRuntime>,
}

impl ImportCleaner {
    /// 用给定运行时构造清理器。
    pub fn new(runtime: Arc<dyn ImportCleanUpRuntime>) -> Self {
        Self { runtime }
    }

    /// 仅在全部 write-and-ingest 子任务结束后执行清理。
    ///
    /// 这些子任务可能共享已排序文件，过早删除会导致并发写入失败。
    /// Cleanup only runs after all write-and-ingest subtasks finish, because
    /// those subtasks can share sorted files.
    pub fn Clean(&self, task: &mut Task) -> Result<(), errors::SharedError> {
        self.BatchClean(std::slice::from_mut(task))
    }

    /// Restore all table modes, delete each URI group with one scan, then meter.
    /// Cleanup and history transfer are not atomic; retries must be safe.
    pub fn BatchClean(&self, tasks: &mut [Task]) -> Result<(), errors::SharedError> {
        self.BatchCleanWithContext(
            &astersql_dxf_framework_metering::Context::background(),
            tasks,
        )
    }

    /// Cleanup with caller cancellation propagated to the parallel metering workers.
    pub fn BatchCleanWithContext(
        &self,
        context: &astersql_dxf_framework_metering::Context,
        tasks: &mut [Task],
    ) -> Result<(), errors::SharedError> {
        let mut groups: HashMap<String, Vec<String>> = HashMap::new();
        let mut meter_tasks = Vec::new();
        for (index, task) in tasks.iter_mut().enumerate() {
            let mut task_meta = TaskMeta::Unmarshal(&task.Meta)?;
            // Capture both values before redaction so construction uses live credentials.
            let uri = task_meta.Plan.CloudStorageURI.clone();
            let global_sort = task_meta.Plan.IsGlobalSort();
            redactSensitiveInfo(task, &mut task_meta);
            if self.runtime.is_classic() {
                let table_id = task_meta
                    .Plan
                    .TableInfo
                    .as_ref()
                    .map(|table| table.ID)
                    .unwrap_or_default();
                match self
                    .runtime
                    .restore_table_mode(task_meta.Plan.DBID, table_id)
                {
                    Ok(()) | Err(RestoreTableModeError::TableNotFound) => {}
                    Err(RestoreTableModeError::Other(error)) => return Err(error),
                }
            }
            if global_sort {
                groups.entry(uri).or_default().push(task.ID.to_string());
                if self.runtime.is_nextgen() && task.State == TaskStateSucceed {
                    meter_tasks.push(index);
                }
            }
        }
        for (uri, dirs) in groups {
            let store = CleanupStoreGuard(self.runtime.open_global_sort_store(&uri)?);
            let dirs: Vec<_> = dirs.iter().map(String::as_str).collect();
            astersql_ingestor_globalsort::CleanUpFilesInDirectories(store.0.as_ref(), &dirs)
                .map_err(|error| errors::New(error.to_string()))?;
        }
        let meter_tasks: Vec<_> = meter_tasks.into_iter().map(|index| &tasks[index]).collect();
        sendMeterOnCleanInParallel(context, &meter_tasks, |context, task| {
            self.sendMeterOnClean(context, task)
        })
    }

    /// 从 post-process 元数据提取行数/数据 KV/索引 KV 大小并发送计量。
    fn sendMeterOnClean(
        &self,
        context: &CleanupMeteringContext,
        task: &Task,
    ) -> Result<(), errors::SharedError> {
        let Some(meta) = self
            .runtime
            .post_process_meta_with_context(context, task.ID)?
        else {
            return Ok(());
        };
        let (row_count, data_kv_size, index_kv_size) = meterDataFromPostProcess(&meta);
        self.runtime.send_meter_data_with_context(
            context,
            task,
            row_count as i64,
            data_kv_size as i64,
            index_kv_size as i64,
        )
    }
}

/// Run bounded workers, stop dispatch after the first error, and join every worker.
pub(crate) fn sendMeterOnCleanInParallel<F>(
    parent: &astersql_dxf_framework_metering::Context,
    tasks: &[&Task],
    send: F,
) -> Result<(), errors::SharedError>
where
    F: Fn(&CleanupMeteringContext, &Task) -> Result<(), errors::SharedError> + Sync,
{
    if tasks.is_empty() {
        return Ok(());
    }
    let context = CleanupMeteringContext::new(parent);
    let pending = Mutex::new(tasks.iter());
    let first_error = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..CLEAN_METERING_CONCURRENCY.min(tasks.len()) {
            let context = &context;
            let pending = &pending;
            let first_error = &first_error;
            let send = &send;
            scope.spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    loop {
                        let task = {
                            let mut pending = pending.lock().unwrap();
                            context.check()?;
                            pending.next().copied()
                        };
                        let Some(task) = task else {
                            return Ok(());
                        };
                        send(context, task)?;
                    }
                }))
                .unwrap_or_else(|panic| {
                    let message = panic
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| panic.downcast_ref::<&str>().copied())
                        .unwrap_or("metering panic");
                    Err(errors::New(message.to_owned()))
                });
                if let Err(error) = result {
                    let mut first = first_error.lock().unwrap();
                    if first.is_none() {
                        *first = Some(error);
                    }
                    context.cancel();
                }
            });
        }
    });
    match first_error.into_inner().unwrap() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// 从后处理 checksum 汇总：(行数, 数据 KV 字节数, 索引 KV 字节数)。
///
/// `DataKVGroupID` 对应数据组；其余 group 视为索引组累加 Size。
pub fn meterDataFromPostProcess(meta: &PostProcessStepMeta) -> (u64, u64, u64) {
    let mut row_count = 0;
    let mut data_kv_size = 0;
    let mut index_kv_size: u64 = 0;
    // 按 checksum 分组区分数据 KV 与索引 KV。
    for (group, checksum) in &meta.Checksum {
        if *group == DataKVGroupID {
            row_count = checksum.KVs;
            data_kv_size = checksum.Size;
        } else {
            // Go's uint64 addition wraps modulo 2^64.
            index_kv_size = index_kv_size.wrapping_add(checksum.Size);
        }
    }
    (row_count, data_kv_size, index_kv_size)
}

impl astersql_dxf_framework_scheduler::Cleaner for ImportCleaner {
    fn clean(
        &self,
        task: &mut astersql_dxf_framework_scheduler::Task,
    ) -> astersql_dxf_framework_scheduler::Result<()> {
        astersql_dxf_framework_scheduler::BatchCleaner::batch_clean(
            self,
            std::slice::from_mut(task),
        )
    }
    fn batch_cleaner(&self) -> Option<&dyn astersql_dxf_framework_scheduler::BatchCleaner> {
        Some(self)
    }

    fn expired_file_cleaner(
        &self,
    ) -> Option<&dyn astersql_dxf_framework_scheduler::ExpiredFileCleaner> {
        Some(self)
    }
}

struct SchedulerTaskInfoGetter<'a> {
    manager: &'a dyn astersql_dxf_framework_scheduler::TaskManager,
}

impl crate::conflictrows::TaskInfoGetter for SchedulerTaskInfoGetter<'_> {
    fn GetTaskCleanupInfoByIDs(
        &self,
        _ctx: &astersql_objstore::storage::Context,
        task_ids: &[i64],
    ) -> anyhow::Result<HashMap<i64, astersql_dxf_framework_storage::TaskCleanupInfo>> {
        self.manager
            .task_cleanup_info_by_ids(task_ids)
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }
}

impl astersql_dxf_framework_scheduler::ExpiredFileCleaner for ImportCleaner {
    fn clean_expired_files(
        &self,
        context: &astersql_dxf_framework_scheduler::Context,
        task_info_getter: &dyn astersql_dxf_framework_scheduler::TaskManager,
        cloud_storage_uri: &str,
    ) -> astersql_dxf_framework_scheduler::Result<()> {
        let storage_context = astersql_objstore::storage::Context::from_cancellation_flag(
            context.cancellation_flag(),
        );
        crate::conflictrows::CleanConflictRowFiles(
            &storage_context,
            &SchedulerTaskInfoGetter {
                manager: task_info_getter,
            },
            cloud_storage_uri,
        )
        .map_err(|error| astersql_dxf_framework_scheduler::SchedulerError::new(error.to_string()))
    }
}
impl astersql_dxf_framework_scheduler::BatchCleaner for ImportCleaner {
    fn batch_clean(
        &self,
        tasks: &mut [astersql_dxf_framework_scheduler::Task],
    ) -> astersql_dxf_framework_scheduler::Result<()> {
        let mut import_tasks = tasks
            .iter()
            .map(crate::scheduler::frameworkTaskToImportTask)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                astersql_dxf_framework_scheduler::SchedulerError::new(error.to_string())
            })?;
        let result = self.BatchClean(&mut import_tasks);
        // Redaction persists even when a later cleanup side effect fails.
        for (task, import_task) in tasks.iter_mut().zip(import_tasks) {
            task.meta = import_task.Meta;
        }
        result.map_err(|error| {
            astersql_dxf_framework_scheduler::SchedulerError::new(error.to_string())
        })
    }
}

/// Register the import cleaner on the owner's actual cleanup capability path.
pub fn RegisterImportCleanerFactory(runtime: Arc<dyn ImportCleanUpRuntime>) {
    astersql_dxf_framework_scheduler::RegisterCleanerFactory(
        astersql_dxf_framework_proto::ImportInto,
        Arc::new(move || Arc::new(ImportCleaner::new(runtime.clone()))),
    );
}
