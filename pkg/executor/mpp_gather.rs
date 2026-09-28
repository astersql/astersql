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

// MPP Gather 执行器：从 TiFlash 等 MPP 节点汇聚查询结果。
//
// MPP（大规模并行处理）把物理计划拆成可在存储节点并行执行的任务；
// Gather 在 TiDB 侧打开协调执行器、拉取 chunk（行批次），并填充虚拟列。
// `dummy` 模式只生成根任务 KeyRange，不真正消费结果（用于 Explain 等）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 判断表侧计划根节点是否为 ExchangeSender（数据交换发送端）。
pub trait MPPTableReader {
    fn table_plan_is_exchange_sender(&self) -> bool;
}

/// 会话上下文：是否允许 MPP，以及查询级 MPP 标识。
pub trait MPPSessionContext {
    fn is_mpp_allowed(&self) -> bool;
    fn mpp_query_info(&self) -> &MPPQueryInfo;
}

/// 一次 MPP 查询的 QueryID / QueryTS（时间戳），用原子量做惰性分配。
#[derive(Debug, Default)]
pub struct MPPQueryInfo {
    pub QueryID: AtomicU64,
    pub QueryTS: AtomicU64,
}

/// 会话允许 MPP 且表计划为 ExchangeSender 时启用 MPP 执行。
pub fn useMPPExecution<C: MPPSessionContext, T: MPPTableReader>(context: &C, reader: &T) -> bool {
    context.is_mpp_allowed() && reader.table_plan_is_exchange_sender()
}

/// 惰性分配并返回 MPP QueryID（仅在仍为 0 时写入）。
pub fn getMPPQueryID<C: MPPSessionContext>(
    context: &C,
    allocate_mpp_query_id: impl FnOnce() -> u64,
) -> u64 {
    let query_id = &context.mpp_query_info().QueryID;
    let allocated = allocate_mpp_query_id();
    let _ = query_id.compare_exchange(0, allocated, Ordering::SeqCst, Ordering::SeqCst);
    query_id.load(Ordering::SeqCst)
}

/// 惰性写入当前纳秒时间戳作为 QueryTS。
pub fn getMPPQueryTS<C: MPPSessionContext>(context: &C) -> u64 {
    let query_ts = &context.mpp_query_info().QueryTS;
    let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as u64,
        Err(error) => 0u64.wrapping_sub(error.duration().as_nanos() as u64),
    };
    let _ = query_ts.compare_exchange(0, now, Ordering::SeqCst, Ordering::SeqCst);
    query_ts.load(Ordering::SeqCst)
}

/// 物理计划树抽象：用于收集 plan id 与识别 ExchangeSender。
pub trait PhysicalPlan: Clone {
    type ExchangeSender;

    fn id(&self) -> i32;
    fn plan_type(&self) -> &str;
    fn children(&self) -> Vec<Self>;
    fn as_exchange_sender(&self) -> Option<&Self::ExchangeSender>;
}

/// 深度优先收集物理计划树中所有节点 id。
pub fn collectPlanIDs<P: PhysicalPlan>(plan: &P, ids: &mut Vec<i32>) {
    ids.push(plan.id());
    for child in plan.children() {
        collectPlanIDs(&child, ids);
    }
}

/// MPP 返回的数据批次接口。
pub trait MPPChunk {
    fn reset(&mut self);
    fn num_rows(&self) -> usize;
}

/// Concrete integration boundary for DistSQL, MPP retry, schema and table
/// operations. Every Go side effect is required; there is no default success
/// implementation.
///
/// DistSQL / MPP 重试 / schema 与表操作的集成边界；无默认成功实现。
pub trait MPPGatherRuntime {
    type Error;
    type Context;
    type Plan: PhysicalPlan;
    type InfoSchema;
    type MPPQueryID: Clone;
    type SelectResult;
    type MPPExecutor;
    type MemoryTracker;
    type ColumnInfo;
    type FieldType;
    type SchemaColumn;
    type Table: Clone;
    type KeyRange: Clone;

    fn error(&self, message: String) -> Self::Error;
    fn generate_root_mpp_tasks(
        &mut self,
        start_ts: u64,
        query_id: Self::MPPQueryID,
        sender: &<Self::Plan as PhysicalPlan>::ExchangeSender,
        info_schema: &Self::InfoSchema,
    ) -> Result<Vec<Self::KeyRange>, Self::Error>;
    fn new_executor_with_retry(
        &mut self,
        context: &Self::Context,
        memory_tracker: &mut Self::MemoryTracker,
        plan_ids: &[i32],
        plan: Self::Plan,
        start_ts: u64,
        query_id: Self::MPPQueryID,
        info_schema: &Self::InfoSchema,
    ) -> Result<Self::MPPExecutor, (Option<Self::MPPExecutor>, Self::Error)>;
    fn executor_key_ranges(&self, executor: &Self::MPPExecutor) -> Vec<Self::KeyRange>;
    fn select_result_from_mpp_response(
        &mut self,
        plan_ids: &[i32],
        executor_id: i32,
        executor: &mut Self::MPPExecutor,
    ) -> Self::SelectResult;
    fn next_result<Q: MPPChunk>(
        &mut self,
        result: &mut Self::SelectResult,
        context: &Self::Context,
        chunk: &mut Q,
    ) -> Result<(), Self::Error>;
    fn close_result(&mut self, result: &mut Self::SelectResult) -> Result<(), Self::Error>;
    fn close_mpp_executor(&mut self, executor: &mut Self::MPPExecutor) -> Result<(), Self::Error>;
    fn fill_virtual_column_values<Q: MPPChunk>(
        &mut self,
        return_field_types: &[Self::FieldType],
        virtual_column_indices: &[usize],
        columns: &[Self::ColumnInfo],
        chunk: &mut Q,
    ) -> Result<(), Self::Error>;
    fn executor_id(&self) -> i32;
}

/// MPP Gather 算子状态：持有计划、时间戳、响应迭代器与虚拟列信息。
pub struct MPPGather<R>
where
    R: MPPGatherRuntime,
{
    pub BaseExecutor: R,
    pub is: R::InfoSchema,
    pub originalPlan: R::Plan,
    /// 快照读起始时间戳（start_ts）。
    pub startTS: u64,
    pub mppQueryID: R::MPPQueryID,
    pub respIter: Option<R::SelectResult>,
    pub memTracker: R::MemoryTracker,
    pub columns: Vec<R::ColumnInfo>,
    pub virtualColumnIndex: Vec<usize>,
    pub virtualColumnRetFieldTypes: Vec<R::FieldType>,
    pub table: R::Table,
    /// 根任务覆盖的 KeyRange（键范围，对应 Region 扫描区间）。
    pub kvRanges: Vec<R::KeyRange>,
    pub dummy: bool,
    pub mppExec: Option<R::MPPExecutor>,
}

impl<R> MPPGather<R>
where
    R: MPPGatherRuntime,
{
    /// 打开 Gather：dummy 只生成任务范围；否则创建 MPP 执行器与结果迭代器。
    pub fn Open(&mut self, context: &R::Context) -> Result<(), R::Error> {
        if self.dummy {
            let Some(sender) = self.originalPlan.as_exchange_sender() else {
                return Err(self.BaseExecutor.error(format!(
                    "unexpected plan type, expect: PhysicalExchangeSender, got: {}",
                    self.originalPlan.plan_type()
                )));
            };
            self.kvRanges = self.BaseExecutor.generate_root_mpp_tasks(
                self.startTS,
                self.mppQueryID.clone(),
                sender,
                &self.is,
            )?;
            return Ok(());
        }

        let mut plan_ids = Vec::new();
        collectPlanIDs(&self.originalPlan, &mut plan_ids);
        // 创建失败时尽量关闭已分配的执行器再返回错误。
        let executor = match self.BaseExecutor.new_executor_with_retry(
            context,
            &mut self.memTracker,
            &plan_ids,
            self.originalPlan.clone(),
            self.startTS,
            self.mppQueryID.clone(),
            &self.is,
        ) {
            Ok(executor) => executor,
            Err((mut executor, error)) => {
                if let Some(executor) = executor.as_mut() {
                    let _ = self.BaseExecutor.close_mpp_executor(executor);
                }
                self.mppExec = executor;
                return Err(error);
            }
        };
        self.kvRanges = self.BaseExecutor.executor_key_ranges(&executor);
        self.mppExec = Some(executor);
        let executor = self
            .mppExec
            .as_mut()
            .expect("MPP executor was stored immediately above");
        let executor_id = self.BaseExecutor.executor_id();
        self.respIter = Some(self.BaseExecutor.select_result_from_mpp_response(
            &plan_ids,
            executor_id,
            executor,
        ));
        Ok(())
    }

    /// 拉取下一批行；dummy 直接返回空；有行时填充虚拟列。
    pub fn Next<Q: MPPChunk>(
        &mut self,
        context: &R::Context,
        chunk: &mut Q,
    ) -> Result<(), R::Error> {
        chunk.reset();
        if self.dummy {
            return Ok(());
        }
        let result = self
            .respIter
            .as_mut()
            .expect("Open must create a response iterator before Next");
        self.BaseExecutor.next_result(result, context, chunk)?;
        if chunk.num_rows() == 0 {
            return Ok(());
        }
        self.BaseExecutor.fill_virtual_column_values(
            &self.virtualColumnRetFieldTypes,
            &self.virtualColumnIndex,
            &self.columns,
            chunk,
        )
    }

    /// 关闭结果迭代器；dummy 下若仍有迭代器则视为内部错误。
    pub fn Close(&mut self) -> Result<(), R::Error> {
        if self.dummy {
            if let Some(result) = self.respIter.as_mut() {
                let _ = self.BaseExecutor.close_result(result);
                return Err(self
                    .BaseExecutor
                    .error("e.respIter != nil when e.dummy is set".into()));
            }
            return Ok(());
        }
        if let Some(result) = self.respIter.as_mut() {
            return self.BaseExecutor.close_result(result);
        }
        Ok(())
    }

    /// 返回关联表元信息。
    pub fn Table(&self) -> R::Table {
        self.table.clone()
    }

    /// 标记为 dummy（仅生成任务、不读结果）。
    pub fn setDummy(&mut self) {
        self.dummy = true;
    }
}
