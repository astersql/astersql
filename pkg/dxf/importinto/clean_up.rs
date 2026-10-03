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
use std::sync::Arc;

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
pub struct ImportCleanUp {
    /// 清理副作用实现。
    runtime: Arc<dyn ImportCleanUpRuntime>,
}

impl ImportCleanUp {
    /// 用给定运行时构造清理器。
    pub fn new(runtime: Arc<dyn ImportCleanUpRuntime>) -> Self {
        Self { runtime }
    }

    /// 仅在全部 write-and-ingest 子任务结束后执行清理。
    ///
    /// 这些子任务可能共享已排序文件，过早删除会导致并发写入失败。
    /// Cleanup only runs after all write-and-ingest subtasks finish, because
    /// those subtasks can share sorted files.
    pub fn CleanUp(&self, task: &mut Task) -> Result<(), errors::SharedError> {
        self.CleanUpBatch(std::slice::from_mut(task))
    }

    /// Restore all table modes, delete each URI group with one scan, then meter.
    /// Cleanup and history transfer are not atomic; retries must be safe.
    pub fn CleanUpBatch(&self, tasks: &mut [Task]) -> Result<(), errors::SharedError> {
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
        for index in meter_tasks {
            self.sendMeterOnCleanUp(&tasks[index])?;
        }
        Ok(())
    }

    /// 从 post-process 元数据提取行数/数据 KV/索引 KV 大小并发送计量。
    fn sendMeterOnCleanUp(&self, task: &Task) -> Result<(), errors::SharedError> {
        let Some(meta) = self.runtime.post_process_meta(task.ID)? else {
            return Ok(());
        };
        let (row_count, data_kv_size, index_kv_size) = meterDataFromPostProcess(&meta);
        self.runtime.send_meter_data(
            task,
            row_count as i64,
            data_kv_size as i64,
            index_kv_size as i64,
        )
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

impl astersql_dxf_framework_scheduler::CleanUpRoutine for ImportCleanUp {
    fn clean_up(
        &self,
        task: &mut astersql_dxf_framework_scheduler::Task,
    ) -> astersql_dxf_framework_scheduler::Result<()> {
        astersql_dxf_framework_scheduler::BatchCleanUpRoutine::clean_up_batch(
            self,
            std::slice::from_mut(task),
        )
    }
    fn batch_cleanup(&self) -> Option<&dyn astersql_dxf_framework_scheduler::BatchCleanUpRoutine> {
        Some(self)
    }
}
impl astersql_dxf_framework_scheduler::BatchCleanUpRoutine for ImportCleanUp {
    fn clean_up_batch(
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
        let result = self.CleanUpBatch(&mut import_tasks);
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
pub fn RegisterImportCleanUpFactory(runtime: Arc<dyn ImportCleanUpRuntime>) {
    astersql_dxf_framework_scheduler::RegisterSchedulerCleanUpFactory(
        astersql_dxf_framework_proto::ImportInto,
        Arc::new(move || Arc::new(ImportCleanUp::new(runtime.clone()))),
    );
}
