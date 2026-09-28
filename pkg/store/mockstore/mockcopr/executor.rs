// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 协处理器 DAG 执行器：表/索引扫描、Selection、TopN、Limit 等算子。
//
// 各算子通过 `executor` trait 链式串联（下游 `SetSrcExec` 指向上游），
// `Next` 拉取一行；扫描算子按 `start_ts` 做 MVCC（多版本并发控制）可见性读取。

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use crate::copr_handler::{ByItem, CopError, Datum, ExecDetail, Expr, KeyRange, KvReader, Row};
use crate::topn::{sortRow, topNHeap};

/// 一行的列值向量别名。
pub type RowValue = Row;
/// `Next` 返回值：有行或扫描结束（None）。
pub type NextRow = Option<RowValue>;

/// 协处理器算子统一接口：上下游链接、计数、拉取行与执行详情。
pub trait executor: Send {
    fn SetSrcExec(&mut self, source: Option<Box<dyn executor>>);
    fn GetSrcExec(&self) -> Option<&dyn executor>;
    fn ResetCounts(&mut self);
    fn Counts(&self) -> Vec<i64>;
    fn Next(&mut self) -> Result<NextRow, CopError>;
    fn ExecDetails(&self) -> Vec<ExecDetail>;
}

/// 累加一次 `Next` 调用的耗时、迭代次数与产出行数。
fn update_detail(detail: &mut ExecDetail, begin: Instant, row: &NextRow) {
    detail.time_processed += begin.elapsed();
    detail.iterations += 1;
    if row.is_some() {
        detail.produced_rows += 1;
    }
}

/// Go updates execution details with `defer`, so failed `Next` calls are iterations too.
fn update_result_detail(
    detail: &mut ExecDetail,
    begin: Instant,
    result: &Result<NextRow, CopError>,
) {
    match result {
        Ok(row) => update_detail(detail, begin, row),
        Err(_) => update_detail(detail, begin, &None),
    }
}

impl ExecDetail {
    /// 便捷包装：用开始时刻与产出行更新本算子详情。
    pub fn update(&mut self, begin: Instant, row: &NextRow) {
        update_detail(self, begin, row);
    }
}

/// 收集上游详情后再追加本算子详情，形成从叶到根的链路。
fn append_details(source: Option<&dyn executor>, own: &ExecDetail) -> Vec<ExecDetail> {
    let mut details = source.map(executor::ExecDetails).unwrap_or_default();
    details.push(own.clone());
    details
}

/// 表扫描执行器：按键范围从 `KvReader` 拉取行并缓存。
pub struct tableScanExec {
    pub reader: Arc<dyn KvReader>,
    pub ranges: Vec<KeyRange>,
    pub start_ts: u64,
    pub descending: bool,
    pub rows: VecDeque<Row>,
    row_range_indexes: VecDeque<Option<usize>>,
    pub loaded: bool,
    pub cursor: usize,
    pub counts: Vec<i64>,
    pub exec_detail: ExecDetail,
    pub src: Option<Box<dyn executor>>,
}

impl tableScanExec {
    /// 为每个 range 预分配计数槽，尚未真正扫描。
    pub fn new(
        reader: Arc<dyn KvReader>,
        ranges: Vec<KeyRange>,
        start_ts: u64,
        descending: bool,
    ) -> Self {
        Self {
            counts: vec![0; ranges.len()],
            reader,
            ranges,
            start_ts,
            descending,
            rows: VecDeque::new(),
            row_range_indexes: VecDeque::new(),
            loaded: false,
            cursor: 0,
            exec_detail: ExecDetail::default(),
            src: None,
        }
    }

    /// 惰性加载：首次调用时按 ranges 扫描，并记录每行所属 range。
    fn load(&mut self) -> Result<(), CopError> {
        if self.loaded {
            return Ok(());
        }
        let pairs = self
            .reader
            .scan(&self.ranges, self.start_ts, self.descending)?;
        for pair in pairs {
            // 将键归入对应 KeyRange，累加该 range 的命中计数。
            let range_index = self.ranges.iter().position(|range| {
                pair.key >= range.start && (range.end.is_empty() || pair.key < range.end)
            });
            self.row_range_indexes.push_back(range_index);
            self.rows.push_back(pair.value);
        }
        self.loaded = true;
        Ok(())
    }

    /// 点查：对单点 range 扫描并取第一行。
    pub fn getRowFromPoint(&self, range: &KeyRange) -> Result<NextRow, CopError> {
        Ok(self
            .reader
            .scan(std::slice::from_ref(range), self.start_ts, self.descending)?
            .into_iter()
            .next()
            .map(|pair| pair.value))
    }

    /// 范围扫描：返回该 range 内全部行。
    pub fn getRowFromRange(&self, range: &KeyRange) -> Result<Vec<Row>, CopError> {
        Ok(self
            .reader
            .scan(std::slice::from_ref(range), self.start_ts, self.descending)?
            .into_iter()
            .map(|pair| pair.value)
            .collect())
    }
}

impl executor for tableScanExec {
    fn SetSrcExec(&mut self, source: Option<Box<dyn executor>>) {
        self.src = source;
    }
    fn GetSrcExec(&self) -> Option<&dyn executor> {
        self.src.as_deref()
    }
    fn ResetCounts(&mut self) {
        self.counts.fill(0);
    }
    fn Counts(&self) -> Vec<i64> {
        self.counts.clone()
    }
    fn ExecDetails(&self) -> Vec<ExecDetail> {
        append_details(self.src.as_deref(), &self.exec_detail)
    }

    fn Next(&mut self) -> Result<NextRow, CopError> {
        let begin = Instant::now();
        let result = (|| {
            self.load()?;
            let row = self.rows.pop_front();
            if row.is_some() {
                if let Some(Some(range_index)) = self.row_range_indexes.pop_front() {
                    self.counts[range_index] += 1;
                }
                self.cursor += 1;
            }
            Ok(row)
        })();
        update_result_detail(&mut self.exec_detail, begin, &result);
        result
    }
}

/// 索引扫描：复用表扫描实现，额外标记是否唯一索引。
pub struct indexScanExec {
    pub inner: tableScanExec,
    pub unique: bool,
}

impl indexScanExec {
    /// 构造索引扫描，`unique` 表示唯一索引路径。
    pub fn new(
        reader: Arc<dyn KvReader>,
        ranges: Vec<KeyRange>,
        start_ts: u64,
        descending: bool,
        unique: bool,
    ) -> Self {
        Self {
            inner: tableScanExec::new(reader, ranges, start_ts, descending),
            unique,
        }
    }
    /// 是否唯一索引扫描。
    pub fn isUnique(&self) -> bool {
        self.unique
    }
    /// 委托内层点查。
    pub fn getRowFromPoint(&self, range: &KeyRange) -> Result<NextRow, CopError> {
        self.inner.getRowFromPoint(range)
    }
    /// 委托内层范围扫描。
    pub fn getRowFromRange(&self, range: &KeyRange) -> Result<Vec<Row>, CopError> {
        self.inner.getRowFromRange(range)
    }
}

impl executor for indexScanExec {
    fn SetSrcExec(&mut self, source: Option<Box<dyn executor>>) {
        self.inner.SetSrcExec(source);
    }
    fn GetSrcExec(&self) -> Option<&dyn executor> {
        self.inner.GetSrcExec()
    }
    fn ResetCounts(&mut self) {
        self.inner.ResetCounts();
    }
    fn Counts(&self) -> Vec<i64> {
        self.inner.Counts()
    }
    fn Next(&mut self) -> Result<NextRow, CopError> {
        self.inner.Next()
    }
    fn ExecDetails(&self) -> Vec<ExecDetail> {
        self.inner.ExecDetails()
    }
}

/// Selection（过滤）算子：对上游行求条件表达式，仅放行为真的行。
pub struct selectionExec {
    pub conditions: Vec<Expr>,
    pub src: Option<Box<dyn executor>>,
    pub exec_detail: ExecDetail,
}

impl selectionExec {
    /// 用一组过滤表达式构造 Selection。
    pub fn new(conditions: Vec<Expr>) -> Self {
        Self {
            conditions,
            src: None,
            exec_detail: ExecDetail::default(),
        }
    }
}

/// 对行求值所有条件：遇 NULL 或假即返回 false（AND 语义）。
pub fn evalBool(expressions: &[Expr], row: &[Datum]) -> Result<bool, CopError> {
    for expression in expressions {
        let value = expression.eval(row)?;
        if matches!(value, Datum::Null) || !value.truthy() {
            return Ok(false);
        }
    }
    Ok(true)
}

impl executor for selectionExec {
    fn SetSrcExec(&mut self, source: Option<Box<dyn executor>>) {
        self.src = source;
    }
    fn GetSrcExec(&self) -> Option<&dyn executor> {
        self.src.as_deref()
    }
    fn ResetCounts(&mut self) {
        if let Some(source) = self.src.as_mut() {
            source.ResetCounts();
        }
    }
    fn Counts(&self) -> Vec<i64> {
        self.src
            .as_ref()
            .map(|source| source.Counts())
            .unwrap_or_default()
    }
    fn ExecDetails(&self) -> Vec<ExecDetail> {
        append_details(self.src.as_deref(), &self.exec_detail)
    }

    fn Next(&mut self) -> Result<NextRow, CopError> {
        let begin = Instant::now();
        // 循环拉取上游，直到条件为真或上游耗尽。
        let result = (|| loop {
            let row = self
                .src
                .as_mut()
                .ok_or_else(|| CopError::InvalidRequest("selection has no source".into()))?
                .Next()?;
            let Some(row) = row else {
                break Ok(None);
            };
            if evalBool(&self.conditions, &row)? {
                break Ok(Some(row));
            }
        })();
        update_result_detail(&mut self.exec_detail, begin, &result);
        result
    }
}

/// TopN 算子：先耗尽上游填堆，再按序吐出最多 `limit` 行。
pub struct topNExec {
    pub order_by: Vec<ByItem>,
    pub limit: usize,
    pub src: Option<Box<dyn executor>>,
    pub rows: VecDeque<Row>,
    pub executed: bool,
    pub working_heap: Option<topNHeap>,
    pub exec_detail: ExecDetail,
}

impl topNExec {
    /// 构造尚未执行的 TopN（ORDER BY + LIMIT）。
    pub fn new(order_by: Vec<ByItem>, limit: usize) -> Self {
        Self {
            order_by,
            limit,
            src: None,
            rows: VecDeque::new(),
            executed: false,
            working_heap: None,
            exec_detail: ExecDetail::default(),
        }
    }

    /// 从上游取一行并尝试加入堆；无更多行时返回 false。
    pub fn innerNext(&mut self) -> Result<bool, CopError> {
        let row = self
            .src
            .as_mut()
            .ok_or_else(|| CopError::InvalidRequest("top-n has no source".into()))?
            .Next()?;
        let Some(row) = row else {
            return Ok(false);
        };
        self.evalTopN(row)?;
        Ok(true)
    }

    /// 计算 ORDER BY 键并尝试将行加入工作堆。
    pub fn evalTopN(&mut self, row: Row) -> Result<(), CopError> {
        let key = self
            .order_by
            .iter()
            .map(|item| item.expr.eval(&row))
            .collect::<Result<Vec<_>, _>>()?;
        self.working_heap
            .as_mut()
            .ok_or_else(|| CopError::InvalidRequest("top-n heap is not initialized".into()))?
            .tryToAddRow(sortRow { key, data: row });
        Ok(())
    }
}

impl executor for topNExec {
    fn SetSrcExec(&mut self, source: Option<Box<dyn executor>>) {
        self.src = source;
    }
    fn GetSrcExec(&self) -> Option<&dyn executor> {
        self.src.as_deref()
    }
    fn ResetCounts(&mut self) {
        if let Some(source) = self.src.as_mut() {
            source.ResetCounts();
        }
    }
    fn Counts(&self) -> Vec<i64> {
        self.src
            .as_ref()
            .map(|source| source.Counts())
            .unwrap_or_default()
    }
    fn ExecDetails(&self) -> Vec<ExecDetail> {
        append_details(self.src.as_deref(), &self.exec_detail)
    }

    fn Next(&mut self) -> Result<NextRow, CopError> {
        let begin = Instant::now();
        let result = (|| {
            // 首次调用：建堆、吞掉上游、转成有序结果队列。
            if !self.executed {
                self.working_heap = Some(topNHeap::new(self.limit, self.order_by.clone()));
                while self.innerNext()? {}
                self.rows = self
                    .working_heap
                    .take()
                    .expect("top-n heap was initialized")
                    .intoSortedRows()?
                    .into();
                self.executed = true;
            }
            Ok(self.rows.pop_front())
        })();
        update_result_detail(&mut self.exec_detail, begin, &result);
        result
    }
}

/// Limit 算子：最多向上游拉取 `limit` 行后返回结束。
pub struct limitExec {
    pub limit: usize,
    pub cursor: usize,
    pub src: Option<Box<dyn executor>>,
    pub exec_detail: ExecDetail,
}

impl limitExec {
    /// 构造限制为 `limit` 行的 Limit。
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            cursor: 0,
            src: None,
            exec_detail: ExecDetail::default(),
        }
    }
}

impl executor for limitExec {
    fn SetSrcExec(&mut self, source: Option<Box<dyn executor>>) {
        self.src = source;
    }
    fn GetSrcExec(&self) -> Option<&dyn executor> {
        self.src.as_deref()
    }
    fn ResetCounts(&mut self) {
        if let Some(source) = self.src.as_mut() {
            source.ResetCounts();
        }
    }
    fn Counts(&self) -> Vec<i64> {
        self.src
            .as_ref()
            .map(|source| source.Counts())
            .unwrap_or_default()
    }
    fn ExecDetails(&self) -> Vec<ExecDetail> {
        append_details(self.src.as_deref(), &self.exec_detail)
    }

    fn Next(&mut self) -> Result<NextRow, CopError> {
        let begin = Instant::now();
        let result = (|| {
            if self.cursor >= self.limit {
                return Ok(None);
            }
            let row = self
                .src
                .as_mut()
                .ok_or_else(|| CopError::InvalidRequest("limit has no source".into()))?
                .Next()?;
            if row.is_some() {
                self.cursor += 1;
            }
            Ok(row)
        })();
        update_result_detail(&mut self.exec_detail, begin, &result);
        result
    }
}

/// 判断行数据是否已包含指定列 ID 的值。
pub fn hasColVal(data: &[Datum], column_ids: &HashMap<i64, usize>, id: i64) -> bool {
    column_ids
        .get(&id)
        .and_then(|offset| data.get(*offset))
        .is_some_and(|value| !matches!(value, Datum::Null))
}

/// 按投影列 ID 组装行：`-1` 表示 handle（行主键/隐式句柄）列。
pub fn getRowData(
    columns: &[i64],
    column_ids: &HashMap<i64, usize>,
    handle: Datum,
    values: &[Datum],
) -> Result<Row, CopError> {
    columns
        .iter()
        .map(|column_id| {
            if *column_id == -1 {
                return Ok(handle.clone());
            }
            column_ids
                .get(column_id)
                .and_then(|offset| values.get(*offset))
                .cloned()
                .ok_or(CopError::ColumnOffset(
                    *column_ids.get(column_id).unwrap_or(&usize::MAX),
                ))
        })
        .collect()
}

/// 将表达式切片克隆为拥有所有权的向量（对齐 Go convertToExprs）。
pub fn convertToExprs(expressions: &[Expr]) -> Vec<Expr> {
    expressions.to_vec()
}
