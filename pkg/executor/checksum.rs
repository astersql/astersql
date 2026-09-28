// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// ADMIN CHECKSUM TABLE 执行器：对表/索引做分布式校验和扫描并汇总结果。
//
// Checksum（校验和）用于核对副本或导入导出后的数据一致性：对各物理表与 Public
// 索引发起 DistSQL（分布式 SQL）扫描，按 CRC64-XOR 合并 checksum，并累加 KV 数与字节数。
// 并发度由会话变量控制，worker 池从任务队列取请求并回传结果。
#![allow(non_snake_case)]

use std::collections::{HashMap, VecDeque};
use std::fmt::Display;
use std::num::ParseIntError;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Mutex};

/// 单次 DistSQL checksum 扫描返回的聚合结果。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChecksumResponse {
    /// CRC64-XOR 校验和。
    pub checksum: u64,
    /// 扫描到的键值对总数。
    pub total_kvs: u64,
    /// 扫描到的总字节数。
    pub total_bytes: u64,
}

/// 逻辑库信息（输出结果中的库名列）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatabaseInfo {
    /// 数据库名。
    pub name: String,
}

/// 索引 schema 状态；仅 Public 索引参与 checksum。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaState {
    /// 对用户可见的正式状态。
    Public,
    /// 其他未就绪状态（如 DeleteOnly），跳过校验。
    Other,
}

/// 索引元信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexInfo {
    /// 索引 ID。
    pub id: i64,
    /// 当前 schema 状态。
    pub state: SchemaState,
}

/// 单个分区定义。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartitionDefinition {
    /// 分区对应的物理表 ID。
    pub id: i64,
}

/// 分区表的分区集合。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartitionInfo {
    /// 各分区定义列表。
    pub definitions: Vec<PartitionDefinition>,
}

/// 待校验的逻辑表元信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableInfo {
    /// 逻辑表 ID。
    pub id: i64,
    /// 表名。
    pub name: String,
    /// 是否为 clustered index / common handle（主键即行键）。
    pub is_common_handle: bool,
    /// 表上索引列表。
    pub indices: Vec<IndexInfo>,
    /// 分区信息；非分区表为 None。
    pub partition: Option<PartitionInfo>,
}

/// DistSQL checksum 扫描目标：表数据或索引。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChecksumScanOn {
    /// 扫描表（行数据 / 主键范围）。
    Table,
    /// 扫描二级索引。
    Index,
}

/// 校验和算法；当前仅支持 CRC64-XOR。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChecksumAlgorithm {
    /// 对分片结果按 64 位 CRC 异或合并。
    Crc64Xor,
}

/// 传给 DistSQL RequestBuilder 适配器的完整请求规格。
/// Complete input passed to the DistSQL RequestBuilder adapter.
pub struct ChecksumRequestSpec<R, T, G, S> {
    pub scan_on: ChecksumScanOn,
    pub algorithm: ChecksumAlgorithm,
    pub physical_table_id: i64,
    pub index_id: Option<i64>,
    pub common_handle: bool,
    pub ranges: Vec<R>,
    pub start_ts: u64,
    pub concurrency: usize,
    pub resource_group_tagger: T,
    pub resource_group_name: G,
    pub explicit_request_source_type: S,
}

/// DistSQL checksum 结果流：逐块拉取原始响应并关闭。
pub trait ChecksumResultStream {
    type Context;
    type Error;

    fn next_raw(&mut self, context: &Self::Context) -> Result<Option<Vec<u8>>, Self::Error>;
    fn close(&mut self) -> Result<(), Self::Error>;
}

/// 生产边界：BaseExecutor、会话变量、RequestBuilder、DistSQL checksum、日志与 SQL killer。
/// Production boundary for BaseExecutor, session variables, RequestBuilder,
/// DistSQL checksum calls, logging, and the SQL killer.
pub trait ChecksumBackend: Sync {
    type Context: Clone;
    type Error: Display + Send;
    type Request: Send;
    type Range;
    type ResourceGroupTagger;
    type ResourceGroupName;
    type RequestSourceType;
    type ResultStream: ChecksumResultStream<Context = Self::Context, Error = Self::Error>;

    fn open_base(&self, context: &Self::Context) -> Result<(), Self::Error>;
    fn session_context(&self) -> Self::Context;
    fn checksum_concurrency_variable(&self) -> Result<String, Self::Error>;
    fn invalid_checksum_concurrency(&self, value: &str, error: ParseIntError) -> Self::Error;
    fn zero_checksum_concurrency(&self) -> Self::Error;

    fn full_not_null_range(&self) -> Vec<Self::Range>;
    fn full_int_range(&self, unsigned: bool) -> Vec<Self::Range>;
    fn full_range(&self) -> Vec<Self::Range>;
    fn dist_sql_scan_concurrency(&self) -> usize;
    fn resource_group_tagger(&self) -> Self::ResourceGroupTagger;
    fn resource_group_name(&self) -> Self::ResourceGroupName;
    fn explicit_request_source_type(&self) -> Self::RequestSourceType;
    fn build_request(
        &self,
        request: ChecksumRequestSpec<
            Self::Range,
            Self::ResourceGroupTagger,
            Self::ResourceGroupName,
            Self::RequestSourceType,
        >,
    ) -> Result<Self::Request, Self::Error>;

    fn handle_kill_signal(&self) -> Result<(), Self::Error>;
    fn checksum(&self, request: &Self::Request) -> Result<Self::ResultStream, Self::Error>;
    fn decode_checksum_response(&self, data: &[u8]) -> Result<ChecksumResponse, Self::Error>;
    fn after_handle_checksum_request(&self);

    fn warn_checksum_failed(&self, context: &Self::Context, error: &Self::Error);
    fn info_checksum_result(
        &self,
        context: &Self::Context,
        table_id: i64,
        physical_table_id: i64,
        index_id: i64,
        response: &ChecksumResponse,
    );
}

/// 输出结果 chunk：写入库名、表名与汇总校验列。
pub trait ChecksumOutputChunk {
    fn reset(&mut self);
    fn append_string(&mut self, column: usize, value: &str);
    fn append_u64(&mut self, column: usize, value: u64);
}

/// ADMIN CHECKSUM TABLE 执行器：并发执行任务并在 Next 中吐出汇总行。
pub struct ChecksumTableExec<B: ChecksumBackend> {
    /// 后端适配器（打开、构请求、解码等）。
    pub BaseExecutor: B,
    /// 逻辑表 ID → 校验上下文。
    pub tables: HashMap<i64, ChecksumContext>,
    /// 是否已输出过结果（只吐一次）。
    pub done: bool,
}

impl<B: ChecksumBackend> ChecksumTableExec<B> {
    /// 打开执行器：按并发度启动 worker，汇总各任务结果。
    pub fn Open(&mut self, context: B::Context) -> Result<(), B::Error> {
        self.BaseExecutor.open_base(&context)?;
        // 解析会话变量得到 worker 并发度。
        let concurrency = getChecksumTableConcurrency(&self.BaseExecutor)?;
        if concurrency == 0 {
            return Err(self.BaseExecutor.zero_checksum_concurrency());
        }
        // 为每张逻辑表展开物理表/索引任务。
        let tasks = self.buildTasks()?;
        let task_count = tasks.len();
        let task_queue = Arc::new(Mutex::new(VecDeque::from(tasks)));
        let (result_sender, result_receiver) = sync_channel(task_count.max(1));

        // 按并发度启动 worker，共享任务队列。
        std::thread::scope(|scope| {
            for _ in 0..concurrency {
                let task_queue = Arc::clone(&task_queue);
                let result_sender = result_sender.clone();
                let backend = &self.BaseExecutor;
                scope.spawn(move || {
                    Self::checksumWorker(backend, &task_queue, result_sender);
                });
            }
        });
        drop(result_sender);

        // 收集每个任务恰好一次的回传结果。
        let mut last_error = None;
        for _ in 0..task_count {
            let result = result_receiver
                .recv()
                .expect("every checksum task sends exactly one result");
            match result.response {
                Err(error) => {
                    self.BaseExecutor.warn_checksum_failed(&context, &error);
                    // The Go named return value is overwritten for every
                    // failed result, so the final observed error is returned.
                    last_error = Some(error);
                }
                Ok(response) => {
                    self.BaseExecutor.info_checksum_result(
                        &context,
                        result.tableID,
                        result.physicalTableID,
                        result.indexID,
                        &response,
                    );
                    self.handleResult(result.tableID, response);
                }
            }
        }
        match last_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// 将各表汇总结果写入输出 chunk；仅执行一次。
    pub fn Next<C: ChecksumOutputChunk>(
        &mut self,
        _context: B::Context,
        request: &mut C,
    ) -> Result<(), B::Error> {
        request.reset();
        if self.done {
            return Ok(());
        }
        for table in self.tables.values() {
            request.append_string(0, &table.dbInfo.name);
            request.append_string(1, &table.tableInfo.name);
            request.append_u64(2, table.response.checksum);
            request.append_u64(3, table.response.total_kvs);
            request.append_u64(4, table.response.total_bytes);
        }
        self.done = true;
        Ok(())
    }

    /// 汇总所有逻辑表的 DistSQL 任务列表。
    fn buildTasks(&self) -> Result<Vec<ChecksumTask<B::Request>>, B::Error> {
        let mut all_tasks = Vec::with_capacity(self.tables.len());
        let mut task_count = 0;
        for table in self.tables.values() {
            let tasks = table.buildTasks(&self.BaseExecutor)?;
            task_count += tasks.len();
            all_tasks.push(tasks);
        }
        let mut result = Vec::with_capacity(task_count);
        for mut tasks in all_tasks {
            result.append(&mut tasks);
        }
        Ok(result)
    }

    /// 将单任务响应合并进对应逻辑表上下文。
    fn handleResult(&mut self, table_id: i64, response: ChecksumResponse) {
        self.tables
            .get_mut(&table_id)
            .expect("checksum result refers to an existing logical table")
            .handleResponse(&response);
    }

    /// Worker：从队列取任务、执行请求、回传结果。
    fn checksumWorker(
        backend: &B,
        task_queue: &Mutex<VecDeque<ChecksumTask<B::Request>>>,
        result_sender: SyncSender<ChecksumResult<B::Error>>,
    ) {
        loop {
            let task = task_queue
                .lock()
                .expect("checksum task queue lock poisoned")
                .pop_front();
            let Some(task) = task else {
                return;
            };
            let result = ChecksumResult {
                tableID: task.tableID,
                physicalTableID: task.physicalTableID,
                indexID: task.indexID,
                response: Self::handleChecksumRequest(backend, &task.request),
            };
            if result_sender.send(result).is_err() {
                return;
            }
        }
    }

    /// 发起一次 checksum 请求并合并流式响应；Close 错误优先。
    fn handleChecksumRequest(
        backend: &B,
        request: &B::Request,
    ) -> Result<ChecksumResponse, B::Error> {
        backend.handle_kill_signal()?;
        let context = backend.session_context();
        let mut result_stream = backend.checksum(request)?;
        let processing_result = (|| {
            let mut response = ChecksumResponse::default();
            loop {
                let Some(data) = result_stream.next_raw(&context)? else {
                    return Ok(response);
                };
                let update = backend.decode_checksum_response(&data)?;
                updateChecksumResponse(&mut response, &update);
                backend.handle_kill_signal()?;
            }
        })();

        // This matches the Go defer: Close errors override any earlier error,
        // and the failpoint hook runs after Close on every result-stream path.
        let close_result = result_stream.close();
        backend.after_handle_checksum_request();
        match close_result {
            Err(error) => Err(error),
            Ok(()) => processing_result,
        }
    }
}

/// 单个 DistSQL checksum 任务。
pub struct ChecksumTask<R> {
    pub tableID: i64,
    pub physicalTableID: i64,
    pub indexID: i64,
    pub request: R,
}

/// Worker 回传的单任务结果。
pub struct ChecksumResult<E> {
    pub tableID: i64,
    pub physicalTableID: i64,
    pub indexID: i64,
    pub response: Result<ChecksumResponse, E>,
}

/// 单张逻辑表的校验上下文与累计响应。
pub struct ChecksumContext {
    pub dbInfo: DatabaseInfo,
    pub tableInfo: TableInfo,
    pub startTs: u64,
    pub response: ChecksumResponse,
}

/// 构造空累计响应的校验上下文。
pub fn newChecksumContext(
    database: DatabaseInfo,
    table: TableInfo,
    start_ts: u64,
) -> ChecksumContext {
    ChecksumContext {
        dbInfo: database,
        tableInfo: table,
        startTs: start_ts,
        response: ChecksumResponse::default(),
    }
}

impl ChecksumContext {
    /// 为逻辑表及其各分区物理表生成表扫描 + Public 索引任务。
    fn buildTasks<B: ChecksumBackend>(
        &self,
        backend: &B,
    ) -> Result<Vec<ChecksumTask<B::Request>>, B::Error> {
        let partition_definitions = self
            .tableInfo
            .partition
            .as_ref()
            .map(|partition| partition.definitions.as_slice())
            .unwrap_or_default();
        let mut requests = Vec::with_capacity(checksumRequestCount(&self.tableInfo));
        // 先为逻辑表 ID 本身生成请求，再为每个分区物理表生成。
        self.appendRequest4PhysicalTable(
            backend,
            self.tableInfo.id,
            self.tableInfo.id,
            &mut requests,
        )?;
        for partition in partition_definitions {
            self.appendRequest4PhysicalTable(
                backend,
                self.tableInfo.id,
                partition.id,
                &mut requests,
            )?;
        }
        Ok(requests)
    }

    /// 为单个物理表追加表扫描任务与各 Public 索引任务。
    fn appendRequest4PhysicalTable<B: ChecksumBackend>(
        &self,
        backend: &B,
        table_id: i64,
        physical_table_id: i64,
        requests: &mut Vec<ChecksumTask<B::Request>>,
    ) -> Result<(), B::Error> {
        requests.push(ChecksumTask {
            tableID: table_id,
            physicalTableID: physical_table_id,
            indexID: -1,
            request: self.buildTableRequest(backend, physical_table_id)?,
        });
        for index in &self.tableInfo.indices {
            if index.state != SchemaState::Public {
                continue;
            }
            requests.push(ChecksumTask {
                tableID: table_id,
                physicalTableID: physical_table_id,
                indexID: index.id,
                request: self.buildIndexRequest(backend, physical_table_id, index)?,
            });
        }
        Ok(())
    }

    /// 构造表数据 checksum 请求（按 common handle 选择范围）。
    fn buildTableRequest<B: ChecksumBackend>(
        &self,
        backend: &B,
        physical_table_id: i64,
    ) -> Result<B::Request, B::Error> {
        let ranges = if self.tableInfo.is_common_handle {
            backend.full_not_null_range()
        } else {
            backend.full_int_range(false)
        };
        backend.build_request(ChecksumRequestSpec {
            scan_on: ChecksumScanOn::Table,
            algorithm: ChecksumAlgorithm::Crc64Xor,
            physical_table_id,
            index_id: None,
            common_handle: self.tableInfo.is_common_handle,
            ranges,
            start_ts: self.startTs,
            concurrency: backend.dist_sql_scan_concurrency(),
            resource_group_tagger: backend.resource_group_tagger(),
            resource_group_name: backend.resource_group_name(),
            explicit_request_source_type: backend.explicit_request_source_type(),
        })
    }

    /// 构造二级索引 checksum 请求（全范围扫描）。
    fn buildIndexRequest<B: ChecksumBackend>(
        &self,
        backend: &B,
        physical_table_id: i64,
        index: &IndexInfo,
    ) -> Result<B::Request, B::Error> {
        backend.build_request(ChecksumRequestSpec {
            scan_on: ChecksumScanOn::Index,
            algorithm: ChecksumAlgorithm::Crc64Xor,
            physical_table_id,
            index_id: Some(index.id),
            common_handle: false,
            ranges: backend.full_range(),
            start_ts: self.startTs,
            concurrency: backend.dist_sql_scan_concurrency(),
            resource_group_tagger: backend.resource_group_tagger(),
            resource_group_name: backend.resource_group_name(),
            explicit_request_source_type: backend.explicit_request_source_type(),
        })
    }

    /// 将增量响应 XOR/累加进本表累计结果。
    fn handleResponse(&mut self, update: &ChecksumResponse) {
        updateChecksumResponse(&mut self.response, update);
    }
}

/// 计算表需要发起的 DistSQL 请求数：物理表数 × (Public 索引数 + 1)。
pub fn checksumRequestCount(table: &TableInfo) -> usize {
    let physical_tables = table
        .partition
        .as_ref()
        .map_or(1, |partition| partition.definitions.len() + 1);
    let public_indices = table
        .indices
        .iter()
        .filter(|index| index.state == SchemaState::Public)
        .count();
    physical_tables * (public_indices + 1)
}

/// 从会话变量解析 checksum 并发度。
pub fn getChecksumTableConcurrency<B: ChecksumBackend>(backend: &B) -> Result<usize, B::Error> {
    let value = backend.checksum_concurrency_variable()?;
    value
        .parse::<usize>()
        .map_err(|error| backend.invalid_checksum_concurrency(&value, error))
}

/// 合并两份 ChecksumResponse：checksum 异或，计数累加。
pub fn updateChecksumResponse(response: &mut ChecksumResponse, update: &ChecksumResponse) {
    response.checksum ^= update.checksum;
    response.total_kvs += update.total_kvs;
    response.total_bytes += update.total_bytes;
}
