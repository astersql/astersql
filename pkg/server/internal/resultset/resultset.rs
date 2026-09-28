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

// 查询结果集核心实现与游标 RU v2（Request Unit）消耗追踪。
//
// `TidbResultSet` 包装 sqlexec::RecordSet，负责列信息缓存、Finish/Close
// 生命周期与可分离（TryDetach）结果集；`CursorRUV2Tracker` 在游标 fetch
// 过程中按增量上报 TiDB / TiKV / TiFlash 的 RU 消耗给资源组。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, TryLockError};

use astersql_planner_core::PlanCacheStmt;
use astersql_resourcegroup::ConsumptionReporter;
use astersql_server_internal_column::{ConvertColumnInfo, Info};
use astersql_util_chunk as chunk;
use astersql_util_execdetails::ruv2_metrics::{
    RUV2Metrics, RUV2Weights, SyncRUV2MetricsFromRUDetails, tikvutil,
};
use astersql_util_sqlexec as sqlexec;

/// 预编译语句引用（带列 Info 的 PlanCacheStmt）。
pub type PreparedStmtRef = Arc<PlanCacheStmt<Arc<Info>>>;

/// 服务端结果集抽象：列元数据、按 chunk 拉取、关闭与游标 RU 上报。
pub trait ResultSet {
    /// 返回列定义信息（可能来自预编译缓存）。
    fn Columns(&mut self) -> Vec<Arc<Info>>;
    /// 按字段类型新建一个空 RecordChunk。
    fn NewChunk(&mut self, allocator: Option<&mut dyn chunk::Allocator>) -> sqlexec::RecordChunk;
    /// 向 `req` 填充下一批行；无更多行时 chunk 为空。
    fn Next(
        &mut self,
        ctx: &sqlexec::context::Context,
        req: &mut sqlexec::RecordChunk,
    ) -> Result<(), sqlexec::GoError>;
    /// 关闭结果集并释放底层 RecordSet（幂等）。
    fn Close(&mut self);
    /// 是否已关闭。
    fn IsClosed(&self) -> bool;
    /// 返回各列 FieldType（用于分配 chunk）。
    fn FieldTypes(&self) -> Vec<Box<chunk::types::FieldType>>;
    /// 绑定预编译语句，用于列信息缓存。
    fn SetPreparedStmt(&mut self, stmt: Option<PreparedStmtRef>);
    /// 尝试结束执行（与 Next 互斥；锁忙则跳过）。
    fn Finish(&mut self) -> Result<(), sqlexec::GoError>;
    /// 尝试将剩余结果分离为独立 ResultSet（如 cursor 场景）。
    fn TryDetach(&mut self) -> Result<(Option<Box<dyn ResultSet>>, bool), sqlexec::GoError>;
    /// Fetch 返回客户端后的钩子。
    fn OnFetchReturned(&mut self);
    /// 挂载游标 RU v2 追踪器。
    fn SetCursorRUV2Tracker(&mut self, tracker: Option<Arc<CursorRUV2Tracker>>);
    /// 上报结果 chunk 单元格增量并刷新 RU 增量。
    fn ReportCursorRUV2Delta(&mut self, result_chunk_cells_delta: i64);
}

/// Object-safe view of `resourcegroup.ConsumptionReporter` used by a cursor.
/// 游标侧 object-safe 的 RU v2 消耗上报接口（对资源组计费）。
pub trait CursorRUV2Reporter: Send + Sync {
    fn ReportRUV2Consumption(
        &self,
        resource_group_name: &str,
        tikv_ruv2: f64,
        tidb_ruv2: f64,
        tiflash_ruv2: f64,
    );
}

impl<T> CursorRUV2Reporter for T
where
    T: ConsumptionReporter,
{
    fn ReportRUV2Consumption(
        &self,
        resource_group_name: &str,
        tikv_ruv2: f64,
        tidb_ruv2: f64,
        tiflash_ruv2: f64,
    ) {
        self.report_ruv2_consumption(resource_group_name, tikv_ruv2, tidb_ruv2, tiflash_ruv2);
    }
}

/// 已向资源组上报过的 RU 累计值，用于计算增量。
#[derive(Default)]
struct CursorRUV2ReportState {
    reported_tidb_ru: f64,
    reported_tikv_ruv2: f64,
    reported_tiflash_ru: f64,
}

/// 游标执行期间的 RU v2 追踪器：累计 TiDB 侧指标与 TiKV/TiFlash RUDetails，
/// 并按 fetch 增量上报到资源组。
pub struct CursorRUV2Tracker {
    reporter: Option<Arc<dyn CursorRUV2Reporter>>,
    metrics: Option<Arc<RUV2Metrics>>,
    ru_details: Option<Arc<tikvutil::RUDetails>>,
    resource_group_name: String,
    weights: RUV2Weights,
    state: Mutex<CursorRUV2ReportState>,
}

/// 构造游标 RU 追踪器；无 metrics/ru_details 或 metrics.Bypass 时返回 None。
pub fn NewCursorRUV2Tracker(
    reporter: Option<Arc<dyn CursorRUV2Reporter>>,
    resource_group_name: String,
    metrics: Option<Arc<RUV2Metrics>>,
    ru_details: Option<Arc<tikvutil::RUDetails>>,
    weights: RUV2Weights,
) -> Option<Arc<CursorRUV2Tracker>> {
    if metrics.is_none() && ru_details.is_none() {
        return None;
    }
    if metrics.as_ref().is_some_and(|metrics| metrics.Bypass()) {
        return None;
    }

    // 用 RUDetails 同步 metrics，并以当前累计值作为已上报基线。
    SyncRUV2MetricsFromRUDetails(metrics.as_deref(), ru_details.as_deref());
    let state = CursorRUV2ReportState {
        reported_tidb_ru: metrics
            .as_ref()
            .map_or(0.0, |metrics| metrics.CalculateRUValues(weights)),
        reported_tikv_ruv2: ru_details
            .as_ref()
            .map_or(0.0, |details| details.TiKVRUV2()),
        reported_tiflash_ru: ru_details
            .as_ref()
            .map_or(0.0, |details| details.TiflashRU()),
    };

    Some(Arc::new(CursorRUV2Tracker {
        reporter,
        metrics,
        ru_details,
        resource_group_name,
        weights,
        state: Mutex::new(state),
    }))
}

impl CursorRUV2Tracker {
    /// 将结果 chunk 单元格数计入 TiDB 侧 RU 指标。
    fn addResultChunkCells(&self, delta: i64) {
        if delta <= 0 {
            return;
        }
        if let Some(metrics) = &self.metrics {
            metrics.AddResultChunkCells(delta);
        }
    }

    /// 计算相对上次上报的正增量，并通知资源组；随后更新已上报基线。
    fn reportDelta(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let current_tidb_ru = if let Some(metrics) = &self.metrics {
            SyncRUV2MetricsFromRUDetails(Some(metrics), self.ru_details.as_deref());
            metrics.CalculateRUValues(self.weights)
        } else {
            0.0
        };
        let current_tikv_ruv2 = self
            .ru_details
            .as_ref()
            .map_or(state.reported_tikv_ruv2, |details| details.TiKVRUV2());
        let current_tiflash_ru = self
            .ru_details
            .as_ref()
            .map_or(state.reported_tiflash_ru, |details| details.TiflashRU());

        // 仅在资源组名非空时上报正增量，避免空组或回退值产生负计费。
        if let Some(reporter) = self
            .reporter
            .as_ref()
            .filter(|_| !self.resource_group_name.is_empty())
        {
            let delta_tikv = current_tikv_ruv2 - state.reported_tikv_ruv2;
            let delta_tidb = current_tidb_ru - state.reported_tidb_ru;
            let delta_tiflash = current_tiflash_ru - state.reported_tiflash_ru;
            if delta_tikv > 0.0 || delta_tidb > 0.0 || delta_tiflash > 0.0 {
                reporter.ReportRUV2Consumption(
                    &self.resource_group_name,
                    delta_tikv.max(0.0),
                    delta_tidb.max(0.0),
                    delta_tiflash.max(0.0),
                );
            }
        }

        state.reported_tidb_ru = current_tidb_ru;
        state.reported_tikv_ruv2 = current_tikv_ruv2;
        state.reported_tiflash_ru = current_tiflash_ru;
    }
}

/// 将 RU 追踪器挂到结果集上。
pub fn AttachCursorRUV2Tracker(
    result_set: &mut dyn ResultSet,
    tracker: Option<Arc<CursorRUV2Tracker>>,
) {
    result_set.SetCursorRUV2Tracker(tracker);
}

/// 向结果集报告 chunk 单元格增量并触发 RU 增量上报。
pub fn ReportCursorRUV2Delta(result_set: &mut dyn ResultSet, result_chunk_cells_delta: i64) {
    result_set.ReportCursorRUV2Delta(result_chunk_cells_delta);
}

/// 由 sqlexec RecordSet 构造服务端 ResultSet。
pub fn New(
    record_set: Box<dyn sqlexec::RecordSet>,
    prepared_stmt: Option<PreparedStmtRef>,
) -> Box<dyn ResultSet> {
    Box::new(TidbResultSet::new(record_set, prepared_stmt))
}

/// TiDB 结果集实现：包装 RecordSet，缓存列信息，并用 finish_lock
/// 串行化 Next/NewChunk/Finish/Close。
pub struct TidbResultSet {
    record_set: Option<Box<dyn sqlexec::RecordSet>>,
    prepared_stmt: Option<PreparedStmtRef>,
    cursor_ruv2: Option<Arc<CursorRUV2Tracker>>,
    columns: Option<Vec<Arc<Info>>>,
    /// 与 Finish/Next 互斥的锁；Finish 使用 try_lock 避免阻塞拉取。
    finish_lock: Arc<Mutex<()>>,
    closed: AtomicBool,
}

impl TidbResultSet {
    /// 构造新结果集，初始未关闭且无列缓存。
    pub fn new(
        record_set: Box<dyn sqlexec::RecordSet>,
        prepared_stmt: Option<PreparedStmtRef>,
    ) -> Self {
        Self {
            record_set: Some(record_set),
            prepared_stmt,
            cursor_ruv2: None,
            columns: None,
            finish_lock: Arc::new(Mutex::new(())),
            closed: AtomicBool::new(false),
        }
    }

    /// 取得底层 RecordSet 不可变引用（Close 后 panic）。
    fn recordSet(&self) -> &dyn sqlexec::RecordSet {
        self.record_set
            .as_deref()
            .expect("result set used after Close")
    }

    /// 取得底层 RecordSet 可变引用（Close 后 panic）。
    fn recordSetMut(&mut self) -> &mut dyn sqlexec::RecordSet {
        self.record_set
            .as_deref_mut()
            .expect("result set used after Close")
    }

    /// 测试用：暴露 finish_lock 以便模拟锁竞争。
    #[cfg(test)]
    pub(crate) fn FinishLockForTest(&self) -> Arc<Mutex<()>> {
        Arc::clone(&self.finish_lock)
    }
}

impl ResultSet for TidbResultSet {
    fn Columns(&mut self) -> Vec<Arc<Info>> {
        // 优先本地缓存，其次预编译语句缓存，最后从 RecordSet.Fields 转换。
        if let Some(columns) = &self.columns {
            return columns.clone();
        }
        if let Some(columns) = self
            .prepared_stmt
            .as_ref()
            .and_then(|stmt| stmt.CachedColumnInfos())
        {
            self.columns = Some(columns);
            return self.columns.as_ref().expect("columns cached").clone();
        }

        let columns: Vec<_> = self
            .recordSet()
            .Fields()
            .iter()
            .map(|field| Arc::new(ConvertColumnInfo(field)))
            .collect();
        if let Some(stmt) = &self.prepared_stmt {
            stmt.CacheColumnInfos(columns.clone());
        }
        self.columns = Some(columns);
        self.columns.as_ref().expect("columns initialized").clone()
    }

    fn NewChunk(&mut self, allocator: Option<&mut dyn chunk::Allocator>) -> sqlexec::RecordChunk {
        let _guard = self
            .finish_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.record_set
            .as_deref()
            .expect("result set used after Close")
            .NewChunk(allocator)
    }

    fn Next(
        &mut self,
        ctx: &sqlexec::context::Context,
        req: &mut sqlexec::RecordChunk,
    ) -> Result<(), sqlexec::GoError> {
        let _guard = self
            .finish_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.record_set
            .as_deref_mut()
            .expect("result set used after Close")
            .Next(ctx, req)
    }

    fn Close(&mut self) {
        let _guard = self
            .finish_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // CAS 保证只关闭一次；底层 Close 错误经 Call 吞掉以免 panic。
        if self
            .closed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        if let Some(mut record_set) = self.record_set.take() {
            astersql_parser_terror::Call(move || record_set.Close());
        }
    }

    fn IsClosed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn FieldTypes(&self) -> Vec<Box<chunk::types::FieldType>> {
        self.recordSet()
            .Fields()
            .iter()
            .map(|field| {
                Box::new(
                    field
                        .column
                        .as_ref()
                        .expect("ResultField.column must be present")
                        .FieldType
                        .clone(),
                )
            })
            .collect()
    }

    fn SetPreparedStmt(&mut self, stmt: Option<PreparedStmtRef>) {
        self.prepared_stmt = stmt;
    }

    fn Finish(&mut self) -> Result<(), sqlexec::GoError> {
        // try_lock：若 Next 正持锁则直接返回 Ok，避免阻塞拉取路径。
        let _guard = match self.finish_lock.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => return Ok(()),
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        };
        match self.record_set.as_deref_mut() {
            Some(record_set) => record_set.Finish(),
            None => Ok(()),
        }
    }

    fn TryDetach(&mut self) -> Result<(Option<Box<dyn ResultSet>>, bool), sqlexec::GoError> {
        let (record_set, detached) = self.recordSetMut().TryDetach()?;
        if !detached {
            return Ok((None, false));
        }
        // 分离出的 RecordSet 继承预编译语句与已缓存列信息。
        let record_set = record_set.expect("detached RecordSet must be present");
        let mut detached_result = TidbResultSet::new(record_set, self.prepared_stmt.clone());
        detached_result.columns = self.columns.clone();
        Ok((Some(Box::new(detached_result)), true))
    }

    fn OnFetchReturned(&mut self) {
        self.recordSetMut().OnFetchReturned();
    }

    fn SetCursorRUV2Tracker(&mut self, tracker: Option<Arc<CursorRUV2Tracker>>) {
        self.cursor_ruv2 = tracker;
    }

    fn ReportCursorRUV2Delta(&mut self, result_chunk_cells_delta: i64) {
        if let Some(tracker) = &self.cursor_ruv2 {
            tracker.addResultChunkCells(result_chunk_cells_delta);
            tracker.reportDelta();
        }
    }
}
