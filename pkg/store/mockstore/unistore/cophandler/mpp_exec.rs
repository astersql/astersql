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

// MPP 下推执行器树的本地执行实现。
//
// 对应 Go `mppExec`：按执行计划（Executor）递归执行 Table/IndexScan、Selection、
// Limit、TopN、Projection、Expand、Aggregation、Join 与 Exchange 分区逻辑，
// 产出行集、范围计数、NDV（Distinct 基数估计）与执行摘要。

use crate::cop_handler::{
    AggCall, AggKind, CopError, Datum, ExecutionSummary, Executor, Expr, JoinType, KeyRange,
    KvReader, Row, duration_summary,
};
use crate::topn::TopNHeap;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, Instant};

/// MaterializedExec 每次 `next` 返回的最大行数。
pub const BATCH_SIZE: usize = 1024;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 执行输出：结果行、范围计数、NDV、摘要与中间分区行。
pub struct ExecutionOutput {
    pub rows: Vec<Row>,
    pub range_counts: Vec<i64>,
    pub ndvs: Vec<i64>,
    pub summaries: Vec<ExecutionSummary>,
    pub intermediate: Vec<Vec<Row>>,
}

impl ExecutionOutput {
    /// 替换结果行并追加本次耗时摘要。
    fn map_rows(mut self, rows: Vec<Row>, started: Instant) -> Self {
        self.summaries
            .push(duration_summary(started.elapsed(), rows.len()));
        self.rows = rows;
        self
    }
}

/// Pull-executor interface corresponding to the Go `mppExec` contract.
/// 拉取式执行器接口，对应 Go `mppExec`：open/next/stop 与执行摘要。
pub trait MppExec {
    fn open(&mut self) -> Result<(), CopError>;
    fn next(&mut self) -> Result<Option<Vec<Row>>, CopError>;
    fn stop(&mut self) -> Result<(), CopError>;
    fn execution_summary(&self) -> ExecutionSummary;
}

/// 将已物化行集按 BATCH_SIZE 分批吐出的简单执行器。
pub struct MaterializedExec {
    rows: Vec<Row>,
    cursor: usize,
    opened: bool,
    started: Option<Instant>,
}

impl MaterializedExec {
    /// 用完整行集构造物化执行器。
    pub fn new(rows: Vec<Row>) -> Self {
        Self {
            rows,
            cursor: 0,
            opened: false,
            started: None,
        }
    }
}

impl MppExec for MaterializedExec {
    /// 打开执行器并记录起始时间。
    fn open(&mut self) -> Result<(), CopError> {
        self.opened = true;
        self.started = Some(Instant::now());
        Ok(())
    }
    /// 返回下一批行；耗尽则 None。
    fn next(&mut self) -> Result<Option<Vec<Row>>, CopError> {
        if !self.opened {
            return Err(CopError::InvalidRequest("executor is not open".into()));
        }
        if self.cursor >= self.rows.len() {
            return Ok(None);
        }
        let end = (self.cursor + BATCH_SIZE).min(self.rows.len());
        let rows = self.rows[self.cursor..end].to_vec();
        self.cursor = end;
        Ok(Some(rows))
    }
    /// 关闭执行器。
    fn stop(&mut self) -> Result<(), CopError> {
        self.opened = false;
        Ok(())
    }
    /// 返回自 open 以来的耗时与已产出行数摘要。
    fn execution_summary(&self) -> ExecutionSummary {
        duration_summary(
            self.started.map_or(Duration::ZERO, |time| time.elapsed()),
            self.cursor,
        )
    }
}

/// 按执行计划树递归本地执行，返回聚合后的 `ExecutionOutput`。
pub fn execute_executor(
    reader: &dyn KvReader,
    ranges: &[KeyRange],
    start_ts: u64,
    executor: &Executor,
) -> Result<ExecutionOutput, CopError> {
    let started = Instant::now();
    match executor {
        Executor::TableScan {
            columns,
            descending,
        } => scan(reader, ranges, start_ts, columns, *descending, false),
        Executor::IndexScan {
            columns,
            descending,
            ..
        } => scan(reader, ranges, start_ts, columns, *descending, true),
        // Selection：按条件过滤子节点行。
        Executor::Selection { condition, child } => {
            let output = execute_executor(reader, ranges, start_ts, child)?;
            let rows = output
                .rows
                .iter()
                .filter_map(|row| match condition.eval(row) {
                    Ok(value) if value.truthy() => Some(Ok(row.clone())),
                    Ok(_) => None,
                    Err(error) => Some(Err(error)),
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(output.map_rows(rows, started))
        }
        // Limit：截断子节点结果到前 limit 行。
        Executor::Limit { limit, child } => {
            let mut output = execute_executor(reader, ranges, start_ts, child)?;
            output.rows.truncate(*limit);
            let rows = std::mem::take(&mut output.rows);
            Ok(output.map_rows(rows, started))
        }
        // TopN：用有界堆保留最优 limit 行后排序输出。
        Executor::TopN {
            limit,
            order_by,
            child,
        } => {
            let mut output = execute_executor(reader, ranges, start_ts, child)?;
            let mut heap = TopNHeap::new(*limit, order_by.clone());
            for row in std::mem::take(&mut output.rows) {
                heap.add_data_row(row)?;
            }
            let rows = heap.into_sorted_rows()?;
            Ok(output.map_rows(rows, started))
        }
        // Projection：对每行求表达式列表得到新行。
        Executor::Projection { expressions, child } => {
            let output = execute_executor(reader, ranges, start_ts, child)?;
            let rows = output
                .rows
                .iter()
                .map(|row| expressions.iter().map(|expr| expr.eval(row)).collect())
                .collect::<Result<Vec<Row>, _>>()?;
            Ok(output.map_rows(rows, started))
        }
        // Expand：按 levels 展开分组集（CUBE/ROLLUP 风格），缺失列为 NULL。
        Executor::Expand { levels, child } => {
            let output = execute_executor(reader, ranges, start_ts, child)?;
            let mut rows = Vec::with_capacity(output.rows.len().saturating_mul(levels.len()));
            for row in &output.rows {
                for level in levels {
                    rows.push(
                        level
                            .iter()
                            .map(|offset| {
                                offset
                                    .and_then(|offset| row.get(offset).cloned())
                                    .unwrap_or(Datum::Null)
                            })
                            .collect(),
                    );
                }
            }
            Ok(output.map_rows(rows, started))
        }
        // Aggregation：按 group_by 分组并计算聚合函数。
        Executor::Aggregation {
            group_by,
            calls,
            child,
            stream,
        } => {
            let mut output = execute_executor(reader, ranges, start_ts, child)?;
            let rows = aggregate(std::mem::take(&mut output.rows), group_by, calls, *stream)?;
            Ok(output.map_rows(rows, started))
        }
        // Join：分别执行左右子树后做哈希连接。
        Executor::Join {
            join_type,
            left_key,
            right_key,
            left,
            right,
        } => {
            let mut left_output = execute_executor(reader, ranges, start_ts, left)?;
            let right_output = execute_executor(reader, ranges, start_ts, right)?;
            left_output.summaries.extend(right_output.summaries);
            let rows = join(
                &left_output.rows,
                &right_output.rows,
                executor_output_width(right),
                join_type,
                left_key,
                right_key,
            )?;
            Ok(left_output.map_rows(rows, started))
        }
        // ExchangeSender：若有分区键则按键分桶写入 intermediate。
        Executor::ExchangeSender {
            partition_keys,
            child,
            ..
        } => {
            let mut output = execute_executor(reader, ranges, start_ts, child)?;
            if !partition_keys.is_empty() {
                let mut partitions = BTreeMap::<Vec<Datum>, Vec<Row>>::new();
                for row in std::mem::take(&mut output.rows) {
                    let key = partition_keys
                        .iter()
                        .map(|expr| expr.eval(&row))
                        .collect::<Result<Vec<_>, _>>()?;
                    partitions.entry(key).or_default().push(row);
                }
                output.intermediate = partitions.values().cloned().collect();
                output.rows = partitions.into_values().flatten().collect();
            }
            output
                .summaries
                .push(duration_summary(started.elapsed(), output.rows.len()));
            Ok(output)
        }
        Executor::ExchangeReceiver { .. } => Err(CopError::Unsupported(
            "exchange receiver requires an MPP task context",
        )),
    }
}

/// 从计划 schema 推导输出列数；ExchangeReceiver 的 schema 不在精简计划节点中携带。
fn executor_output_width(executor: &Executor) -> Option<usize> {
    match executor {
        Executor::TableScan { columns, .. } | Executor::IndexScan { columns, .. } => {
            Some(columns.len())
        }
        Executor::Selection { child, .. }
        | Executor::Limit { child, .. }
        | Executor::TopN { child, .. }
        | Executor::ExchangeSender { child, .. } => executor_output_width(child),
        Executor::Projection { expressions, .. } => Some(expressions.len()),
        Executor::Expand { levels, .. } => Some(levels.first().map_or(0, Vec::len)),
        Executor::Aggregation {
            group_by, calls, ..
        } => Some(calls.len() + group_by.len()),
        Executor::Join {
            join_type,
            left,
            right,
            ..
        } => {
            let left_width = executor_output_width(left)?;
            match join_type {
                JoinType::Inner | JoinType::LeftOuter => {
                    Some(left_width + executor_output_width(right)?)
                }
                JoinType::Semi | JoinType::AntiSemi => Some(left_width),
            }
        }
        Executor::ExchangeReceiver { .. } => None,
    }
}

/// 按 ranges 扫描 KV：投影列、可选降序，索引扫描时统计 NDV。
fn scan(
    reader: &dyn KvReader,
    ranges: &[KeyRange],
    start_ts: u64,
    columns: &[usize],
    descending: bool,
    index_scan: bool,
) -> Result<ExecutionOutput, CopError> {
    let started = Instant::now();
    let mut rows = Vec::new();
    let mut counts = Vec::with_capacity(ranges.len());
    let mut ndvs = Vec::with_capacity(ranges.len());
    for range in ranges {
        let pairs = reader.scan(std::slice::from_ref(range), start_ts, descending)?;
        counts.push(pairs.len() as i64);
        let mut distinct = BTreeSet::new();
        for pair in pairs {
            let projected = columns
                .iter()
                .map(|offset| {
                    pair.value
                        .get(*offset)
                        .cloned()
                        .ok_or(CopError::ColumnOffset(*offset))
                })
                .collect::<Result<Row, _>>()?;
            if index_scan {
                distinct.insert(projected.clone());
            }
            rows.push(projected);
        }
        ndvs.push(if index_scan { distinct.len() as i64 } else { 0 });
    }
    Ok(ExecutionOutput {
        summaries: vec![duration_summary(started.elapsed(), rows.len())],
        rows,
        range_counts: counts,
        ndvs,
        intermediate: Vec::new(),
    })
}

#[derive(Clone, Debug)]
/// 聚合状态机：COUNT/SUM/MIN/MAX/FIRST。
enum AggState {
    Count(u64),
    Sum(Option<Datum>),
    Min(Option<Datum>),
    Max(Option<Datum>),
    First(Option<Datum>),
}

impl AggState {
    /// 按聚合种类初始化空状态。
    fn new(kind: &AggKind) -> Self {
        match kind {
            AggKind::Count => Self::Count(0),
            AggKind::Sum => Self::Sum(None),
            AggKind::Min => Self::Min(None),
            AggKind::Max => Self::Max(None),
            AggKind::First => Self::First(None),
        }
    }
    /// 用新值更新聚合状态（NULL 按种类选择性忽略）。
    fn update(&mut self, value: Datum) -> Result<(), CopError> {
        match self {
            Self::Count(count) => {
                if !matches!(value, Datum::Null) {
                    *count += 1;
                }
            }
            Self::Sum(sum) => {
                if matches!(value, Datum::Null) {
                    return Ok(());
                }
                *sum = Some(match sum.take() {
                    None => value,
                    Some(current) => sum_datum(current, value)?,
                });
            }
            Self::Min(min) => {
                if !matches!(value, Datum::Null) && min.as_ref().is_none_or(|old| value < *old) {
                    *min = Some(value);
                }
            }
            Self::Max(max) => {
                if !matches!(value, Datum::Null) && max.as_ref().is_none_or(|old| value > *old) {
                    *max = Some(value);
                }
            }
            Self::First(first) => {
                if first.is_none() {
                    *first = Some(value);
                }
            }
        }
        Ok(())
    }
    /// 结束聚合并输出最终 Datum。
    fn finish(self) -> Datum {
        match self {
            Self::Count(value) => Datum::Uint(value),
            Self::Sum(value) | Self::Min(value) | Self::Max(value) | Self::First(value) => {
                value.unwrap_or(Datum::Null)
            }
        }
    }
}

/// 数值 SUM：同型相加，或非负 Int 与 Uint 混合。
fn sum_datum(left: Datum, right: Datum) -> Result<Datum, CopError> {
    match (left, right) {
        (Datum::Int(a), Datum::Int(b)) => Ok(Datum::Int(a.saturating_add(b))),
        (Datum::Uint(a), Datum::Uint(b)) => Ok(Datum::Uint(a.saturating_add(b))),
        (Datum::Real(a), Datum::Real(b)) => Ok(Datum::Real(a + b)),
        (Datum::Int(a), Datum::Uint(b)) | (Datum::Uint(b), Datum::Int(a)) if a >= 0 => {
            Ok(Datum::Uint((a as u64).saturating_add(b)))
        }
        _ => Err(CopError::Type("SUM expects numeric values".into())),
    }
}

/// 分组聚合：stream 模式按有序键切换分组，否则用哈希表累积。
fn aggregate(
    rows: Vec<Row>,
    group_by: &[Expr],
    calls: &[AggCall],
    stream: bool,
) -> Result<Vec<Row>, CopError> {
    // StreamAgg：要求输入已按分组键有序，遇新键则输出上一组。
    if stream {
        let mut output = Vec::new();
        let mut current_key: Option<Vec<Datum>> = None;
        let mut states = Vec::new();
        for row in rows {
            let key = group_by
                .iter()
                .map(|expr| expr.eval(&row))
                .collect::<Result<Vec<_>, _>>()?;
            if current_key.as_ref().is_some_and(|old| *old != key) {
                output.push(finish_group(
                    current_key.take().unwrap(),
                    std::mem::take(&mut states),
                ));
            }
            if current_key.is_none() {
                current_key = Some(key);
                states = calls.iter().map(|call| AggState::new(&call.kind)).collect();
            }
            update_states(&mut states, calls, &row)?;
        }
        if let Some(key) = current_key {
            output.push(finish_group(key, states));
        }
        if output.is_empty() && group_by.is_empty() {
            output.push(finish_group(
                Vec::new(),
                calls.iter().map(|call| AggState::new(&call.kind)).collect(),
            ));
        }
        return Ok(output);
    }
    // HashAgg：用 BTreeMap 累积状态，并像 Go 的 groupKeys 一样保留分组首见顺序。
    let mut groups = BTreeMap::<Vec<Datum>, Vec<AggState>>::new();
    let mut group_keys = Vec::new();
    for row in rows {
        let key = group_by
            .iter()
            .map(|expr| expr.eval(&row))
            .collect::<Result<Vec<_>, _>>()?;
        if !groups.contains_key(&key) {
            group_keys.push(key.clone());
        }
        let states = groups
            .entry(key)
            .or_insert_with(|| calls.iter().map(|call| AggState::new(&call.kind)).collect());
        update_states(states, calls, &row)?;
    }
    if groups.is_empty() && group_by.is_empty() {
        let key = Vec::new();
        groups.insert(
            key.clone(),
            calls.iter().map(|call| AggState::new(&call.kind)).collect(),
        );
        group_keys.push(key);
    }
    Ok(group_keys
        .into_iter()
        .map(|key| {
            let states = groups
                .remove(&key)
                .expect("group key and aggregation state must stay in sync");
            finish_group(key, states)
        })
        .collect())
}

/// 对每个 AggCall 求值并更新对应状态。
fn update_states(
    states: &mut [AggState],
    calls: &[AggCall],
    row: &[Datum],
) -> Result<(), CopError> {
    for (state, call) in states.iter_mut().zip(calls) {
        let value = match &call.expr {
            Some(expr) => expr.eval(row)?,
            None => Datum::Int(1),
        };
        state.update(value)?;
    }
    Ok(())
}

/// 将各聚合结果与分组键拼接为一行；Go 的输出 schema 固定为聚合列在前。
fn finish_group(key: Vec<Datum>, states: Vec<AggState>) -> Row {
    let mut row = states.into_iter().map(AggState::finish).collect::<Row>();
    row.extend(key);
    row
}

/// 哈希 Join：右表建哈希，左表探测；支持 Inner/LeftOuter/Semi/AntiSemi。
fn join(
    left: &[Row],
    right: &[Row],
    right_schema_width: Option<usize>,
    join_type: &JoinType,
    left_key: &Expr,
    right_key: &Expr,
) -> Result<Vec<Row>, CopError> {
    let mut hash = BTreeMap::<Datum, Vec<&Row>>::new();
    for row in right {
        let key = right_key.eval(row)?;
        if !matches!(key, Datum::Null) {
            hash.entry(key).or_default().push(row);
        }
    }
    let right_width = right_schema_width
        .or_else(|| right.first().map(Vec::len))
        .unwrap_or(0);
    let mut output = Vec::new();
    for left_row in left {
        let key = left_key.eval(left_row)?;
        let matches = hash.get(&key);
        match join_type {
            // Inner：仅输出左右键匹配的拼接行。
            JoinType::Inner => {
                if let Some(matches) = matches {
                    for right_row in matches {
                        let mut row = left_row.clone();
                        row.extend((*right_row).clone());
                        output.push(row);
                    }
                }
            }
            // LeftOuter：无匹配时右半填充 NULL。
            JoinType::LeftOuter => match matches {
                Some(matches) => {
                    for right_row in matches {
                        let mut row = left_row.clone();
                        row.extend((*right_row).clone());
                        output.push(row);
                    }
                }
                None => {
                    let mut row = left_row.clone();
                    row.extend(std::iter::repeat_n(Datum::Null, right_width));
                    output.push(row);
                }
            },
            // Semi：存在匹配则只输出左行。
            JoinType::Semi => {
                if matches.is_some() {
                    output.push(left_row.clone());
                }
            }
            // AntiSemi：无匹配才输出左行。
            JoinType::AntiSemi => {
                if matches.is_none() {
                    output.push(left_row.clone());
                }
            }
        }
    }
    Ok(output)
}

#[derive(Default)]
/// 按 task_id 缓冲 Exchange 行包的简易内存通道。
pub struct ExchangeBuffer {
    packets: HashMap<i64, Vec<Vec<Row>>>,
}

impl ExchangeBuffer {
    /// 向指定 task 追加一批行。
    pub fn send(&mut self, task_id: i64, rows: Vec<Row>) {
        self.packets.entry(task_id).or_default().push(rows);
    }
    /// 取出并清空指定 task 的全部缓冲行。
    pub fn receive(&mut self, task_id: i64) -> Vec<Row> {
        self.packets
            .remove(&task_id)
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .collect()
    }
}
