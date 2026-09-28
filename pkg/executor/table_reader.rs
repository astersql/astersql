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

// TableReader 执行器：按 KV 范围从 TiKV/TiFlash 拉取表扫描结果。
//
// TableReader 对应物理计划中的 TableFullScan / TableRangeScan：把逻辑
// 范围编成 DistSQL/Coprocessor DAG 请求，经 Region（数据分片）下发到存储层，
// 再把 SelectResult 填回 Chunk。分区表、有序合并、虚拟列与 int64 边界拆分
// 等细节由 [`TableReaderExecutor`] 与 [`tableResultHandler`] 协作完成。

#![allow(non_camel_case_types, non_snake_case)]

use std::mem::size_of;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 半开 KV 键区间 [start_key, end_key)。
pub struct KeyRange {
    pub start_key: Vec<u8>,
    pub end_key: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 分区物理表 ID 及其对应的键区间列表。
pub struct PartitionIDAndRanges {
    pub id: i64,
    pub key_ranges: Vec<KeyRange>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 存储引擎类型：行存 TiKV、列存 TiFlash 或其他。
pub enum StoreType {
    TiKV,
    TiFlash,
    Other,
}

/// 构造 DistSQL 请求时的键来源形态。
pub enum RequestKeySource<R> {
    /// 已编码好的键区间列表。
    KeyRanges(Vec<KeyRange>),
    /// 按分区分组的键区间。
    PartitionKeyRanges(Vec<Vec<KeyRange>>),
    /// 分区 ID + 键区间对。
    PartitionIDAndRanges(Vec<PartitionIDAndRanges>),
    /// 由 handle（行标识）范围推导键。
    HandleRanges {
        table_id: i64,
        common_handle: bool,
        ranges: Vec<R>,
    },
}

/// 构建表扫描 DistSQL 请求时的公共选项（时间戳、副本范围、分页等）。
pub struct TableReaderRequestOptions<D, I, M> {
    pub dag_request: D,
    pub start_ts: u64,
    pub desc: bool,
    pub keep_order: bool,
    pub txn_scope: String,
    pub read_replica_scope: String,
    /// Separate TiFlash partition requests do not set this option.
    /// 是否 stale read（读历史快照）；TiFlash 分区分开请求时不设。
    pub is_staleness: Option<bool>,
    pub info_schema: I,
    pub memory_tracker: M,
    pub store_type: StoreType,
    pub paging: bool,
    pub allow_batch_cop: bool,
    pub net_data_size: f64,
    pub tidb_server_id: Option<String>,
}

/// 把逻辑 Range 编成 KV KeyRange；Separately 版本同时返回分区 ID。
pub trait kvRangeBuilder<D, R, E>: Send + Sync {
    fn buildKeyRange(&self, dctx: &D, ranges: &[R]) -> Result<Vec<Vec<KeyRange>>, E>;
    fn buildKeyRangeSeparately(
        &self,
        dctx: &D,
        ranges: &[R],
    ) -> Result<(Vec<i64>, Vec<Vec<KeyRange>>), E>;
}

/// TableReader 对会话/DistSQL/元数据/内存追踪等的后端依赖抽象。
pub trait TableReaderBackend: Send + Sync + 'static {
    type Context: Clone;
    type Error;
    type Chunk;
    type DistSQLContext: Clone;
    type RangerContext: Clone;
    type BuildPBContext: Clone;
    type ExprBuildContext: Clone;
    type InfoSchema: Clone;
    type ServerInfo;
    type MemoryTracker: Clone;
    type Table: Clone;
    type Range: Clone;
    type DagRequest: Clone;
    type Plan: Clone;
    type ColumnInfo: Clone;
    type FieldType: Clone;
    type Schema: Clone;
    type ByItem: Clone;
    type Request;
    type SelectResult;
    type TraceGuard;

    fn error(&self, message: String) -> Self::Error;
    fn trace_error(&self, error: Self::Error) -> Self::Error;
    fn dist_sql_context(&self) -> Self::DistSQLContext;
    fn ranger_context(&self) -> Self::RangerContext;
    fn build_pb_context(&self) -> Self::BuildPBContext;
    fn expression_build_context(&self) -> Self::ExprBuildContext;
    fn statement_memory_tracker(&self) -> Self::MemoryTracker;
    fn info_schema(&self) -> Self::InfoSchema;
    fn ddl_owner_available(&self) -> bool;
    fn ddl_owner(&self, context: &Self::Context) -> Result<Self::ServerInfo, Self::Error>;
    fn server_id(&self, server: &Self::ServerInfo) -> String;

    fn executor_id(&self) -> i32;
    fn new_memory_tracker(&self, executor_id: i32) -> Self::MemoryTracker;
    fn reset_memory_tracker(&self, tracker: &Self::MemoryTracker);
    fn attach_memory_tracker(&self, tracker: &Self::MemoryTracker, parent: &Self::MemoryTracker);
    fn start_trace_region(
        &self,
        context: &Self::Context,
        name: &str,
    ) -> (Self::TraceGuard, Self::Context);
    fn open_failpoint_delay(&self) -> Option<Duration>;

    fn runtime_stats_enabled(&self, dctx: &Self::DistSQLContext) -> bool;
    fn set_collect_execution_summaries(&self, dag: &mut Self::DagRequest, collect: bool);
    fn rebuild_tree_based_dag(
        &self,
        build_context: &Self::BuildPBContext,
        table_plan: &Self::Plan,
        dag: &mut Self::DagRequest,
    ) -> Result<(), Self::Error>;
    fn rebuild_list_based_dag(
        &self,
        build_context: &Self::BuildPBContext,
        plans: &[Self::Plan],
        dag: &mut Self::DagRequest,
    ) -> Result<(), Self::Error>;
    fn resolve_correlated_ranges(
        &self,
        table_scan: &Self::Plan,
    ) -> Result<Vec<Self::Range>, Self::Error>;
    fn group_ranges_by_columns(
        &self,
        ranges: &[Self::Range],
        column_indexes: &[usize],
    ) -> Result<Vec<Vec<Self::Range>>, Self::Error>;
    fn split_ranges_across_int64_boundary(
        &self,
        ranges: Vec<Self::Range>,
        keep_order: bool,
        desc: bool,
        common_handle: bool,
    ) -> (Vec<Self::Range>, Vec<Self::Range>);

    fn table_id(&self, table: &Self::Table) -> i64;
    fn table_name(&self, table: &Self::Table) -> Option<String>;
    fn table_is_common_handle(&self, table: &Self::Table) -> bool;
    fn table_is_cluster_table(&self, table: &Self::Table) -> bool;
    fn cluster_table_uses_ddl_owner(&self, table: &Self::Table) -> bool;
    fn range_memory_usage(&self, range: &Self::Range) -> i64;
    fn key_range_slice_memory_usage(&self, ranges: &[KeyRange]) -> i64;
    fn dag_size(&self, dag: &Self::DagRequest) -> usize;

    fn update_executor_table_ids(
        &self,
        context: &Self::Context,
        dag: &mut Self::DagRequest,
        partition_scan: bool,
        table_ids: &[i64],
    ) -> Result<(), Self::Error>;
    fn build_request(
        &self,
        dctx: &Self::DistSQLContext,
        source: RequestKeySource<Self::Range>,
        options: TableReaderRequestOptions<Self::DagRequest, Self::InfoSchema, Self::MemoryTracker>,
    ) -> Result<Self::Request, Self::Error>;
    fn request_key_ranges(&self, request: &Self::Request) -> Vec<KeyRange>;
    fn sort_request_key_ranges(&self, request: &mut Self::Request) -> Vec<KeyRange>;

    fn return_field_types(&self) -> Vec<Self::FieldType>;
    fn physical_plan_ids(&self, plans: &[Self::Plan]) -> Vec<i32>;
    fn plan_id(&self, plan: &Self::Plan) -> i32;
    fn select_with_runtime_stats(
        &self,
        context: &Self::Context,
        dctx: &Self::DistSQLContext,
        request: Self::Request,
        field_types: Vec<Self::FieldType>,
        cop_plan_ids: Vec<i32>,
        root_plan_id: i32,
    ) -> Result<Self::SelectResult, Self::Error>;
    fn serial_select_results(&self, results: Vec<Self::SelectResult>) -> Self::SelectResult;
    fn sorted_select_results(
        &self,
        expression_context: &Self::ExprBuildContext,
        results: Vec<Self::SelectResult>,
        schema: &Self::Schema,
        by_items: &[Self::ByItem],
        memory_tracker: &Self::MemoryTracker,
    ) -> Self::SelectResult;

    fn reset_chunk(&self, chunk: &mut Self::Chunk);
    fn chunk_num_rows(&self, chunk: &Self::Chunk) -> usize;
    fn select_result_next(
        &self,
        result: &mut Self::SelectResult,
        context: &Self::Context,
        chunk: &mut Self::Chunk,
    ) -> Result<(), Self::Error>;
    fn select_result_next_raw(
        &self,
        result: &mut Self::SelectResult,
        context: &Self::Context,
    ) -> Result<Option<Vec<u8>>, Self::Error>;
    fn close_select_result(&self, result: Self::SelectResult) -> Result<(), Self::Error>;
    fn normalize_context_error(&self, context: &Self::Context, error: Self::Error) -> Self::Error;

    fn log_table_scan(
        &self,
        context: &Self::Context,
        table_name: Option<&str>,
        ranges: &[Self::Range],
    );
    fn fill_virtual_column_values(
        &self,
        return_types: &[Self::FieldType],
        virtual_indexes: &[usize],
        schema: &Self::Schema,
        columns: &[Self::ColumnInfo],
        expression_context: &Self::ExprBuildContext,
        chunk: &mut Self::Chunk,
    ) -> Result<(), Self::Error>;
    fn schema_column_count(&self, schema: &Self::Schema) -> usize;
    fn schema_column_is_virtual(&self, schema: &Self::Schema, index: usize) -> bool;
    fn schema_column_id(&self, schema: &Self::Schema, index: usize) -> i64;
    fn schema_column_return_type(&self, schema: &Self::Schema, index: usize) -> Self::FieldType;
    fn column_offset_by_id(&self, columns: &[Self::ColumnInfo], id: i64) -> usize;

    fn report_cop_index_usage_for_handle(&self, table: &Self::Table, physical_plan_id: i32);
}

/// 测试/注入用：覆盖默认 SelectResult 构建逻辑。
pub trait SelectResultOverride<B: TableReaderBackend>: Send + Sync {
    fn select_result(
        &self,
        context: &B::Context,
        dctx: &B::DistSQLContext,
        request: B::Request,
        field_types: Vec<B::FieldType>,
        cop_plan_ids: Vec<i32>,
    ) -> Result<B::SelectResult, B::Error>;
}

/// SelectResult 钩子：有 override 则走注入实现，否则走 backend 默认路径。
pub struct selectResultHook<B: TableReaderBackend> {
    pub selectResultFunc: Option<Arc<dyn SelectResultOverride<B>>>,
}

impl<B: TableReaderBackend> selectResultHook<B> {
    /// 发起带运行时统计的 Select；优先使用注入的 override。
    pub fn SelectResult(
        &self,
        backend: &B,
        context: &B::Context,
        dctx: &B::DistSQLContext,
        request: B::Request,
        field_types: Vec<B::FieldType>,
        cop_plan_ids: Vec<i32>,
        root_plan_id: i32,
    ) -> Result<B::SelectResult, B::Error> {
        if let Some(select_result) = self.selectResultFunc.as_ref() {
            return select_result.select_result(context, dctx, request, field_types, cop_plan_ids);
        }
        backend.select_with_runtime_stats(
            context,
            dctx,
            request,
            field_types,
            cop_plan_ids,
            root_plan_id,
        )
    }
}

/// TableReader 打开时绑定的 DistSQL / Ranger / PB / 表达式 / 内存 / schema 上下文。
pub struct tableReaderExecutorContext<B: TableReaderBackend> {
    backend: Arc<B>,
    pub dctx: B::DistSQLContext,
    pub rctx: B::RangerContext,
    pub buildPBCtx: B::BuildPBContext,
    pub ectx: B::ExprBuildContext,
    pub stmtMemTracker: B::MemoryTracker,
    pub infoSchema: B::InfoSchema,
    pub getDDLOwner: bool,
}

impl<B: TableReaderBackend> tableReaderExecutorContext<B> {
    /// 返回当前 InfoSchema（元数据快照）克隆。
    pub fn GetInfoSchema(&self) -> B::InfoSchema {
        self.infoSchema.clone()
    }

    /// 在允许时查询 DDL Owner 节点信息；否则报错。
    pub fn GetDDLOwner(&self, context: &B::Context) -> Result<B::ServerInfo, B::Error> {
        if self.getDDLOwner {
            return self.backend.ddl_owner(context);
        }
        Err(self
            .backend
            .error("GetDDLOwner in a context without DDL".to_owned()))
    }
}

/// 从 backend 拉取各子上下文，构造 tableReaderExecutorContext。
pub fn newTableReaderExecutorContext<B: TableReaderBackend>(
    backend: Arc<B>,
) -> tableReaderExecutorContext<B> {
    tableReaderExecutorContext {
        dctx: backend.dist_sql_context(),
        rctx: backend.ranger_context(),
        buildPBCtx: backend.build_pb_context(),
        ectx: backend.expression_build_context(),
        stmtMemTracker: backend.statement_memory_tracker(),
        infoSchema: backend.info_schema(),
        getDDLOwner: backend.ddl_owner_available(),
        backend,
    }
}

/// 表扫描执行器：持有范围、DAG、排序合并选项与结果 handler。
pub struct TableReaderExecutor<B: TableReaderBackend> {
    pub tableReaderExecutorContext: tableReaderExecutorContext<B>,
    pub backend: Arc<B>,
    pub indexUsageReporter: bool,
    pub table: B::Table,
    pub kvRangeBuilder: Option<Arc<dyn kvRangeBuilder<B::DistSQLContext, B::Range, B::Error>>>,
    pub ranges: Vec<B::Range>,
    pub groupedRanges: Vec<Vec<B::Range>>,
    pub groupByColIdxs: Vec<usize>,
    pub kvRanges: Vec<KeyRange>,
    pub dagPB: B::DagRequest,
    pub startTS: u64,
    pub txnScope: String,
    pub readReplicaScope: String,
    pub isStaleness: bool,
    pub netDataSize: f64,
    pub columns: Vec<B::ColumnInfo>,
    pub resultHandler: Option<tableResultHandler<B>>,
    pub plans: Vec<B::Plan>,
    pub tablePlan: B::Plan,
    pub schema: B::Schema,
    pub memTracker: Option<B::MemoryTracker>,
    pub selectResultHook: selectResultHook<B>,
    pub keepOrder: bool,
    pub desc: bool,
    pub byItems: Vec<B::ByItem>,
    pub paging: bool,
    pub storeType: StoreType,
    pub corColInFilter: bool,
    pub corColInAccess: bool,
    pub virtualColumnIndex: Vec<usize>,
    pub virtualColumnRetFieldTypes: Vec<B::FieldType>,
    pub batchCop: bool,
    pub dummy: bool,
}

impl<B: TableReaderBackend> TableReaderExecutor<B> {
    /// 返回关联表元数据。
    pub fn Table(&self) -> B::Table {
        self.table.clone()
    }

    /// 标记为 dummy：Open 只收集 kvRanges，不发起真实 Select。
    pub fn setDummy(&mut self) {
        self.dummy = true;
    }

    /// 估算执行器自身、ranges、kvRanges 与 DAG 的内存占用。
    pub fn memUsage(&self) -> i64 {
        let mut result = size_of::<Self>() as i64;
        result += size_of::<*const ()>() as i64 * self.ranges.capacity() as i64;
        for range in &self.ranges {
            result += self.backend.range_memory_usage(range);
        }
        result += self.backend.key_range_slice_memory_usage(&self.kvRanges);
        result += self.backend.dag_size(&self.dagPB) as i64;
        result
    }

    /// 打开扫描：挂内存追踪、重建相关子查询 DAG、拆 int64 边界并建 SelectResult。
    pub fn Open(&mut self, context: &B::Context) -> Result<(), B::Error> {
        let (_trace, context) = self
            .backend
            .start_trace_region(context, "TableReaderExecutor.Open");
        if let Some(delay) = self.backend.open_failpoint_delay() {
            thread::sleep(delay);
        }

        // 复用或新建执行器级 memory tracker，并挂到语句级 tracker。
        let tracker = match self.memTracker.take() {
            Some(tracker) => {
                self.backend.reset_memory_tracker(&tracker);
                tracker
            }
            None => self.backend.new_memory_tracker(self.backend.executor_id()),
        };
        self.backend
            .attach_memory_tracker(&tracker, &self.tableReaderExecutorContext.stmtMemTracker);
        self.memTracker = Some(tracker);

        // Filter 中含相关列时，按存储类型重建 tree/list DAG。
        if self.corColInFilter {
            if self.storeType == StoreType::TiFlash {
                self.backend.rebuild_tree_based_dag(
                    &self.tableReaderExecutorContext.buildPBCtx,
                    &self.tablePlan,
                    &mut self.dagPB,
                )?;
            } else {
                self.backend.rebuild_list_based_dag(
                    &self.tableReaderExecutorContext.buildPBCtx,
                    &self.plans,
                    &mut self.dagPB,
                )?;
            }
        }
        if self
            .backend
            .runtime_stats_enabled(&self.tableReaderExecutorContext.dctx)
        {
            self.backend
                .set_collect_execution_summaries(&mut self.dagPB, true);
        }
        // Access 中含相关列时，重新解析范围并可按列分组。
        if self.corColInAccess {
            self.ranges = self.backend.resolve_correlated_ranges(&self.plans[0])?;
            if !self.groupByColIdxs.is_empty() {
                self.groupedRanges = self
                    .backend
                    .group_ranges_by_columns(&self.ranges, &self.groupByColIdxs)?;
            }
        }

        self.resultHandler = Some(tableResultHandler::new(Arc::clone(&self.backend)));
        let grouped_ranges = if !self.groupedRanges.is_empty() {
            self.groupedRanges.clone()
        } else if !self.ranges.is_empty() {
            vec![self.ranges.clone()]
        } else {
            Vec::new()
        };

        // 有序扫描时，有符号/无符号 int64 键需拆成两段再合并，避免跨边界乱序。
        let common_handle = self.backend.table_is_common_handle(&self.table);
        let mut first_part = Vec::new();
        let mut second_part = Vec::new();
        for ranges in grouped_ranges {
            let (signed, unsigned) = self.backend.split_ranges_across_int64_boundary(
                ranges,
                self.keepOrder,
                self.desc,
                common_handle,
            );
            if !signed.is_empty() {
                first_part.push(signed);
            }
            if !unsigned.is_empty() {
                second_part.push(unsigned);
            }
        }

        if self.dummy {
            // Dummy 模式：只编码请求以收集 kvRanges，不打开结果流。
            if self.desc && !second_part.is_empty() {
                std::mem::swap(&mut first_part, &mut second_part);
            }
            for ranges in first_part.into_iter().chain(second_part) {
                let request = self.buildKVReq(&context, &ranges)?;
                self.kvRanges
                    .extend(self.backend.request_key_ranges(&request));
            }
            return Ok(());
        }

        // 两段结果：optional 先读完再读主结果，保证有序合并。
        let first_result = self.buildRespForGroupedRanges(&context, &first_part)?;
        if second_part.is_empty() {
            self.resultHandler
                .as_mut()
                .expect("table result handler must be initialized")
                .open(None, first_result);
            return Ok(());
        }
        let second_result = self.buildRespForGroupedRanges(&context, &second_part)?;
        self.resultHandler
            .as_mut()
            .expect("table result handler must be initialized")
            .open(Some(first_result), second_result);
        Ok(())
    }

    /// 拉取下一批行到 request Chunk，并填充虚拟列。
    pub fn Next(&mut self, context: &B::Context, request: &mut B::Chunk) -> Result<(), B::Error> {
        if self.dummy {
            self.backend.reset_chunk(request);
            return Ok(());
        }

        let table_name = self.backend.table_name(&self.table);
        self.backend
            .log_table_scan(context, table_name.as_deref(), &self.ranges);
        // Do not reset or replace the caller chunk: its requiredRows contract
        // must reach the underlying SelectResult unchanged.
        // 不重置调用方 Chunk，以保留 requiredRows 契约原样下推。
        self.resultHandler
            .as_mut()
            .expect("table result handler must be initialized")
            .nextChunk(context, request)?;
        self.backend.fill_virtual_column_values(
            &self.virtualColumnRetFieldTypes,
            &self.virtualColumnIndex,
            &self.schema,
            &self.columns,
            &self.tableReaderExecutorContext.ectx,
            request,
        )
    }

    /// 关闭结果流；可选上报 cop 侧索引使用情况。
    pub fn Close(&mut self) -> Result<(), B::Error> {
        if self.indexUsageReporter {
            self.backend.report_cop_index_usage_for_handle(
                &self.table,
                self.backend.plan_id(&self.plans[0]),
            );
        }

        let close_result = match self.resultHandler.as_mut() {
            Some(handler) => handler.Close(),
            None => Ok(()),
        };
        self.kvRanges.clear();
        if self.dummy {
            return Ok(());
        }
        close_result
    }

    /// 为分组范围构建 SelectResult（TiFlash 分区 / 多请求有序合并）。
    pub fn buildRespForGroupedRanges(
        &mut self,
        context: &B::Context,
        grouped_ranges: &[Vec<B::Range>],
    ) -> Result<B::SelectResult, B::Error> {
        if self.storeType == StoreType::TiFlash && self.kvRangeBuilder.is_some() {
            assert!(grouped_ranges.len() == 1 && self.groupedRanges.is_empty());
        }
        // TiFlash + 分区 range builder：可 batchCop 或按分区分别 Select。
        if self.storeType == StoreType::TiFlash
            && self.kvRangeBuilder.is_some()
            && grouped_ranges.len() == 1
            && self.groupedRanges.is_empty()
        {
            let ranges = &grouped_ranges[0];
            if !self.batchCop {
                let mut requests = self.buildKVReqSeparately(context, ranges)?;
                self.kvRanges = sortAndGetKVRangesFromReqs(self.backend.as_ref(), &mut requests);
                let mut results = Vec::with_capacity(requests.len());
                for request in requests {
                    results.push(self.select_result(context, request)?);
                }
                return Ok(self.backend.serial_select_results(results));
            }

            let mut request = self.buildKVReqForPartitionTableScan(context, ranges)?;
            self.kvRanges = sortAndGetKVRangesFromReqs(
                self.backend.as_ref(),
                std::slice::from_mut(&mut request),
            );
            return self.select_result(context, request);
        }

        let mut requests = self.buildKVReqSeparatelyForGroupedRanges(context, grouped_ranges)?;
        if requests.is_empty() {
            requests.push(self.buildKVReq(context, &[])?);
        }
        self.kvRanges = sortAndGetKVRangesFromReqs(self.backend.as_ref(), &mut requests);
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            results.push(self.select_result(context, request)?);
        }
        if results.len() == 1 {
            return Ok(results.remove(0));
        }

        // 多路结果按 byItems 做有序归并。
        assert!(!self.byItems.is_empty());
        Ok(self.backend.sorted_select_results(
            &self.tableReaderExecutorContext.ectx,
            results,
            &self.schema,
            &self.byItems,
            self.memTracker
                .as_ref()
                .expect("table reader memory tracker must be initialized"),
        ))
    }

    /// 每组范围各建一个或一组 KV 请求。
    pub fn buildKVReqSeparatelyForGroupedRanges(
        &mut self,
        context: &B::Context,
        grouped_ranges: &[Vec<B::Range>],
    ) -> Result<Vec<B::Request>, B::Error> {
        let mut requests = Vec::new();
        for ranges in grouped_ranges {
            // 有 byItems 且可按分区拆分时，每分区单独请求以便有序合并。
            if self.kvRangeBuilder.is_some() && !self.byItems.is_empty() {
                requests.extend(self.buildKVReqSeparately(context, ranges)?);
            } else {
                requests.push(self.buildKVReq(context, ranges)?);
            }
        }
        Ok(requests)
    }

    /// 按分区分别编码键区间并各建一个 DistSQL 请求。
    pub fn buildKVReqSeparately(
        &mut self,
        context: &B::Context,
        ranges: &[B::Range],
    ) -> Result<Vec<B::Request>, B::Error> {
        let (partition_ids, key_ranges) = self
            .kvRangeBuilder
            .as_ref()
            .expect("partition range builder must exist")
            .buildKeyRangeSeparately(&self.tableReaderExecutorContext.dctx, ranges)?;
        let mut requests = Vec::with_capacity(key_ranges.len());
        for (index, key_range) in key_ranges.into_iter().enumerate() {
            self.backend.update_executor_table_ids(
                context,
                &mut self.dagPB,
                true,
                &[partition_ids[index]],
            )?;
            requests.push(self.backend.build_request(
                &self.tableReaderExecutorContext.dctx,
                RequestKeySource::KeyRanges(key_range),
                self.request_options(None, None),
            )?);
        }
        Ok(requests)
    }

    /// 分区表扫描：把各分区 ID+键区间打成单个 batch 请求。
    pub fn buildKVReqForPartitionTableScan(
        &mut self,
        context: &B::Context,
        ranges: &[B::Range],
    ) -> Result<B::Request, B::Error> {
        let (partition_ids, key_ranges) = self
            .kvRangeBuilder
            .as_ref()
            .expect("partition range builder must exist")
            .buildKeyRangeSeparately(&self.tableReaderExecutorContext.dctx, ranges)?;
        let partition_ranges = partition_ids
            .iter()
            .copied()
            .zip(key_ranges)
            .map(|(id, key_ranges)| PartitionIDAndRanges { id, key_ranges })
            .collect();
        self.backend
            .update_executor_table_ids(context, &mut self.dagPB, true, &partition_ids)?;
        self.backend.build_request(
            &self.tableReaderExecutorContext.dctx,
            RequestKeySource::PartitionIDAndRanges(partition_ranges),
            self.request_options(None, None),
        )
    }

    /// 构造单次 DistSQL 请求：有分区 builder 则用分区键，否则用 handle 范围。
    pub fn buildKVReq(
        &mut self,
        context: &B::Context,
        ranges: &[B::Range],
    ) -> Result<B::Request, B::Error> {
        let source = match self.kvRangeBuilder.as_ref() {
            Some(range_builder) => RequestKeySource::PartitionKeyRanges(
                range_builder.buildKeyRange(&self.tableReaderExecutorContext.dctx, ranges)?,
            ),
            None => RequestKeySource::HandleRanges {
                table_id: self.backend.table_id(&self.table),
                common_handle: self.backend.table_is_common_handle(&self.table),
                ranges: ranges.to_vec(),
            },
        };

        // 集群表若需 DDL Owner 路由，则附带 server id。
        let server_id = if self.backend.table_is_cluster_table(&self.table)
            && self.backend.cluster_table_uses_ddl_owner(&self.table)
        {
            let server = self.tableReaderExecutorContext.GetDDLOwner(context)?;
            Some(self.backend.server_id(&server))
        } else {
            None
        };
        self.backend.build_request(
            &self.tableReaderExecutorContext.dctx,
            source,
            self.request_options(Some(self.isStaleness), server_id),
        )
    }

    /// 根据 schema 计算虚拟列下标与返回类型并缓存到执行器。
    pub fn buildVirtualColumnInfo(&mut self) {
        let (indexes, return_types) =
            buildVirtualColumnInfo(self.backend.as_ref(), &self.schema, &self.columns);
        self.virtualColumnIndex = indexes;
        self.virtualColumnRetFieldTypes = return_types;
    }

    /// 组装 TableReaderRequestOptions（含可选 stale 标记与 server id）。
    fn request_options(
        &self,
        is_staleness: Option<bool>,
        tidb_server_id: Option<String>,
    ) -> TableReaderRequestOptions<B::DagRequest, B::InfoSchema, B::MemoryTracker> {
        TableReaderRequestOptions {
            dag_request: self.dagPB.clone(),
            start_ts: self.startTS,
            desc: self.desc,
            keep_order: self.keepOrder,
            txn_scope: self.txnScope.clone(),
            read_replica_scope: self.readReplicaScope.clone(),
            is_staleness,
            info_schema: self.tableReaderExecutorContext.GetInfoSchema(),
            memory_tracker: self
                .memTracker
                .as_ref()
                .expect("table reader memory tracker must be initialized")
                .clone(),
            store_type: self.storeType,
            paging: self.paging,
            allow_batch_cop: self.batchCop,
            net_data_size: self.netDataSize,
            tidb_server_id,
        }
    }

    /// 经 selectResultHook 发起 Select。
    fn select_result(
        &self,
        context: &B::Context,
        request: B::Request,
    ) -> Result<B::SelectResult, B::Error> {
        self.selectResultHook.SelectResult(
            self.backend.as_ref(),
            context,
            &self.tableReaderExecutorContext.dctx,
            request,
            self.backend.return_field_types(),
            self.backend.physical_plan_ids(&self.plans),
            self.backend.executor_id(),
        )
    }
}

/// 对各请求排序键区间后按 start_key 全局排序并拼接。
pub fn sortAndGetKVRangesFromReqs<B: TableReaderBackend>(
    backend: &B,
    requests: &mut [B::Request],
) -> Vec<KeyRange> {
    let mut ranges = Vec::with_capacity(requests.len());
    for request in requests {
        let request_ranges = backend.sort_request_key_ranges(request);
        ranges.extend(request_ranges);
    }
    ranges.sort_by(|left, right| left.start_key.cmp(&right.start_key));
    ranges
}

/// 收集 schema 中虚拟列下标，并按列在 columns 中的偏移排序。
pub fn buildVirtualColumnIndex<B: TableReaderBackend>(
    backend: &B,
    schema: &B::Schema,
    columns: &[B::ColumnInfo],
) -> Vec<usize> {
    let mut indexes = Vec::with_capacity(columns.len());
    for index in 0..backend.schema_column_count(schema) {
        if backend.schema_column_is_virtual(schema, index) {
            indexes.push(index);
        }
    }
    indexes.sort_by(|left, right| {
        let left_id = backend.schema_column_id(schema, *left);
        let right_id = backend.schema_column_id(schema, *right);
        let left_offset = backend.column_offset_by_id(columns, left_id);
        let right_offset = backend.column_offset_by_id(columns, right_id);
        left_offset.cmp(&right_offset)
    });
    indexes
}

/// 返回虚拟列下标及其 FieldType 列表。
pub fn buildVirtualColumnInfo<B: TableReaderBackend>(
    backend: &B,
    schema: &B::Schema,
    columns: &[B::ColumnInfo],
) -> (Vec<usize>, Vec<B::FieldType>) {
    let indexes = buildVirtualColumnIndex(backend, schema, columns);
    let return_types = indexes
        .iter()
        .map(|index| backend.schema_column_return_type(schema, *index))
        .collect();
    (indexes, return_types)
}

/// 双路 SelectResult 合并器：先耗尽 optional，再读主 result。
pub struct tableResultHandler<B: TableReaderBackend> {
    backend: Arc<B>,
    pub optionalResult: Option<B::SelectResult>,
    pub result: Option<B::SelectResult>,
    pub optionalFinished: bool,
}

impl<B: TableReaderBackend> tableResultHandler<B> {
    /// 构造空 handler，待 [`open`] 绑定结果流。
    fn new(backend: Arc<B>) -> Self {
        Self {
            backend,
            optionalResult: None,
            result: None,
            optionalFinished: false,
        }
    }

    /// 注册 optional + 主结果；无 optional 则直接标记完成。
    pub fn open(&mut self, optional_result: Option<B::SelectResult>, result: B::SelectResult) {
        if optional_result.is_none() {
            self.optionalFinished = true;
            self.result = Some(result);
            return;
        }
        self.optionalResult = optional_result;
        self.result = Some(result);
        self.optionalFinished = false;
    }

    /// 先从 optional 填 Chunk；耗尽后再从主 result 拉取。
    pub fn nextChunk(
        &mut self,
        context: &B::Context,
        chunk: &mut B::Chunk,
    ) -> Result<(), B::Error> {
        if !self.optionalFinished {
            self.backend.select_result_next(
                self.optionalResult
                    .as_mut()
                    .expect("optional table result must exist"),
                context,
                chunk,
            )?;
            if self.backend.chunk_num_rows(chunk) > 0 {
                return Ok(());
            }
            self.optionalFinished = true;
        }
        self.backend.select_result_next(
            self.result.as_mut().expect("table result must exist"),
            context,
            chunk,
        )
    }

    /// 原始字节版 next：同样优先 optional。
    pub fn nextRaw(&mut self, context: &B::Context) -> Result<Option<Vec<u8>>, B::Error> {
        if !self.optionalFinished {
            let data = self
                .backend
                .select_result_next_raw(
                    self.optionalResult
                        .as_mut()
                        .expect("optional table result must exist"),
                    context,
                )
                .map_err(|error| self.backend.normalize_context_error(context, error))?;
            if data.is_some() {
                return Ok(data);
            }
            self.optionalFinished = true;
        }
        self.backend
            .select_result_next_raw(
                self.result.as_mut().expect("table result must exist"),
                context,
            )
            .map_err(|error| self.backend.normalize_context_error(context, error))
    }

    /// 关闭两路结果，保留首个错误并 trace。
    pub fn Close(&mut self) -> Result<(), B::Error> {
        let mut first_error = None;
        if let Some(optional_result) = self.optionalResult.take()
            && let Err(error) = self.backend.close_select_result(optional_result)
        {
            first_error = Some(error);
        }
        if let Some(result) = self.result.take()
            && let Err(error) = self.backend.close_select_result(result)
            && first_error.is_none()
        {
            first_error = Some(error);
        }
        match first_error {
            Some(error) => Err(self.backend.trace_error(error)),
            None => Ok(()),
        }
    }
}
