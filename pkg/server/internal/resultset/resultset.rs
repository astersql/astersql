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

// 查询结果集核心实现与游标响应字节同步。
//
// `TidbResultSet` 包装 sqlexec::RecordSet，负责列信息缓存、Finish/Close
// 生命周期与可分离（TryDetach）结果集；`CursorRUV2Tracker` 在游标 fetch
// 过程中同步 TiKV coprocessor response bytes。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, TryLockError};

use astersql_planner_core::PlanCacheStmt;
use astersql_server_internal_column::{ConvertColumnInfo, Info};
use astersql_util_chunk as chunk;
use astersql_util_execdetails::ruv2_metrics::{
    RUV2Metrics, SyncRUV2MetricsFromRUDetails, tikvutil,
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
    /// 同步游标读取期间新增的 TiKV coprocessor response bytes。
    fn ReportCursorRUV2Delta(&mut self);
}

/// 游标执行期间的 RUv2 指标追踪器，只同步响应字节。
pub struct CursorRUV2Tracker {
    metrics: Arc<RUV2Metrics>,
    ru_details: Arc<tikvutil::RUDetails>,
    state: Mutex<()>,
}

/// 构造游标响应字节追踪器，缺少任一输入或 bypass 时返回 None。
pub fn NewCursorRUV2Tracker(
    metrics: Option<Arc<RUV2Metrics>>,
    ru_details: Option<Arc<tikvutil::RUDetails>>,
) -> Option<Arc<CursorRUV2Tracker>> {
    let (Some(metrics), Some(ru_details)) = (metrics, ru_details) else {
        return None;
    };
    if metrics.Bypass() {
        return None;
    }
    SyncRUV2MetricsFromRUDetails(Some(&metrics), Some(&ru_details));
    Some(Arc::new(CursorRUV2Tracker {
        metrics,
        ru_details,
        state: Mutex::new(()),
    }))
}

impl CursorRUV2Tracker {
    /// 串行排空并转移游标 fetch 期间新增的响应字节。
    fn reportDelta(&self) {
        let _guard = self
            .state
            .lock()
            .expect("cursor RUv2 tracker lock poisoned");
        SyncRUV2MetricsFromRUDetails(Some(&self.metrics), Some(&self.ru_details));
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
pub fn ReportCursorRUV2Delta(result_set: &mut dyn ResultSet) {
    result_set.ReportCursorRUV2Delta();
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

    fn ReportCursorRUV2Delta(&mut self) {
        if let Some(tracker) = &self.cursor_ruv2 {
            tracker.reportDelta();
        }
    }
}
