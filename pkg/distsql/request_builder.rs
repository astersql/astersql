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

// DistSQL KV 请求构造器与表/索引 range 编码。
//
// `RequestBuilder` 在调用 Select 前填充 `KvRequest`：请求类型、payload、key range、
// 会话变量、事务 scope（txn_scope）等。同时提供表 handle / 索引区间到 TiKV key
// 的编码辅助。当前实现通过 `KvRequest` 构造请求。

// RequestBuilder 如何填充 kv.Request，以及 table/index range 如何编码为 kv.KeyRange；
// 不会发送 KV 请求，也不会真正分配 TiKV 任务，Go 依赖均作为占位模块名保留。

use std::sync::Arc;
use std::time::Duration;

use crate::{DistSqlError, DistSqlResult, KeyRange, RequestType, StoreType};

/// 事务隔离级别（简化枚举，对齐常见 Snapshot / ReadCommitted）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum IsolationLevel {
    /// 快照隔离（SI）：读已提交快照。
    #[default]
    Snapshot,
    /// 读已提交（RC）。
    ReadCommitted,
}
/// KV 请求优先级。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Priority {
    Low,
    #[default]
    Normal,
    High,
}
/// 请求载荷：DAG / Analyze / Checksum 的序列化字节。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestPayload {
    /// 尚未设置 payload；Go 的 `kv.Request` 允许构造后再补充请求类型。
    Empty,
    /// DAG（算子图）序列化数据。
    Dag(Vec<u8>),
    /// Analyze 请求序列化数据。
    Analyze(Vec<u8>),
    /// Checksum 请求序列化数据。
    Checksum(Vec<u8>),
}
/// 分区 ID 与其对应的 key ranges（分区表扫描用）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartitionIDAndRanges {
    /// 物理分区表 ID。
    pub partition_id: i64,
    /// 该分区下的扫描区间。
    pub ranges: Vec<KeyRange>,
}

/// 发往存储层的完整 KV 请求描述。
#[derive(Clone)]
pub struct KvRequest {
    /// 请求类型。
    pub request_type: RequestType,
    /// 序列化后的请求体。
    pub payload: RequestPayload,
    /// 非分区 key ranges。
    pub key_ranges: Vec<KeyRange>,
    /// 按分区组织的 ranges。
    pub partition_ranges: Vec<PartitionIDAndRanges>,
    /// 事务 start_ts（MVCC 可见性）。
    pub start_ts: u64,
    /// 是否降序。
    pub descending: bool,
    /// 是否保序。
    pub keep_order: bool,
    /// Coprocessor 并发。
    pub concurrency: usize,
    /// 目标存储。
    pub store_type: StoreType,
    /// 隔离级别。
    pub isolation_level: IsolationLevel,
    /// 优先级。
    pub priority: Priority,
    /// 是否禁止写入 coprocessor cache（Analyze/Checksum 默认开启）。
    pub not_fill_cache: bool,
    /// 是否流式。
    pub streaming: bool,
    /// 是否启用分页拉取。
    pub paging: bool,
    /// 是否允许 Batch Cop（批量 coprocessor）。
    pub allow_batch_cop: bool,
    /// 超时。
    pub timeout: Duration,
    /// 事务作用域（如 global 或具体 DC）。
    pub txn_scope: String,
    /// 读副本作用域。
    pub read_replica_scope: String,
    /// 是否 stale read（读历史快照）。
    pub is_staleness: bool,
    /// 资源组名称。
    pub resource_group_name: String,
    /// 请求来源标签。
    pub request_source: String,
    /// 显式请求来源类型。
    pub explicit_source_type: String,
    /// TiDB 实例 server id。
    pub server_id: u64,
    /// 连接 ID。
    pub connection_id: u64,
    /// 连接别名。
    pub connection_alias: String,
    /// 各 range 预估行数提示。
    pub key_range_hints: Vec<usize>,
    /// Limits in-flight cop requests across requests that share this limiter.
    pub copr_request_limiter: Option<Arc<astersql_kv::CoprRequestLimiter>>,
    /// Limits in-flight cop requests per store for a query.
    pub query_cop_store_limiter: Option<Arc<astersql_kv::QueryCopStoreLimiter>>,
    pub store_batch_size: isize,
    pub allow_batch_task_data_merge: bool,
    pub execute_batch_tasks_serially: bool,
}

/// 从会话变量拷贝到请求的 DistSQL 相关字段子集。
#[derive(Clone, Debug)]
pub struct SessionVars {
    pub isolation_level: IsolationLevel,
    pub priority: Priority,
    pub concurrency: usize,
    pub streaming: bool,
    pub paging: bool,
    pub allow_batch_cop: bool,
    pub txn_scope: String,
    pub read_replica_scope: String,
    pub is_staleness: bool,
    pub resource_group_name: String,
    pub request_source: String,
}
impl Default for SessionVars {
    fn default() -> Self {
        Self {
            isolation_level: IsolationLevel::Snapshot,
            priority: Priority::Normal,
            concurrency: 15,
            streaming: false,
            paging: false,
            allow_batch_cop: false,
            txn_scope: "global".into(),
            read_replica_scope: "global".into(),
            is_staleness: false,
            resource_group_name: String::new(),
            request_source: String::new(),
        }
    }
}

/// 完整版请求构造器：一次性使用，可携带延迟错误与 txn_scope 校验器。
#[derive(Default)]
pub struct RequestBuilder {
    request_type: Option<RequestType>,
    payload: Option<RequestPayload>,
    key_ranges: Vec<KeyRange>,
    partition_ranges: Vec<PartitionIDAndRanges>,
    start_ts: u64,
    descending: bool,
    keep_order: bool,
    concurrency: usize,
    store_type: Option<StoreType>,
    isolation_level: IsolationLevel,
    priority: Priority,
    not_fill_cache: bool,
    streaming: bool,
    paging: bool,
    allow_batch_cop: bool,
    timeout: Option<Duration>,
    txn_scope: String,
    read_replica_scope: String,
    is_staleness: bool,
    resource_group_name: String,
    request_source: String,
    explicit_source_type: String,
    server_id: u64,
    connection_id: u64,
    connection_alias: String,
    hints: Vec<usize>,
    copr_request_limiter: Option<Arc<astersql_kv::CoprRequestLimiter>>,
    store_batch_size: isize,
    allow_batch_task_data_merge: bool,
    execute_batch_tasks_serially: bool,
    used: bool,
    error: Option<DistSqlError>,
    scope_checker: Option<Arc<dyn TxnScopeChecker>>,
}

impl RequestBuilder {
    /// 创建构造器并设置 Go `kv.Request` 的零值语义。
    pub fn new() -> Self {
        Self {
            concurrency: 0,
            isolation_level: IsolationLevel::Snapshot,
            priority: Priority::Normal,
            txn_scope: "global".into(),
            read_replica_scope: "global".into(),
            ..Self::default()
        }
    }
    /// 构建最终 KvRequest；同一 builder 成功后不可复用。
    pub fn Build(&mut self) -> DistSqlResult<KvRequest> {
        if self.used {
            return Err(DistSqlError("RequestBuilder cannot be reused".into()));
        }
        self.used = true;
        // 延迟错误优先返回，并允许调用方重试（重置 used）。
        if let Some(error) = self.error.take() {
            self.used = false;
            return Err(error);
        }
        self.verifyTxnScope()?;
        let request_type = self.request_type.unwrap_or(RequestType::Dag);
        let payload = self.payload.clone().unwrap_or(RequestPayload::Empty);
        Ok(KvRequest {
            request_type,
            payload,
            key_ranges: self.key_ranges.clone(),
            partition_ranges: self.partition_ranges.clone(),
            start_ts: self.start_ts,
            descending: self.descending,
            keep_order: self.keep_order,
            concurrency: self.concurrency,
            store_type: self.store_type.unwrap_or(StoreType::TiKv),
            isolation_level: self.isolation_level,
            priority: self.priority,
            not_fill_cache: self.not_fill_cache,
            streaming: self.streaming,
            paging: self.paging,
            allow_batch_cop: self.allow_batch_cop,
            timeout: self.timeout.unwrap_or(Duration::from_secs(60)),
            txn_scope: self.txn_scope.clone(),
            read_replica_scope: self.read_replica_scope.clone(),
            is_staleness: self.is_staleness,
            resource_group_name: self.resource_group_name.clone(),
            request_source: self.request_source.clone(),
            explicit_source_type: self.explicit_source_type.clone(),
            server_id: self.server_id,
            connection_id: self.connection_id,
            connection_alias: self.connection_alias.clone(),
            key_range_hints: self.hints.clone(),
            copr_request_limiter: self.copr_request_limiter.clone(),
            query_cop_store_limiter: None,
            store_batch_size: self.store_batch_size,
            allow_batch_task_data_merge: self.allow_batch_task_data_merge,
            execute_batch_tasks_serially: self.execute_batch_tasks_serially,
        })
    }
    /// 设置为 DAG 请求并保存 payload。
    pub fn SetDAGRequest(&mut self, payload: Vec<u8>) -> &mut Self {
        self.request_type = Some(RequestType::Dag);
        self.payload = Some(RequestPayload::Dag(payload));
        self
    }
    /// 设置为 Analyze 请求，并指定隔离级别。
    pub fn SetAnalyzeRequest(&mut self, payload: Vec<u8>, isolation: IsolationLevel) -> &mut Self {
        self.request_type = Some(RequestType::Analyze);
        self.payload = Some(RequestPayload::Analyze(payload));
        self.isolation_level = isolation;
        self.priority = Priority::Low;
        self.not_fill_cache = true;
        self
    }
    /// 设置为 Checksum 请求。
    pub fn SetChecksumRequest(&mut self, payload: Vec<u8>) -> &mut Self {
        self.request_type = Some(RequestType::Checksum);
        self.payload = Some(RequestPayload::Checksum(payload));
        self.not_fill_cache = true;
        self
    }
    /// 设置非分区 key ranges。
    pub fn SetKeyRanges(&mut self, ranges: Vec<KeyRange>) -> &mut Self {
        self.key_ranges = ranges;
        self
    }
    /// 设置 key ranges 及行数提示。
    pub fn SetKeyRangesWithHints(&mut self, ranges: Vec<KeyRange>, hints: Vec<usize>) -> &mut Self {
        self.key_ranges = ranges;
        self.hints = hints;
        self
    }
    /// 设置分区 ID 与 ranges。
    pub fn SetPartitionIDAndRanges(&mut self, ranges: Vec<PartitionIDAndRanges>) -> &mut Self {
        self.partition_ranges = ranges;
        self
    }
    /// 设置 start_ts。
    pub fn SetStartTS(&mut self, value: u64) -> &mut Self {
        self.start_ts = value;
        self
    }
    /// 设置降序扫描。
    pub fn SetDesc(&mut self, value: bool) -> &mut Self {
        self.descending = value;
        self
    }
    /// 设置保序。
    pub fn SetKeepOrder(&mut self, value: bool) -> &mut Self {
        self.keep_order = value;
        self
    }
    /// 设置存储类型。
    pub fn SetStoreType(&mut self, value: StoreType) -> &mut Self {
        self.store_type = Some(value);
        self
    }
    /// 设置是否允许 Batch Cop。
    pub fn SetAllowBatchCop(&mut self, value: bool) -> &mut Self {
        self.allow_batch_cop = value;
        self
    }
    /// 设置分页开关。
    pub fn SetPaging(&mut self, value: bool) -> &mut Self {
        self.paging = value;
        self
    }
    /// 设置并发度。
    pub fn SetConcurrency(&mut self, value: usize) -> &mut Self {
        self.concurrency = value;
        self
    }
    pub fn SetCoprRequestLimiter(
        &mut self,
        limiter: Arc<astersql_kv::CoprRequestLimiter>,
    ) -> &mut Self {
        self.copr_request_limiter = Some(limiter);
        self
    }
    pub fn SetStoreBatchSize(&mut self, size: isize) -> &mut Self {
        self.store_batch_size = size;
        self
    }
    pub fn SetAllowBatchTaskDataMerge(&mut self, allow: bool) -> &mut Self {
        self.allow_batch_task_data_merge = allow;
        self
    }
    pub fn SetExecuteBatchTasksSerially(&mut self, serially: bool) -> &mut Self {
        self.execute_batch_tasks_serially = serially;
        self
    }
    /// 设置 TiDB server id。
    pub fn SetTiDBServerID(&mut self, value: u64) -> &mut Self {
        self.server_id = value;
        self
    }
    /// 设置资源组名。
    pub fn SetResourceGroupName(&mut self, value: impl Into<String>) -> &mut Self {
        self.resource_group_name = value.into();
        self
    }
    /// 设置请求来源。
    pub fn SetRequestSource(&mut self, value: impl Into<String>) -> &mut Self {
        self.request_source = value.into();
        self
    }
    /// 设置显式请求来源类型。
    pub fn SetExplicitRequestSourceType(&mut self, value: impl Into<String>) -> &mut Self {
        self.explicit_source_type = value.into();
        self
    }
    /// 设置事务作用域。
    pub fn SetTxnScope(&mut self, value: impl Into<String>) -> &mut Self {
        self.txn_scope = value.into();
        self
    }
    /// 设置读副本作用域。
    pub fn SetReadReplicaScope(&mut self, value: impl Into<String>) -> &mut Self {
        self.read_replica_scope = value.into();
        self
    }
    /// 设置是否 stale read。
    pub fn SetIsStaleness(&mut self, value: bool) -> &mut Self {
        self.is_staleness = value;
        self
    }
    /// 设置连接 ID 与别名。
    pub fn SetConnIDAndConnAlias(&mut self, id: u64, alias: impl Into<String>) -> &mut Self {
        self.connection_id = id;
        self.connection_alias = alias.into();
        self
    }
    /// 从会话变量批量拷贝 DistSQL 相关字段。
    pub fn SetFromSessionVars(&mut self, vars: &SessionVars) -> &mut Self {
        self.isolation_level = vars.isolation_level;
        self.priority = vars.priority;
        if self.concurrency == 0 {
            self.concurrency = vars.concurrency;
        } else {
            self.concurrency = self.concurrency.min(vars.concurrency);
        }
        self.streaming = vars.streaming;
        self.paging = vars.paging;
        self.allow_batch_cop = vars.allow_batch_cop;
        self.txn_scope = vars.txn_scope.clone();
        self.read_replica_scope = vars.read_replica_scope.clone();
        self.is_staleness = vars.is_staleness;
        self.resource_group_name = vars.resource_group_name.clone();
        self.request_source = vars.request_source.clone();
        self
    }
    /// 绑定 schema 侧的 txn_scope 校验器（对齐 Go SetFromInfoSchema）。
    pub fn SetFromInfoSchema(&mut self, checker: Arc<dyn TxnScopeChecker>) -> &mut Self {
        self.scope_checker = Some(checker);
        self
    }
    /// 非 global 的 txn_scope 时，校验各分区物理表是否允许被该 scope 读取。
    fn verifyTxnScope(&self) -> DistSqlResult<()> {
        if self.txn_scope.is_empty() || self.txn_scope == "global" {
            return Ok(());
        }
        if let Some(checker) = &self.scope_checker {
            for range in &self.partition_ranges {
                if !checker.verify_txn_scope(&self.txn_scope, range.partition_id) {
                    return Err(DistSqlError(format!(
                        "table {} is outside txn scope {}",
                        range.partition_id, self.txn_scope
                    )));
                }
            }
        }
        Ok(())
    }
}

/// 校验物理表是否符合给定 txn_scope（通常对照 placement leader DC）。
pub trait TxnScopeChecker: Send + Sync {
    /// 返回该物理表 ID 是否可被 `scope` 读取。
    fn verify_txn_scope(&self, scope: &str, physical_table_id: i64) -> bool;
}
/// global scope 恒为 true；否则委托 `TxnScopeChecker`。
pub fn VerifyTxnScope(scope: &str, physical_table_id: i64, checker: &dyn TxnScopeChecker) -> bool {
    scope.is_empty() || scope == "global" || checker.verify_txn_scope(scope, physical_table_id)
}

/// 将 i64 编码为 TiDB 表键用的大端有符号变换字节。
fn encode_i64(value: i64) -> [u8; 8] {
    ((value as u64) ^ (1u64 << 63)).to_be_bytes()
}
/// 表前缀：`t` + encoded table_id。
fn table_prefix(table_id: i64) -> Vec<u8> {
    let mut key = vec![b't'];
    key.extend_from_slice(&encode_i64(table_id));
    key
}
/// 行记录前缀：`t{tid}_r`。
fn record_prefix(table_id: i64) -> Vec<u8> {
    let mut key = table_prefix(table_id);
    key.extend_from_slice(b"_r");
    key
}
/// 索引前缀：`t{tid}_i{index_id}`。
fn index_prefix(table_id: i64, index_id: i64) -> Vec<u8> {
    let mut key = table_prefix(table_id);
    key.extend_from_slice(b"_i");
    key.extend_from_slice(&encode_i64(index_id));
    key
}
/// 计算字典序上的下一个前缀（PrefixNext），用于半开区间上界。
fn prefix_next(mut key: Vec<u8>) -> Vec<u8> {
    for index in (0..key.len()).rev() {
        if key[index] != 0xff {
            key[index] += 1;
            key.truncate(index + 1);
            return key;
        }
    }
    key.push(0);
    key
}

/// 整型 handle（行主键）区间，含开闭标记。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HandleRange {
    pub low: i64,
    pub high: i64,
    pub low_exclusive: bool,
    pub high_inclusive: bool,
}
/// 将表 handle 区间编码为行 key ranges。
pub fn TableRangesToKVRanges(table_id: i64, ranges: &[HandleRange]) -> Vec<KeyRange> {
    let prefix = record_prefix(table_id);
    ranges
        .iter()
        .map(|range| {
            // 按开闭区间调整端点后再编码为 [start, end)。
            let low = encode_i64(range.low);
            let high = encode_i64(range.high);
            let mut start = prefix.clone();
            start.extend_from_slice(&low);
            if range.low_exclusive {
                start = prefix_next(start);
            }
            let mut end = prefix.clone();
            end.extend_from_slice(&high);
            if range.high_inclusive {
                end = prefix_next(end);
            }
            // Go 的 kv.KeyRange 允许空区间（例如被排除的单点），这里不能
            // 通过 KeyRange::new 丢弃该范围。
            KeyRange { start, end }
        })
        .collect()
}

fn handle_range(table_id: i64, first: i64, last: i64) -> KeyRange {
    let prefix = record_prefix(table_id);
    let mut start = prefix.clone();
    start.extend_from_slice(&encode_i64(first));
    let mut end = prefix;
    end.extend_from_slice(&encode_i64(last));
    // PrefixNext 在编码后的 key 上计算，因而也正确覆盖 i64::MAX。
    end = prefix_next(end);
    KeyRange { start, end }
}
/// 将离散 handle 列表转为逐点 key range，并返回提示下标列表。
pub fn TableHandlesToKVRanges(table_id: i64, handles: &[i64]) -> (Vec<KeyRange>, Vec<usize>) {
    if handles.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let mut ranges = Vec::new();
    let mut hints = Vec::new();
    let mut first = handles[0];
    let mut last = first;
    let mut count = 1;
    for &handle in &handles[1..] {
        if last != i64::MAX && handle == last + 1 {
            last = handle;
            count += 1;
        } else {
            ranges.push(handle_range(table_id, first, last));
            hints.push(count);
            first = handle;
            last = handle;
            count = 1;
        }
    }
    ranges.push(handle_range(table_id, first, last));
    hints.push(count);
    (ranges, hints)
}
/// 将 (partition_id, handle) 列表按分区分组后编码为 key ranges。
pub fn PartitionHandlesToKVRanges(handles: &[(i64, i64)]) -> (Vec<KeyRange>, Vec<usize>) {
    let mut ranges = Vec::new();
    let mut hints = Vec::new();
    if handles.is_empty() {
        return (ranges, hints);
    }
    let mut start = handles[0].1;
    let mut last = start;
    let mut partition = handles[0].0;
    let mut count = 1;
    for &(next_partition, handle) in &handles[1..] {
        if next_partition == partition && last != i64::MAX && handle == last + 1 {
            last = handle;
            count += 1;
        } else {
            ranges.push(handle_range(partition, start, last));
            hints.push(count);
            partition = next_partition;
            start = handle;
            last = handle;
            count = 1;
        }
    }
    ranges.push(handle_range(partition, start, last));
    hints.push(count);
    (ranges, hints)
}
/// 将索引列区间编码为索引 seek key ranges（多表 ID 笛卡尔展开）。
pub fn IndexRangesToKVRanges(
    table_ids: &[i64],
    index_id: i64,
    ranges: &[(Vec<u8>, Vec<u8>)],
) -> DistSqlResult<Vec<KeyRange>> {
    let mut result = Vec::new();
    for table_id in table_ids {
        let prefix = index_prefix(*table_id, index_id);
        for (low, high) in ranges {
            let mut start = prefix.clone();
            start.extend_from_slice(low);
            let mut end = prefix.clone();
            end.extend_from_slice(high);
            result.push(KeyRange { start, end });
        }
    }
    Ok(result)
}
/// 将 common handle（聚簇索引主键）区间编码为行 key ranges。
pub fn CommonHandleRangesToKVRanges(
    table_ids: &[i64],
    ranges: &[(Vec<u8>, Vec<u8>)],
) -> DistSqlResult<Vec<KeyRange>> {
    let mut result = Vec::new();
    for table_id in table_ids {
        let prefix = record_prefix(*table_id);
        for (low, high) in ranges {
            let mut start = prefix.clone();
            start.extend_from_slice(low);
            let mut end = prefix.clone();
            end.extend_from_slice(high);
            result.push(KeyRange { start, end });
        }
    }
    Ok(result)
}
/// 按 int64 边界拆分 handle ranges（有符号/无符号）；保序降序时交换两组返回顺序。
pub fn SplitRangesAcrossInt64Boundary(
    ranges: &[HandleRange],
    keep_order: bool,
    descending: bool,
    common_handle: bool,
) -> (Vec<HandleRange>, Vec<HandleRange>) {
    if common_handle {
        return (ranges.to_vec(), Vec::new());
    }
    let mut signed = Vec::new();
    let mut unsigned = Vec::new();
    for range in ranges {
        if range.low < 0 {
            signed.push(range.clone());
        } else {
            unsigned.push(range.clone());
        }
    }
    if keep_order && descending {
        (unsigned, signed)
    } else {
        (signed, unsigned)
    }
}
/// 构造覆盖整表记录前缀及指定索引前缀的全表扫描 ranges。
pub fn BuildTableRanges(table_id: i64, index_ids: &[i64]) -> Vec<KeyRange> {
    let mut result = Vec::new();
    let record = record_prefix(table_id);
    if let Ok(range) = KeyRange::new(record.clone(), prefix_next(record)) {
        result.push(range);
    }
    for index_id in index_ids {
        let prefix = index_prefix(table_id, *index_id);
        if let Ok(range) = KeyRange::new(prefix.clone(), prefix_next(prefix)) {
            result.push(range);
        }
    }
    result
}
/// 估算单 Region 行数常量，用于小 limit 时调整并发（对齐 Go）。
pub const estimatedRegionRowCount: usize = 100_000;
