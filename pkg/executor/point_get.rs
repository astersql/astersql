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

// Point Get 执行器：按主键或唯一索引精确读取单行。
//
// 对应 Go point get：先（可选）查唯一索引拿到 handle（行标识），再读行记录；
// 支持悲观锁（SELECT FOR UPDATE）、分区表、缓存表、可重复读屏障与行校验和填充。
// MVCC（多版本并发控制）快照由 Snapshot 提供；Region 级访问封装在依赖注入中。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 全局事务作用域名：表示可读任意 placement 的表/分区。
pub const GLOBAL_TXN_SCOPE: &str = "global";
/// 额外 handle 列的虚拟列 ID（非用户表列）。
pub const EXTRA_HANDLE_ID: i64 = -1;
/// 行校验和（row checksum）虚拟列 ID。
pub const EXTRA_ROW_CHECKSUM_ID: i64 = -2;
/// 带 Snapshot 运行时统计的类型标签。
pub const TP_RUNTIME_STATS_WITH_SNAPSHOT: i32 = 1;

/// Point Get 统一结果类型。
pub type PointGetResult<T = ()> = Result<T, PointGetError>;
/// KV 键字节序列。
pub type Key = Vec<u8>;

/// 可重复读（Repeatable Read）路径上的 failpoint 注入点（测试用）。
pub(crate) fn point_get_repeatable_read_failpoint() {
    let _ = fail::eval(
        "github.com/pingcap/tidb/pkg/executor/pointGetRepeatableReadTest-step1",
        |_| (),
    );
    let _ = fail::eval(
        "github.com/pingcap/tidb/pkg/executor/pointGetRepeatableReadTest-step2",
        |_| (),
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Point Get 错误分类。
pub enum PointGetErrorKind {
    General,
    /// 键不存在。
    NotFound,
    /// Placement Policy（放置策略）与事务作用域不匹配。
    InvalidPlacementPolicy,
    /// 索引查到 handle 但行记录缺失（索引与数据不一致）。
    LookupInconsistent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Point Get 错误载体。
pub struct PointGetError {
    pub kind: PointGetErrorKind,
    pub message: String,
}

impl PointGetError {
    /// 构造一般性错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            kind: PointGetErrorKind::General,
            message: message.into(),
        }
    }

    /// 带错误种类构造。
    pub fn with_kind(kind: PointGetErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// 键不存在错误。
    pub fn not_found() -> Self {
        Self::with_kind(PointGetErrorKind::NotFound, "key does not exist")
    }

    /// 是否为 NotFound。
    fn is_not_found(&self) -> bool {
        self.kind == PointGetErrorKind::NotFound
    }
}

impl fmt::Display for PointGetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PointGetError {}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 简化版 Datum（单元格值）枚举。
pub enum Datum {
    Null,
    Int64(i64),
    Uint64(u64),
    Bytes(Vec<u8>),
    String(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 行 handle：整型主键或公共 handle（common handle，多列编码）。
pub enum Handle {
    Int(i64),
    Common(Vec<Vec<u8>>),
}

impl Handle {
    /// 取整型 handle 值；公共 handle 则报错。
    fn int_value(&self) -> PointGetResult<i64> {
        match self {
            Self::Int(value) => Ok(*value),
            Self::Common(_) => Err(PointGetError::new("common handle has no integer value")),
        }
    }

    /// 取公共 handle 中第 index 列的编码字节。
    fn encoded_column(&self, index: usize) -> PointGetResult<&[u8]> {
        match self {
            Self::Common(columns) => columns
                .get(index)
                .map(Vec::as_slice)
                .ok_or_else(|| PointGetError::new("common handle column is out of range")),
            Self::Int(_) => Err(PointGetError::new("integer handle has no encoded columns")),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 字段类型标志：是否主键、是否需要还原数据。
pub struct FieldType {
    pub primary_key: bool,
    pub needs_restored_data: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 表列元数据。
pub struct ColumnInfo {
    pub id: i64,
    pub name: String,
    pub field_type: FieldType,
    pub origin_default: Datum,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 索引列：相对表列的偏移。
pub struct IndexColumn {
    pub offset: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 索引元数据（含全局索引、公共 handle 读路径等）。
pub struct IndexInfo {
    pub id: i64,
    pub name: String,
    pub global: bool,
    pub common_handle_read: bool,
    pub columns: Vec<IndexColumn>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 单个分区定义：物理 ID 与名称。
pub struct PartitionDefinition {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 分区信息：定义列表与 DDL 期间需忽略的分区 ID。
pub struct PartitionInfo {
    pub definitions: Vec<PartitionDefinition>,
    pub ids_in_ddl_to_ignore: Vec<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 表锁类型（读/只读/写）。
pub enum TableLockType {
    Read,
    ReadOnly,
    Write,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 表元数据：主键形态、列、分区与表锁等。
pub struct TableInfo {
    pub id: i64,
    pub name: String,
    pub temporary: bool,
    pub cache_enabled: bool,
    pub pk_is_handle: bool,
    pub is_common_handle: bool,
    pub columns: Vec<ColumnInfo>,
    pub primary_index: Option<IndexInfo>,
    pub partition: Option<PartitionInfo>,
    pub table_lock: Option<TableLockType>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 输出 Schema。
/// 结果 Schema 中的一列（可含虚拟表达式）。
pub struct SchemaColumn {
    pub id: i64,
    pub return_type: FieldType,
    pub virtual_expression: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Schema {
    pub columns: Vec<SchemaColumn>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Point Get 物理计划：目标表、handle/索引值、锁选项与分区裁剪信息。
pub struct PointGetPlan {
    pub id: i64,
    pub schema: Schema,
    pub table: TableInfo,
    pub handle: Handle,
    pub index: Option<IndexInfo>,
    pub index_values: Vec<Datum>,
    pub lock: bool,
    pub lock_wait_time: i64,
    pub partition_index: Option<usize>,
    pub partition_names: Vec<String>,
    pub columns: Vec<ColumnInfo>,
    pub average_row_size: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 简易列式 Chunk，用于承载解码后的行。
pub struct Chunk {
    pub columns: Vec<Vec<Datum>>,
    pub capacity: usize,
}

impl Chunk {
    /// 按列数与容量构造空 Chunk。
    pub fn new(column_count: usize, capacity: usize) -> Self {
        Self {
            columns: vec![Vec::new(); column_count],
            capacity,
        }
    }

    /// 清空所有列数据，保留列结构。
    pub fn reset(&mut self) {
        for column in &mut self.columns {
            column.clear();
        }
    }

    /// 向指定列追加一个 Datum。
    fn append(&mut self, column: usize, value: Datum) -> PointGetResult {
        let target = self
            .columns
            .get_mut(column)
            .ok_or_else(|| PointGetError::new("chunk column is out of range"))?;
        target.push(value);
        Ok(())
    }

    /// 整列替换。
    fn set_column(&mut self, column: usize, values: Vec<Datum>) -> PointGetResult {
        let target = self
            .columns
            .get_mut(column)
            .ok_or_else(|| PointGetError::new("chunk column is out of range"))?;
        *target = values;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 列元数据与其 Datum，用于行校验和计算。
pub struct ColumnData {
    pub column: ColumnInfo,
    pub datum: Datum,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 悲观加锁结果：是否存在、是否已加锁、以及值字节。
pub struct LockedValue {
    pub exists: bool,
    pub already_locked: bool,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 加锁上下文：仅当存在时加锁、已锁值与未锁值列表。
pub struct LockContext {
    pub lock_only_if_exists: bool,
    pub values: BTreeMap<Key, LockedValue>,
    pub values_not_locked: Vec<(Key, Vec<u8>)>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Snapshot 侧运行时统计（RPC 次数、处理时间、描述）。
pub struct SnapshotRuntimeStats {
    pub command_get_rpc_count: u64,
    pub process_time_micros: u64,
    pub description: String,
    pub read_pool_task_details: Option<astersql_kv::PoolTaskDetails>,
    pub scan_detail: Option<astersql_util_execdetails::execdetails::util::ScanDetail>,
}

impl SnapshotRuntimeStats {
    /// 合并另一份统计。
    fn merge(&mut self, other: &Self) {
        self.command_get_rpc_count = self
            .command_get_rpc_count
            .saturating_add(other.command_get_rpc_count);
        self.process_time_micros = self
            .process_time_micros
            .saturating_add(other.process_time_micros);
        if let Some(pool) = other.read_pool_task_details.as_ref() {
            if let Some(current) = self.read_pool_task_details.as_mut() {
                current.Merge(pool);
            } else {
                self.read_pool_task_details = Some(pool.clone());
            }
        }
        if let Some(scan) = other.scan_detail.as_ref() {
            if let Some(current) = self.scan_detail.as_mut() {
                current.Merge(scan);
            } else {
                self.scan_detail = Some(scan.clone());
            }
        }
        if !other.description.is_empty() {
            if !self.description.is_empty() {
                self.description.push_str(", ");
            }
            self.description.push_str(&other.description);
        }
    }
}

/// 可共享的 Snapshot 运行时统计包装。
pub struct runtimeStatsWithSnapshot {
    pub snapshot_runtime_stats: Option<Arc<Mutex<SnapshotRuntimeStats>>>,
}

impl runtimeStatsWithSnapshot {
    /// 返回统计描述字符串。
    pub fn String(&self) -> String {
        self.snapshot_runtime_stats
            .as_ref()
            .and_then(|stats| {
                stats.lock().ok().map(|stats| {
                    let mut text = stats.description.clone();
                    if let Some(pool) = stats
                        .read_pool_task_details
                        .as_ref()
                        .filter(|pool| !pool.Empty())
                    {
                        if !text.is_empty() {
                            text.push_str(", ");
                        }
                        text.push_str(&format!("read_pool:{}", pool.String()));
                    }
                    text
                })
            })
            .unwrap_or_default()
    }

    /// 深拷贝统计快照。
    pub fn Clone(&self) -> Self {
        let snapshot_runtime_stats = self.snapshot_runtime_stats.as_ref().and_then(|stats| {
            stats
                .lock()
                .ok()
                .map(|stats| Arc::new(Mutex::new(stats.clone())))
        });
        Self {
            snapshot_runtime_stats,
        }
    }

    /// 合并另一包装中的统计。
    pub fn Merge(&mut self, other: &runtimeStatsWithSnapshot) {
        let Some(other_stats) = &other.snapshot_runtime_stats else {
            return;
        };
        let Ok(other_stats) = other_stats.lock() else {
            return;
        };
        match &self.snapshot_runtime_stats {
            Some(stats) => {
                if let Ok(mut stats) = stats.lock() {
                    stats.merge(&other_stats);
                }
            }
            None => {
                self.snapshot_runtime_stats = Some(Arc::new(Mutex::new(other_stats.clone())));
            }
        }
    }

    /// 返回统计类型标签。
    pub fn Tp(&self) -> i32 {
        TP_RUNTIME_STATS_WITH_SNAPSHOT
    }
}

impl astersql_util_execdetails::execdetails::RuntimeStats for runtimeStatsWithSnapshot {
    fn String(&self) -> String {
        runtimeStatsWithSnapshot::String(self)
    }
    fn Merge(&mut self, other: &dyn astersql_util_execdetails::execdetails::RuntimeStats) {
        if let Some(other) = other.as_any().downcast_ref::<Self>() {
            runtimeStatsWithSnapshot::Merge(self, other);
        }
    }
    fn CloneBox(&self) -> Box<dyn astersql_util_execdetails::execdetails::RuntimeStats> {
        Box::new(self.Clone())
    }
    fn Tp(&self) -> i32 {
        runtimeStatsWithSnapshot::Tp(self)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// KV 快照读接口（含副本读调节与统计收集）。
pub trait Snapshot: Send {
    fn get(&self, key: &[u8], timeout: Option<Duration>) -> PointGetResult<Vec<u8>>;
    fn set_replica_read_adjuster(&mut self, average_row_size: u64) -> PointGetResult;
    fn set_collect_runtime_stats(
        &mut self,
        stats: Option<Arc<Mutex<SnapshotRuntimeStats>>>,
    ) -> PointGetResult;
    fn initialize_with_session(&mut self) -> PointGetResult;
}

/// 事务接口：有效性、只读标记与内存缓冲读。
pub trait Transaction: Send {
    fn valid(&self) -> bool;
    fn is_read_only(&self) -> bool;
    fn mem_buffer_get(&self, key: &[u8]) -> PointGetResult<Vec<u8>>;
}

/// 新行格式解码到 Chunk。
pub trait RowDecoder: Send {
    fn decode_to_chunk(
        &mut self,
        row_value: &[u8],
        row_index: usize,
        handle: &Handle,
        chunk: &mut Chunk,
    ) -> PointGetResult;
}

/// 单列编码解码到 Chunk。
pub trait DatumDecoder: Send {
    fn decode_one(
        &mut self,
        encoded: &[u8],
        column_index: usize,
        field_type: &FieldType,
        chunk: &mut Chunk,
    ) -> PointGetResult;
}

/// 索引/主键访问用量上报。
pub trait IndexUsageReporter: Send {
    fn report_index(
        &mut self,
        table_id: i64,
        physical_table_id: i64,
        index_id: i64,
        kv_request_count: u64,
        rows: u64,
    );
    fn report_handle(
        &mut self,
        table: &TableInfo,
        physical_table_id: i64,
        kv_request_count: u64,
        rows: u64,
    );
}

/// Point Get 依赖注入边界：编码、加锁、缓存、解码、placement 校验等。
pub trait PointGetDependencies: Send + Sync {
    fn transaction(&self, active: bool) -> PointGetResult<Box<dyn Transaction>>;
    fn build_index_usage_reporter(
        &self,
        plan: &PointGetPlan,
    ) -> Option<Box<dyn IndexUsageReporter>>;
    fn new_row_decoder(
        &self,
        schema: &Schema,
        table: &TableInfo,
    ) -> PointGetResult<Box<dyn RowDecoder>>;
    fn new_datum_decoder(&self) -> PointGetResult<Box<dyn DatumDecoder>>;
    fn build_virtual_column_index(
        &self,
        schema: &Schema,
        columns: &[ColumnInfo],
    ) -> PointGetResult<Vec<usize>>;
    fn is_replica_read_closest_adaptive(&self) -> PointGetResult<bool>;
    fn assert_point_replica_scope(&self, read_replica_scope: &str) -> PointGetResult;
    fn runtime_stats_enabled(&self) -> bool;
    fn append_index_name(&self, name: String) -> PointGetResult;
    fn set_option_for_top_sql(&self, snapshot: &mut dyn Snapshot) -> PointGetResult;
    fn register_runtime_stats(
        &self,
        executor_id: i64,
        stats: &runtimeStatsWithSnapshot,
    ) -> PointGetResult;
    fn merge_tikv_cpu_time(&self, process_time_micros: u64) -> PointGetResult;
    /// Merge point-read scan diagnostics without increasing cop-task counts.
    fn merge_scan_detail(
        &self,
        _scan_detail: Option<&astersql_util_execdetails::execdetails::util::ScanDetail>,
    ) -> PointGetResult {
        Ok(())
    }
    fn actual_rows(&self, executor_id: i64) -> PointGetResult<u64>;
    fn update_delta_for_table_id(&self, table_id: i64) -> PointGetResult;
    fn encode_unique_index_values_for_key(
        &self,
        table: &TableInfo,
        index: &IndexInfo,
        values: &[Datum],
    ) -> PointGetResult<Vec<u8>>;
    fn new_common_handle(&self, encoded: &[u8]) -> PointGetResult<Handle>;
    fn encode_unique_index_key(
        &self,
        table: &TableInfo,
        index: &IndexInfo,
        values: &[Datum],
        physical_table_id: i64,
    ) -> PointGetResult<Key>;
    fn decode_handle_in_index_value(&self, value: &[u8]) -> PointGetResult<Handle>;
    fn decode_global_index_partition_id(&self, value: &[u8]) -> PointGetResult<i64>;
    fn repeatable_read_point_get_barrier(&self) -> PointGetResult;
    fn encode_row_key_with_handle(
        &self,
        physical_table_id: i64,
        handle: &Handle,
    ) -> PointGetResult<Key>;
    fn pessimistic_read_consistency(&self) -> PointGetResult<bool>;
    fn check_max_execution_time_exceeded(&self) -> PointGetResult;
    fn new_lock_context(
        &self,
        lock_wait_time: i64,
        key_count: usize,
    ) -> PointGetResult<LockContext>;
    fn do_lock_keys(&self, context: &mut LockContext, keys: &[Key]) -> PointGetResult;
    fn set_pessimistic_lock_cache(&self, key: Key, value: Vec<u8>) -> PointGetResult;
    fn pessimistic_lock_cache_get(&self, key: &[u8]) -> PointGetResult<Option<Vec<u8>>>;
    fn table_cache_union_get(
        &self,
        table_id: i64,
        snapshot: &dyn Snapshot,
        key: &[u8],
    ) -> PointGetResult<Vec<u8>>;
    fn point_get_cache_enabled(&self) -> PointGetResult<bool>;
    fn maximum_execution_time_millis(&self) -> PointGetResult<u64>;
    fn weak_consistency(&self) -> PointGetResult<bool>;
    fn report_lookup_inconsistent(
        &self,
        table: &TableInfo,
        index: &IndexInfo,
        handle: &Handle,
        row_key: &[u8],
        index_key: &[u8],
    ) -> PointGetResult;
    fn is_new_row_format(&self, row_value: &[u8]) -> bool;
    fn decode_row_with_map_new(
        &self,
        row_value: &[u8],
        table: &TableInfo,
    ) -> PointGetResult<BTreeMap<i64, Datum>>;
    fn decode_handle_to_datum_map(
        &self,
        handle: &Handle,
        handle_column_ids: &[i64],
        table: &TableInfo,
        datums: BTreeMap<i64, Datum>,
    ) -> PointGetResult<BTreeMap<i64, Datum>>;
    fn origin_default_value(&self, column: &ColumnInfo) -> PointGetResult<Datum>;
    fn row_checksum(&self, columns: &[ColumnData], buffer: &[u8]) -> PointGetResult<u32>;
    fn fill_virtual_columns(
        &self,
        return_types: &[FieldType],
        indices: &[usize],
        schema: &Schema,
        columns: &[ColumnInfo],
        chunk: &mut Chunk,
    ) -> PointGetResult;
    fn try_get_common_pk_column_ids(&self, table: &TableInfo) -> PointGetResult<Vec<i64>>;
    fn primary_prefix_column_ids(&self, table: &TableInfo) -> PointGetResult<Vec<i64>>;
    fn cut_old_row(
        &self,
        row_value: &[u8],
        column_positions: &BTreeMap<i64, usize>,
    ) -> PointGetResult<Option<Vec<Vec<u8>>>>;
    fn table_by_id(&self, table_id: i64) -> PointGetResult<TableInfo>;
    fn verify_transaction_scope(&self, scope: &str, physical_id: i64) -> PointGetResult<bool>;
}

/// 构建 Point Get 执行器时的会话/计划辅助接口。
pub trait PointGetBuilder: Send {
    fn validate_readable_table(&mut self, table: &TableInfo) -> PointGetResult;
    fn prune_partitions(&mut self, plan: &PointGetPlan) -> PointGetResult<bool>;
    fn set_error(&mut self, error: PointGetError);
    fn in_select_lock_statement(&self) -> bool;
    fn set_in_select_lock_statement(&mut self, value: bool);
    fn mark_statement_as_tikv(&mut self);
    fn build_index_usage_reporter(
        &mut self,
        plan: &PointGetPlan,
    ) -> Option<Box<dyn IndexUsageReporter>>;
    fn transaction_scope(&self) -> String;
    fn read_replica_scope(&self) -> String;
    fn is_staleness(&self) -> bool;
    fn get_snapshot(&mut self) -> PointGetResult<Box<dyn Snapshot>>;
    fn get_snapshot_timestamp(&mut self) -> PointGetResult<u64>;
    fn wrap_cache_table_snapshot(
        &mut self,
        snapshot: &mut Box<dyn Snapshot>,
        table: &TableInfo,
        snapshot_timestamp: u64,
    ) -> PointGetResult;
    fn mark_has_lock(&mut self);
    fn dependencies(&self) -> Arc<dyn PointGetDependencies>;
}

/// buildPointGet 的三种结果：空表双（TableDual）、执行器或失败。
pub enum BuiltPointGet {
    TableDual,
    Executor(Box<PointGetExecutor>),
    Failed,
}

/// 校验可读性与分区裁剪后构建执行器；临时设置 SELECT 锁语句标志。
pub fn buildPointGet(builder: &mut dyn PointGetBuilder, plan: &PointGetPlan) -> BuiltPointGet {
    if let Err(error) = builder.validate_readable_table(&plan.table) {
        builder.set_error(error);
        return BuiltPointGet::Failed;
    }
    match builder.prune_partitions(plan) {
        Ok(true) => return BuiltPointGet::TableDual,
        Ok(false) => {}
        Err(error) => {
            builder.set_error(error);
            return BuiltPointGet::Failed;
        }
    }

    // 若本语句首次进入加锁 Point Get，临时标记 in_select_lock 并在返回前还原。
    let reset_select_lock = plan.lock && !builder.in_select_lock_statement();
    if reset_select_lock {
        builder.set_in_select_lock_statement(true);
    }
    let result = build_point_get_inner(builder, plan);
    if reset_select_lock {
        builder.set_in_select_lock_statement(false);
    }
    result
}

/// 内部构建：取 Snapshot、Init 执行器、可选包装缓存表快照、标记锁。
fn build_point_get_inner(builder: &mut dyn PointGetBuilder, plan: &PointGetPlan) -> BuiltPointGet {
    builder.mark_statement_as_tikv();
    let snapshot = match builder.get_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            builder.set_error(error);
            return BuiltPointGet::Failed;
        }
    };
    let dependencies = builder.dependencies();
    let mut executor = PointGetExecutor {
        id: plan.id,
        index_usage_reporter: builder.build_index_usage_reporter(plan),
        table_info: plan.table.clone(),
        handle: plan.handle.clone(),
        index_info: plan.index.clone(),
        partition_definition_index: plan.partition_index,
        partition_names: plan.partition_names.clone(),
        index_key: Vec::new(),
        handle_value: Vec::new(),
        index_values: plan.index_values.clone(),
        transaction_scope: builder.transaction_scope(),
        read_replica_scope: builder.read_replica_scope(),
        is_staleness: builder.is_staleness(),
        transaction: None,
        snapshot,
        done: false,
        lock: false,
        lock_wait_time: 0,
        row_decoder: None,
        schema: plan.schema.clone(),
        columns: plan.columns.clone(),
        virtual_column_index: Vec::new(),
        virtual_column_return_field_types: Vec::new(),
        stats: None,
        dependencies,
    };
    if let Err(error) = executor.Init(plan) {
        builder.set_error(error);
        return BuiltPointGet::Failed;
    }
    let snapshot_timestamp = match builder.get_snapshot_timestamp() {
        Ok(timestamp) => timestamp,
        Err(error) => {
            builder.set_error(error);
            return BuiltPointGet::Failed;
        }
    };
    if plan.table.cache_enabled {
        if let Err(error) = builder.wrap_cache_table_snapshot(
            &mut executor.snapshot,
            &plan.table,
            snapshot_timestamp,
        ) {
            builder.set_error(error);
            return BuiltPointGet::Failed;
        }
    }
    if executor.lock {
        builder.mark_has_lock();
    }
    BuiltPointGet::Executor(Box::new(executor))
}

/// Point Get 执行器运行时状态。
pub struct PointGetExecutor {
    pub id: i64,
    pub index_usage_reporter: Option<Box<dyn IndexUsageReporter>>,
    pub table_info: TableInfo,
    pub handle: Handle,
    pub index_info: Option<IndexInfo>,
    pub partition_definition_index: Option<usize>,
    pub partition_names: Vec<String>,
    pub index_key: Key,
    pub handle_value: Vec<u8>,
    pub index_values: Vec<Datum>,
    pub transaction_scope: String,
    pub read_replica_scope: String,
    pub is_staleness: bool,
    pub transaction: Option<Box<dyn Transaction>>,
    pub snapshot: Box<dyn Snapshot>,
    pub done: bool,
    pub lock: bool,
    pub lock_wait_time: i64,
    pub row_decoder: Option<Box<dyn RowDecoder>>,
    pub schema: Schema,
    pub columns: Vec<ColumnInfo>,
    pub virtual_column_index: Vec<usize>,
    pub virtual_column_return_field_types: Vec<FieldType>,
    pub stats: Option<runtimeStatsWithSnapshot>,
    pub dependencies: Arc<dyn PointGetDependencies>,
}

/// 解析物理表/分区 ID：有分区索引则取分区定义 ID，否则逻辑表 ID。
pub fn GetPhysID(table_info: &TableInfo, index: Option<usize>) -> i64 {
    if let Some(index) = index {
        if let Some(partition) = table_info.partition.as_ref() {
            return partition
                .definitions
                .get(index)
                .expect("partition index is out of range")
                .id;
        }
    }
    table_info.id
}

/// 分区名过滤：空列表表示不过滤；否则按 ASCII 大小写不敏感匹配。
pub fn matchPartitionNames(
    partition_id: i64,
    partition_names: &[String],
    partition_info: &PartitionInfo,
) -> bool {
    if partition_names.is_empty() {
        return true;
    }
    for definition in &partition_info.definitions {
        if definition.id == partition_id {
            return partition_names
                .iter()
                .any(|name| definition.name.eq_ignore_ascii_case(name));
        }
    }
    false
}

impl PointGetExecutor {
    /// 用新计划与依赖重建执行器状态并重新初始化 Snapshot 会话选项。
    pub fn Recreated(
        &mut self,
        plan: &PointGetPlan,
        dependencies: Arc<dyn PointGetDependencies>,
    ) -> PointGetResult {
        self.id = plan.id;
        self.dependencies = dependencies;
        self.index_usage_reporter = self.dependencies.build_index_usage_reporter(plan);
        self.Init(plan)?;
        self.snapshot.initialize_with_session()
    }

    /// 按计划重置解码器、锁选项、分区与虚拟列，并配置副本读/统计。
    pub fn Init(&mut self, plan: &PointGetPlan) -> PointGetResult {
        self.row_decoder = Some(
            self.dependencies
                .new_row_decoder(&plan.schema, &plan.table)?,
        );
        self.table_info = plan.table.clone();
        self.handle = plan.handle.clone();
        self.index_info = plan.index.clone();
        self.index_values = plan.index_values.clone();
        self.done = false;
        // 临时表不走悲观锁。
        if !self.table_info.temporary {
            self.lock = plan.lock;
            self.lock_wait_time = plan.lock_wait_time;
        } else {
            self.lock = false;
            self.lock_wait_time = 0;
        }
        self.partition_definition_index = plan.partition_index;
        self.partition_names = plan.partition_names.clone();
        self.schema = plan.schema.clone();
        self.columns = plan.columns.clone();
        self.buildVirtualColumnInfo()?;

        if self.dependencies.is_replica_read_closest_adaptive()? {
            self.snapshot
                .set_replica_read_adjuster(plan.average_row_size)?;
        }
        self.dependencies
            .assert_point_replica_scope(&self.read_replica_scope)?;
        if self.dependencies.runtime_stats_enabled() {
            let snapshot_stats = Arc::new(Mutex::new(SnapshotRuntimeStats::default()));
            self.stats = Some(runtimeStatsWithSnapshot {
                snapshot_runtime_stats: Some(Arc::clone(&snapshot_stats)),
            });
            self.snapshot
                .set_collect_runtime_stats(Some(snapshot_stats))?;
        } else {
            self.stats = None;
        }
        if let Some(index) = &plan.index {
            self.dependencies
                .append_index_name(format!("{}:{}", plan.table.name, index.name))?;
        }
        Ok(())
    }

    /// 收集需填充的虚拟列下标及其返回类型。
    pub fn buildVirtualColumnInfo(&mut self) -> PointGetResult {
        self.virtual_column_index = self
            .dependencies
            .build_virtual_column_index(&self.schema, &self.columns)?;
        self.virtual_column_index.sort_unstable();
        self.virtual_column_return_field_types = self
            .virtual_column_index
            .iter()
            .map(|index| {
                self.schema
                    .columns
                    .get(*index)
                    .map(|column| column.return_type.clone())
                    .ok_or_else(|| PointGetError::new("virtual column index is out of range"))
            })
            .collect::<PointGetResult<Vec<_>>>()?;
        Ok(())
    }

    /// 打开：获取事务、校验 txn_scope，并为 Snapshot 设置 TopSQL 选项。
    pub fn Open(&mut self) -> PointGetResult {
        self.transaction = Some(self.dependencies.transaction(false)?);
        self.verifyTxnScope()?;
        self.dependencies
            .set_option_for_top_sql(self.snapshot.as_mut())
    }

    /// 关闭：上报索引用量与运行时统计，合并 TiKV CPU 时间。
    pub fn Close(&mut self) -> PointGetResult {
        if self.dependencies.runtime_stats_enabled() {
            self.snapshot.set_collect_runtime_stats(None)?;
        }
        if let Some(reporter) = &mut self.index_usage_reporter {
            let physical_table_id = GetPhysID(&self.table_info, self.partition_definition_index);
            let stats = self.stats.as_ref().ok_or_else(|| {
                PointGetError::new("index usage reporter requires snapshot runtime stats")
            })?;
            let request_count = stats
                .snapshot_runtime_stats
                .as_ref()
                .ok_or_else(|| PointGetError::new("snapshot runtime stats are missing"))?
                .lock()
                .map_err(|_| PointGetError::new("snapshot runtime stats lock is poisoned"))?
                .command_get_rpc_count;
            let rows = self.dependencies.actual_rows(self.id)?;
            if let Some(index) = &self.index_info {
                reporter.report_index(
                    self.table_info.id,
                    physical_table_id,
                    index.id,
                    request_count,
                    rows,
                );
            } else {
                reporter.report_handle(&self.table_info, physical_table_id, request_count, rows);
            }
        }
        self.done = false;
        if let Some(stats) = &self.stats {
            self.dependencies.register_runtime_stats(self.id, stats)?;
            if let Some(snapshot_stats) = &stats.snapshot_runtime_stats {
                let snapshot_stats = snapshot_stats
                    .lock()
                    .map_err(|_| PointGetError::new("snapshot runtime stats lock is poisoned"))?;
                self.dependencies
                    .merge_scan_detail(snapshot_stats.scan_detail.as_ref())?;
                self.dependencies
                    .merge_tikv_cpu_time(snapshot_stats.process_time_micros)?;
            }
        }
        Ok(())
    }

    /// 单次产出：索引/主键定位 → 加锁与取值 → 解码行 → 校验和与虚拟列。
    pub fn Next(&mut self, request: &mut Chunk) -> PointGetResult {
        request.reset();
        if self.done {
            return Ok(());
        }
        self.done = true;

        // 物理表 ID；加锁路径会更新表级 delta 统计。
        let mut table_id = GetPhysID(&self.table_info, self.partition_definition_index);
        if self.lock {
            self.dependencies.update_delta_for_table_id(table_id)?;
        }
        // 唯一索引路径：公共 handle 直接编码；否则先读索引拿到 handle。
        if let Some(index) = self.index_info.clone() {
            if index.common_handle_read {
                let encoded = match self.dependencies.encode_unique_index_values_for_key(
                    &self.table_info,
                    &index,
                    &self.index_values,
                ) {
                    Ok(encoded) => encoded,
                    Err(error) if error.is_not_found() => return Ok(()),
                    Err(error) => return Err(error),
                };
                self.handle = self.dependencies.new_common_handle(&encoded)?;
            } else {
                self.index_key = match self.dependencies.encode_unique_index_key(
                    &self.table_info,
                    &index,
                    &self.index_values,
                    table_id,
                ) {
                    Ok(key) => key,
                    Err(error) if error.is_not_found() => Vec::new(),
                    Err(error) => return Err(error),
                };

                // 非悲观读一致时：即使索引键不存在也可能需要加锁。
                let lock_nonexistent_index_key =
                    !self.dependencies.pessimistic_read_consistency()?;
                if lock_nonexistent_index_key {
                    self.lockKeyIfNeeded(&self.index_key.clone())?;
                    self.handle_value = match self.get(&self.index_key) {
                        Ok(value) => value,
                        Err(error) if error.is_not_found() => Vec::new(),
                        Err(error) => return Err(error),
                    };
                } else if self.lock {
                    self.handle_value = self
                        .lockKeyIfExists(&self.index_key.clone())?
                        .unwrap_or_default();
                } else {
                    self.handle_value = match self.get(&self.index_key) {
                        Ok(value) => value,
                        Err(error) if error.is_not_found() => Vec::new(),
                        Err(error) => return Err(error),
                    };
                }
                if self.handle_value.is_empty() {
                    return Ok(());
                }
                self.handle = self
                    .dependencies
                    .decode_handle_in_index_value(&self.handle_value)?;
                point_get_repeatable_read_failpoint();
                self.dependencies.repeatable_read_point_get_barrier()?;
                // 全局索引：从索引值解码分区 ID，并做分区名/DDL 忽略过滤。
                if index.global {
                    let partition_id = self
                        .dependencies
                        .decode_global_index_partition_id(&self.handle_value)?;
                    table_id = partition_id;
                    let partition = self.table_info.partition.as_ref().ok_or_else(|| {
                        PointGetError::new("global index requires partition metadata")
                    })?;
                    if !matchPartitionNames(table_id, &self.partition_names, partition) {
                        return Ok(());
                    }
                    if partition.ids_in_ddl_to_ignore.contains(&partition_id) {
                        return Ok(());
                    }
                }
            }
        }

        // 编码行键并 getAndLock；若索引命中但行缺失且非弱一致，则上报不一致。
        let row_key = self
            .dependencies
            .encode_row_key_with_handle(table_id, &self.handle)?;
        let row_value = self.getAndLock(&row_key)?.unwrap_or_default();
        if row_value.is_empty() {
            if let Some(index) = &self.index_info {
                if !index.common_handle_read && !self.dependencies.weak_consistency()? {
                    self.dependencies.report_lookup_inconsistent(
                        &self.table_info,
                        index,
                        &self.handle,
                        &row_key,
                        &self.index_key,
                    )?;
                }
            }
            return Ok(());
        }

        let decoder = self
            .row_decoder
            .as_mut()
            .ok_or_else(|| PointGetError::new("row decoder is not initialized"))?;
        DecodeRowValToChunk(
            self.dependencies.as_ref(),
            &self.schema,
            &self.table_info,
            &self.handle,
            &row_value,
            request,
            decoder.as_mut(),
        )?;
        fillRowChecksum(
            self.dependencies.as_ref(),
            0,
            1,
            &self.schema,
            &self.table_info,
            std::slice::from_ref(&row_value),
            std::slice::from_ref(&self.handle),
            request,
            &[],
        )?;
        self.dependencies.fill_virtual_columns(
            &self.virtual_column_return_field_types,
            &self.virtual_column_index,
            &self.schema,
            &self.columns,
            request,
        )
    }

    /// 按悲观读一致策略选择：存在才锁 / 先锁再读 / 仅读。
    pub fn getAndLock(&mut self, key: &[u8]) -> PointGetResult<Option<Vec<u8>>> {
        if self.dependencies.pessimistic_read_consistency()? {
            if self.lock {
                return self.lockKeyIfExists(key);
            }
            return match self.get(key) {
                Ok(value) => Ok(Some(value)),
                Err(error) if error.is_not_found() => Ok(None),
                Err(error) => Err(error),
            };
        }
        self.lockKeyIfNeeded(key)?;
        match self.get(key) {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// 需要时对 key 加锁（不要求键已存在）。
    pub fn lockKeyIfNeeded(&mut self, key: &[u8]) -> PointGetResult {
        self.lockKeyBase(key, false).map(|_| ())
    }

    /// 仅当键存在时加锁，并返回锁上下文中的值。
    pub fn lockKeyIfExists(&mut self, key: &[u8]) -> PointGetResult<Option<Vec<u8>>> {
        self.lockKeyBase(key, true)
    }

    /// 加锁核心：构造 LockContext、do_lock_keys，并回填悲观锁缓存。
    pub fn lockKeyBase(
        &mut self,
        key: &[u8],
        lock_only_if_exists: bool,
    ) -> PointGetResult<Option<Vec<u8>>> {
        if key.is_empty() {
            return Ok(None);
        }
        if self.lock {
            self.dependencies.check_max_execution_time_exceeded()?;
            let mut lock_context = self.dependencies.new_lock_context(self.lock_wait_time, 1)?;
            lock_context.lock_only_if_exists = lock_only_if_exists;
            self.dependencies
                .do_lock_keys(&mut lock_context, &[key.to_vec()])?;
            for (not_locked_key, value) in &lock_context.values_not_locked {
                self.dependencies
                    .set_pessimistic_lock_cache(not_locked_key.clone(), value.clone())?;
            }
            if !self.handle_value.is_empty() {
                self.dependencies.set_pessimistic_lock_cache(
                    self.index_key.clone(),
                    self.handle_value.clone(),
                )?;
            }
            if lock_only_if_exists {
                return self.getValueFromLockCtx(&lock_context, key);
            }
        }
        Ok(None)
    }

    /// 从加锁结果中取回值：存在则直接返回；已锁则再 get。
    pub fn getValueFromLockCtx(
        &self,
        lock_context: &LockContext,
        key: &[u8],
    ) -> PointGetResult<Option<Vec<u8>>> {
        if let Some(value) = lock_context.values.get(key) {
            if value.exists {
                return Ok(Some(value.value.clone()));
            }
            if value.already_locked {
                return match self.get(key) {
                    Ok(value) => Ok(Some(value)),
                    Err(error) if error.is_not_found() => Ok(None),
                    Err(error) => Err(error),
                };
            }
        }
        Ok(None)
    }

    /// 读路径：先事务 mem buffer / 悲观锁缓存，再表缓存或 Snapshot.get。
    pub fn get(&self, key: &[u8]) -> PointGetResult<Vec<u8>> {
        if key.is_empty() {
            return Err(PointGetError::not_found());
        }
        // 活跃非只读事务：优先读本地写缓冲。
        if let Some(transaction) = &self.transaction {
            if transaction.valid() && !transaction.is_read_only() {
                match transaction.mem_buffer_get(key) {
                    Ok(value) => return Ok(value),
                    Err(error) if error.is_not_found() => {}
                    Err(error) => return Err(error),
                }
                if self.lock {
                    if let Some(value) = self.dependencies.pessimistic_lock_cache_get(key)? {
                        return Ok(value);
                    }
                }
            }
        }

        // 读锁表且开启 point get cache 时走表缓存联合读。
        if matches!(
            self.table_info.table_lock,
            Some(TableLockType::Read | TableLockType::ReadOnly)
        ) && self.dependencies.point_get_cache_enabled()?
        {
            return self.dependencies.table_cache_union_get(
                self.table_info.id,
                self.snapshot.as_ref(),
                key,
            );
        }
        let timeout_millis = self.dependencies.maximum_execution_time_millis()?;
        let timeout = (timeout_millis > 0).then(|| Duration::from_millis(timeout_millis));
        self.snapshot.get(key, timeout)
    }

    /// 校验当前事务作用域是否允许读取该表/分区（Placement）。
    pub fn verifyTxnScope(&self) -> PointGetResult {
        if self.transaction_scope.is_empty() || self.transaction_scope == GLOBAL_TXN_SCOPE {
            return Ok(());
        }
        let table = self.dependencies.table_by_id(self.table_info.id)?;
        let physical_table_id = GetPhysID(&table, self.partition_definition_index);
        let partition_name = if physical_table_id != table.id {
            let index = self
                .partition_definition_index
                .ok_or_else(|| PointGetError::new("physical partition requires partition index"))?;
            Some(
                table
                    .partition
                    .as_ref()
                    .and_then(|partition| partition.definitions.get(index))
                    .ok_or_else(|| PointGetError::new("partition metadata is missing"))?
                    .name
                    .clone(),
            )
        } else {
            None
        };
        if self
            .dependencies
            .verify_transaction_scope(&self.transaction_scope, physical_table_id)?
        {
            return Ok(());
        }
        let message = match partition_name {
            Some(partition) => format!(
                "table {}'s partition {} can not be read by {} txn_scope",
                table.name, partition, self.transaction_scope
            ),
            None => format!(
                "table {} can not be read by {} txn_scope",
                table.name, self.transaction_scope
            ),
        };
        Err(PointGetError::with_kind(
            PointGetErrorKind::InvalidPlacementPolicy,
            message,
        ))
    }
}

/// Schema 是否包含行校验和列；返回列下标与是否需要填充。
pub fn shouldFillRowChecksum(schema: &Schema) -> (usize, bool) {
    for (index, column) in schema.columns.iter().enumerate() {
        if column.id == EXTRA_ROW_CHECKSUM_ID {
            return (index, true);
        }
    }
    (0, false)
}

#[allow(clippy::too_many_arguments)]
/// 为结果 Chunk 填充行校验和列（仅新行格式；旧格式填 Null）。
pub fn fillRowChecksum(
    dependencies: &dyn PointGetDependencies,
    start: usize,
    end: usize,
    schema: &Schema,
    table_info: &TableInfo,
    values: &[Vec<u8>],
    handles: &[Handle],
    request: &mut Chunk,
    buffer: &[u8],
) -> PointGetResult {
    let (checksum_column_index, should_fill) = shouldFillRowChecksum(schema);
    if !should_fill {
        return Ok(());
    }
    if start > end || end > values.len() || end > handles.len() {
        return Err(PointGetError::new("row checksum range is out of bounds"));
    }

    // 收集参与校验和的 handle 列 ID（整型 PK / common handle / 无）。
    let handle_column_ids = if table_info.pk_is_handle {
        let primary = table_info
            .columns
            .iter()
            .find(|column| column.field_type.primary_key)
            .ok_or_else(|| PointGetError::new("PK handle column is missing"))?;
        vec![primary.id]
    } else if table_info.is_common_handle {
        let primary_index = table_info
            .primary_index
            .as_ref()
            .ok_or_else(|| PointGetError::new("common handle primary index is missing"))?;
        primary_index
            .columns
            .iter()
            .map(|index_column| {
                table_info
                    .columns
                    .get(index_column.offset)
                    .map(|column| column.id)
                    .ok_or_else(|| PointGetError::new("primary index column is out of range"))
            })
            .collect::<PointGetResult<Vec<_>>>()?
    } else {
        Vec::new()
    };

    let mut checksum_values = Vec::with_capacity(end - start);
    for index in start..end {
        let value = &values[index];
        if !dependencies.is_new_row_format(value) {
            checksum_values.push(Datum::Null);
            continue;
        }
        let datums = dependencies.decode_row_with_map_new(value, table_info)?;
        let mut datums = dependencies.decode_handle_to_datum_map(
            &handles[index],
            &handle_column_ids,
            table_info,
            datums,
        )?;
        for column in &table_info.columns {
            if !datums.contains_key(&column.id) {
                datums.insert(column.id, dependencies.origin_default_value(column)?);
            }
        }
        let mut column_data = table_info
            .columns
            .iter()
            .map(|column| {
                datums
                    .get(&column.id)
                    .cloned()
                    .map(|datum| ColumnData {
                        column: column.clone(),
                        datum,
                    })
                    .ok_or_else(|| PointGetError::new("checksum datum is missing"))
            })
            .collect::<PointGetResult<Vec<_>>>()?;
        column_data.sort_by_key(|data| data.column.id);
        let checksum = dependencies.row_checksum(&column_data, buffer)?;
        checksum_values.push(Datum::String(u64::from(checksum).to_string()));
    }
    request.set_column(checksum_column_index, checksum_values)
}

/// 将行值解码进 Chunk：新格式走 RowDecoder，旧格式走 decodeOldRowValToChunk。
pub fn DecodeRowValToChunk(
    dependencies: &dyn PointGetDependencies,
    schema: &Schema,
    table_info: &TableInfo,
    handle: &Handle,
    row_value: &[u8],
    chunk: &mut Chunk,
    row_decoder: &mut dyn RowDecoder,
) -> PointGetResult {
    if dependencies.is_new_row_format(row_value) {
        return row_decoder.decode_to_chunk(row_value, 0, handle, chunk);
    }
    decodeOldRowValToChunk(dependencies, schema, table_info, handle, row_value, chunk)
}

/// 旧行格式：按列切分编码，必要时从 handle 或默认值补齐。
pub fn decodeOldRowValToChunk(
    dependencies: &dyn PointGetDependencies,
    schema: &Schema,
    table_info: &TableInfo,
    handle: &Handle,
    row_value: &[u8],
    chunk: &mut Chunk,
) -> PointGetResult {
    let primary_key_columns = dependencies.try_get_common_pk_column_ids(table_info)?;
    let prefix_column_ids = dependencies.primary_prefix_column_ids(table_info)?;
    let mut column_positions = BTreeMap::new();
    for column in &schema.columns {
        let next_position = column_positions.len();
        column_positions.entry(column.id).or_insert(next_position);
    }
    let mut cut_values = dependencies
        .cut_old_row(row_value, &column_positions)?
        .unwrap_or_else(|| vec![Vec::new(); column_positions.len()]);
    if cut_values.len() < column_positions.len() {
        cut_values.resize(column_positions.len(), Vec::new());
    }
    let mut decoder = dependencies.new_datum_decoder()?;
    for (schema_index, column) in schema.columns.iter().enumerate() {
        if column.virtual_expression.is_some() {
            chunk.append(schema_index, Datum::Null)?;
            continue;
        }
        if tryDecodeFromHandle(
            table_info,
            schema_index,
            column,
            handle,
            chunk,
            decoder.as_mut(),
            &primary_key_columns,
            &prefix_column_ids,
        )? {
            continue;
        }
        let cut_position = *column_positions
            .get(&column.id)
            .ok_or_else(|| PointGetError::new("old row column position is missing"))?;
        let encoded = &cut_values[cut_position];
        if encoded.is_empty() {
            let column_info = getColInfoByID(table_info, column.id).ok_or_else(|| {
                PointGetError::new(format!("column metadata is missing for {}", column.id))
            })?;
            chunk.append(
                schema_index,
                dependencies.origin_default_value(column_info)?,
            )?;
            continue;
        }
        decoder.decode_one(encoded, schema_index, &column.return_type, chunk)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
/// 尝试从 handle 直接填充主键/额外 handle 列；成功返回 true。
pub fn tryDecodeFromHandle(
    table_info: &TableInfo,
    schema_column_index: usize,
    column: &SchemaColumn,
    handle: &Handle,
    chunk: &mut Chunk,
    decoder: &mut dyn DatumDecoder,
    primary_key_columns: &[i64],
    prefix_column_ids: &[i64],
) -> PointGetResult<bool> {
    if table_info.pk_is_handle && column.return_type.primary_key {
        chunk.append(schema_column_index, Datum::Int64(handle.int_value()?))?;
        return Ok(true);
    }
    if column.id == EXTRA_HANDLE_ID {
        chunk.append(schema_column_index, Datum::Int64(handle.int_value()?))?;
        return Ok(true);
    }
    if column.return_type.needs_restored_data {
        return Ok(false);
    }
    if column.return_type.primary_key {
        for (index, handle_column_id) in primary_key_columns.iter().enumerate() {
            if column.id == *handle_column_id
                && notPKPrefixCol(*handle_column_id, prefix_column_ids)
            {
                decoder.decode_one(
                    handle.encoded_column(index)?,
                    schema_column_index,
                    &column.return_type,
                    chunk,
                )?;
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// 列是否不在主键前缀列集合中（前缀列不能仅靠 handle 还原完整值）。
pub fn notPKPrefixCol(column_id: i64, prefix_column_ids: &[i64]) -> bool {
    !prefix_column_ids.contains(&column_id)
}

/// 按列 ID 查找表列元数据。
pub fn getColInfoByID(table: &TableInfo, column_id: i64) -> Option<&ColumnInfo> {
    table.columns.iter().find(|column| column.id == column_id)
}
