// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Executes the narrow physical tree emitted for table TopN queries.
//
// Storage remains owned by the caller.  This adapter translates canonical
// physical operators into the existing executor implementations; in
// particular it never sorts rows itself.
//
// 执行窄物理计划树（如表 TopN 查询）的适配运行时。
//
// 存储由调用方持有。本适配器将规范物理算子翻译为已有执行器实现；
// 自身不排序。物理计划（physical plan）是优化器选定的可执行算子树。

#![allow(non_snake_case)]

use std::cmp::Ordering as RowOrdering;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::sync::atomic::{AtomicUsize, Ordering};

use astersql_executor_sortexec::{DataChunk, Limit, Row, SortExec, SortKey, SortValue, TopNExec};
use astersql_expression::Expression as _;
use astersql_expression_aggregation::{CompleteMode, FinalMode, Partial1Mode, Partial2Mode};
use astersql_planner_core_base::PhysicalPlan;
use astersql_planner_core_operator_physicalop::{
    BasePhysicalAgg, PhysicalHashAgg, PhysicalIndexLookUpReader, PhysicalLimit, PhysicalProjection,
    PhysicalSelection, PhysicalSort, PhysicalStreamAgg, PhysicalTableReader, PhysicalTableScan,
    PhysicalTopN, PhysicalUnionScan,
};

use crate::projection::ProjectRows;
use crate::select::ExecuteLimitValues;

/// 物理计划运行时错误（字符串包装）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhysicalRuntimeError(pub String);

impl Display for PhysicalRuntimeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PhysicalRuntimeError {}

impl From<astersql_executor_sortexec::SortError> for PhysicalRuntimeError {
    fn from(error: astersql_executor_sortexec::SortError) -> Self {
        Self(error.to_string())
    }
}

/// 物理计划运行时通用 Result 别名。
pub type PhysicalRuntimeResult<T> = Result<T, PhysicalRuntimeError>;

/// 单行流式访问回调；聚合等算子可逐行消费而不物化整表。
pub type PhysicalRowVisitor<'a> = dyn FnMut(Row) -> PhysicalRuntimeResult<()> + 'a;

/// 可请求上游正常停止的逐行访问回调；`false` 对齐 Go executor child.Next 不再拉取。
pub type PhysicalRowWhileVisitor<'a> = dyn FnMut(Row) -> PhysicalRuntimeResult<bool> + 'a;

/// Executes LIMIT with the same open/next/close algorithm as SELECT.  Inline
/// projection is applied at the child boundary, while callers can retain the
/// serial outer-projection path by passing `None` and projecting the result.
/// 以与 SELECT 相同的 open/next/close 算法执行 LIMIT。
/// 可在子边界内联投影；传 `None` 则由调用方自行投影。
pub fn ExecuteLimitRows(
    rows: Vec<Row>,
    offset: usize,
    count: usize,
    max_chunk_size: usize,
    child_columns: Option<&[usize]>,
) -> PhysicalRuntimeResult<Vec<Row>> {
    // 可选：先对子结果做列投影再喂给 LimitExec。
    let rows = match child_columns {
        Some(columns) => ProjectRows(rows, columns)?,
        None => rows,
    };
    ExecuteLimitValues(rows, offset, count, max_chunk_size).map_err(PhysicalRuntimeError)
}

/// Provides decoded rows for one canonical physical table scan.
/// 为规范物理表扫描提供已解码行。
pub trait PhysicalTableSource {
    /// 执行表扫描，返回全部行。
    fn Scan(&self, scan: &PhysicalTableScan) -> PhysicalRuntimeResult<Vec<Row>>;

    /// 流式访问扫描结果。
    ///
    /// 默认实现兼容已有的小型内存数据源；真实 KV 数据源覆盖此方法，以 Go
    /// executor 的 child.Next 分块模型逐行消费，避免为聚合物化整表。
    fn ScanRows(
        &self,
        scan: &PhysicalTableScan,
        visitor: &mut PhysicalRowVisitor<'_>,
    ) -> PhysicalRuntimeResult<()> {
        for row in self.Scan(scan)? {
            visitor(row)?;
        }
        Ok(())
    }

    /// 流式访问扫描结果，并允许消费者在取得足够行后停止上游。
    ///
    /// 返回 `true` 表示扫描耗尽，`false` 表示 visitor 请求提前停止。默认实现
    /// 保持第三方数据源兼容；真实 KV 数据源覆盖此方法以停止底层迭代器。
    fn ScanRowsWhile(
        &self,
        scan: &PhysicalTableScan,
        visitor: &mut PhysicalRowWhileVisitor<'_>,
    ) -> PhysicalRuntimeResult<bool> {
        let mut keep_scanning = true;
        self.ScanRows(scan, &mut |row| {
            if keep_scanning {
                keep_scanning = visitor(row)?;
            }
            Ok(())
        })?;
        Ok(keep_scanning)
    }

    /// 统计扫描行数。裸 COUNT(*) 可走此边界，避免解码未使用的列和值。
    ///
    /// 默认实现保持第三方数据源兼容；真实 KV 数据源覆盖后只推进分页迭代器。
    fn CountRows(&self, scan: &PhysicalTableScan) -> PhysicalRuntimeResult<i64> {
        let mut count = 0_i64;
        self.ScanRows(scan, &mut |_| {
            count = count
                .checked_add(1)
                .ok_or_else(|| PhysicalRuntimeError("table row count overflow".to_owned()))?;
            Ok(())
        })?;
        Ok(count)
    }

    /// Returns transaction-local rows which Go's UnionScan merges with the
    /// snapshot rows produced by its child.
    /// 返回事务本地行，对应 Go 侧 UnionScan 与子节点快照行的合并输入。
    fn UnionScanRows(&self, _scan: &PhysicalUnionScan) -> PhysicalRuntimeResult<Vec<Row>> {
        Ok(Vec::new())
    }
}

/// Canonical table source backed by a real KV Retriever and tablecodec rows.
/// 基于真实 KV Retriever 与 tablecodec 行编码的规范表数据源。
pub struct KVRetrieverTableSource<'a> {
    /// KV 检索接口。
    retriever: &'a dyn astersql_kv::Retriever,
    /// 已扫描行数计数。
    scanned_rows: AtomicUsize,
}

impl<'a> KVRetrieverTableSource<'a> {
    /// 用给定 Retriever 构造数据源。
    pub fn New(retriever: &'a dyn astersql_kv::Retriever) -> Self {
        Self {
            retriever,
            scanned_rows: AtomicUsize::new(0),
        }
    }

    /// 返回累计扫描行数。
    pub fn ScannedRows(&self) -> usize {
        self.scanned_rows.load(Ordering::Acquire)
    }
}

/// 将 types::Datum 转为排序/执行器用的 SortValue。
fn datum_to_sort_value(datum: astersql_types::datum::Datum) -> PhysicalRuntimeResult<SortValue> {
    Ok(match datum.Kind() {
        astersql_types::datum::KindNull => SortValue::Null,
        astersql_types::datum::KindInt64 => SortValue::Int(datum.GetInt64()),
        astersql_types::datum::KindUint64 => SortValue::UInt(datum.GetUint64()),
        astersql_types::datum::KindFloat32 | astersql_types::datum::KindFloat64 => {
            SortValue::Float(datum.GetFloat64())
        }
        astersql_types::datum::KindString
        | astersql_types::datum::KindBytes
        | astersql_types::datum::KindBinaryLiteral => SortValue::Bytes(datum.GetBytes()),
        astersql_types::datum::KindMysqlDecimal => {
            SortValue::Bytes(datum.GetMysqlDecimal().String().into_bytes())
        }
        kind => {
            return Err(PhysicalRuntimeError(format!(
                "physical table source does not support datum kind {kind}"
            )));
        }
    })
}

impl PhysicalTableSource for KVRetrieverTableSource<'_> {
    fn Scan(&self, scan: &PhysicalTableScan) -> PhysicalRuntimeResult<Vec<Row>> {
        let mut rows = Vec::new();
        self.ScanRows(scan, &mut |row| {
            rows.push(row);
            Ok(())
        })?;
        Ok(rows)
    }

    fn ScanRows(
        &self,
        scan: &PhysicalTableScan,
        visitor: &mut PhysicalRowVisitor<'_>,
    ) -> PhysicalRuntimeResult<()> {
        self.ScanRowsWhile(scan, &mut |row| {
            visitor(row)?;
            Ok(true)
        })?;
        Ok(())
    }

    fn ScanRowsWhile(
        &self,
        scan: &PhysicalTableScan,
        visitor: &mut PhysicalRowWhileVisitor<'_>,
    ) -> PhysicalRuntimeResult<bool> {
        let table = scan
            .Table
            .as_ref()
            .ok_or_else(|| PhysicalRuntimeError("PhysicalTableScan has no TableInfo".to_owned()))?;
        // 物理表 ID 优先于逻辑表 ID（分区物理表场景）。
        let table_id = if scan.PhysicalTableID != 0 {
            scan.PhysicalTableID
        } else {
            table.ID
        };
        let prefix = astersql_tablecodec::GenTableRecordPrefix(table_id);
        let start = astersql_kv::Key(prefix.0);
        let end = start.PrefixNext();
        let mut iterator = if scan.Desc {
            self.retriever.IterReverse(Some(end), Some(start))
        } else {
            self.retriever.Iter(start, Some(end))
        }
        .map_err(|error| PhysicalRuntimeError(error.to_string()))?;
        let column_types = scan
            .Columns
            .iter()
            .map(|column| (column.ID, Box::new(column.FieldType.clone())))
            .collect::<HashMap<_, _>>();
        let result = (|| {
            while iterator.Valid() {
                let key = iterator.Key();
                let value = iterator.Value();
                // 解码记录键得到 handle，再解码行值到列 ID → Datum。
                let (_, handle) =
                    astersql_tablecodec::DecodeRecordKey(astersql_tablecodec::kv::Key(key.0))
                        .map_err(|error| PhysicalRuntimeError(error.to_string()))?;
                let mut decoded = astersql_tablecodec::DecodeRowToDatumMap(
                    Some(value),
                    column_types.clone(),
                    Some(astersql_tablecodec::time::UTC),
                )
                .map_err(|error| PhysicalRuntimeError(error.to_string()))?;
                // 整型主键句柄列可能不在行值中，需从 handle 补回。
                if table.PKIsHandle {
                    for column in &scan.Columns {
                        if astersql_parser_mysql::r#type::HasPriKeyFlag(column.GetFlag()) {
                            decoded.entry(column.ID).or_insert_with(|| {
                                astersql_types::datum::NewIntDatum(handle.IntValue())
                            });
                        }
                    }
                }
                let row = scan
                    .Columns
                    .iter()
                    .map(|column| {
                        decoded
                            .remove(&column.ID)
                            .map(datum_to_sort_value)
                            .unwrap_or(Ok(SortValue::Null))
                    })
                    .collect::<PhysicalRuntimeResult<Vec<_>>>()?;
                self.scanned_rows.fetch_add(1, Ordering::AcqRel);
                if !visitor(Row(row))? {
                    return Ok(false);
                }
                iterator
                    .Next()
                    .map_err(|error| PhysicalRuntimeError(error.to_string()))?;
            }
            Ok(true)
        })();
        iterator.Close();
        result
    }

    fn CountRows(&self, scan: &PhysicalTableScan) -> PhysicalRuntimeResult<i64> {
        let table = scan
            .Table
            .as_ref()
            .ok_or_else(|| PhysicalRuntimeError("PhysicalTableScan has no TableInfo".to_owned()))?;
        let table_id = if scan.PhysicalTableID != 0 {
            scan.PhysicalTableID
        } else {
            table.ID
        };
        let prefix = astersql_tablecodec::GenTableRecordPrefix(table_id);
        let start = astersql_kv::Key(prefix.0);
        let mut iterator = self
            .retriever
            .Iter(start.clone(), Some(start.PrefixNext()))
            .map_err(|error| PhysicalRuntimeError(error.to_string()))?;
        let result = (|| {
            let mut count = 0_i64;
            while iterator.Valid() {
                count = count
                    .checked_add(1)
                    .ok_or_else(|| PhysicalRuntimeError("table row count overflow".to_owned()))?;
                self.scanned_rows.fetch_add(1, Ordering::AcqRel);
                iterator
                    .Next()
                    .map_err(|error| PhysicalRuntimeError(error.to_string()))?;
            }
            Ok(count)
        })();
        iterator.Close();
        result
    }
}

/// 要求物理节点恰好有一个子节点并返回它。
fn one_child<'a>(
    plan: &'a dyn PhysicalPlan,
    name: &str,
) -> PhysicalRuntimeResult<&'a dyn PhysicalPlan> {
    let children = plan.children();
    if children.len() != 1 {
        return Err(PhysicalRuntimeError(format!(
            "{name} requires exactly one physical child, got {}",
            children.len()
        )));
    }
    Ok(children[0])
}

/// 将 ORDER BY 项转为 SortKey；要求表达式已解析为列引用。
fn sort_keys(items: &[astersql_planner_util::ByItems]) -> PhysicalRuntimeResult<Vec<SortKey>> {
    items
        .iter()
        .map(|item| {
            let column = item.Expr.as_column().ok_or_else(|| {
                PhysicalRuntimeError(
                    "physical TopN/Sort runtime requires resolved column ByItems".to_owned(),
                )
            })?;
            let index = usize::try_from(column.Index).map_err(|_| {
                PhysicalRuntimeError(format!(
                    "physical ordering column has invalid index {}",
                    column.Index
                ))
            })?;
            Ok(if item.Desc {
                SortKey::desc(index)
            } else {
                SortKey::asc(index)
            })
        })
        .collect()
}

/// 排空 TopNExec 的全部输出行。
fn drain_topn(mut executor: TopNExec) -> PhysicalRuntimeResult<Vec<Row>> {
    let mut rows = Vec::new();
    loop {
        let chunk = executor.Next(1024)?;
        if chunk.is_empty() {
            break;
        }
        rows.extend(chunk.rows);
    }
    executor.Close()?;
    Ok(rows)
}

/// 排空 SortExec 的全部输出行。
fn drain_sort(mut executor: SortExec) -> PhysicalRuntimeResult<Vec<Row>> {
    let mut rows = Vec::new();
    loop {
        let chunk = executor.Next(1024)?;
        if chunk.is_empty() {
            break;
        }
        rows.extend(chunk.rows);
    }
    executor.Close()?;
    Ok(rows)
}

/// SortValue → Datum，供表达式求值使用。
fn sort_value_to_datum(value: &SortValue) -> astersql_types::datum::Datum {
    match value {
        SortValue::Null => astersql_types::datum::Datum::default(),
        SortValue::Int(value) => astersql_types::datum::NewIntDatum(*value),
        SortValue::UInt(value) => astersql_types::datum::NewUintDatum(*value),
        SortValue::Float(value) => astersql_types::datum::NewFloat64Datum(*value),
        SortValue::Bytes(value) => astersql_types::datum::NewBytesDatum(value.clone()),
    }
}

/// 在单行上求值表达式，返回 Datum。
fn evaluate_expression(
    expression: &dyn astersql_expression::Expression,
    eval_context: &dyn astersql_expression_exprctx::EvalContext,
    row: &Row,
) -> PhysicalRuntimeResult<astersql_types::datum::Datum> {
    let mutable_row = astersql_util_chunk::mutrow::MutRowFromDatums(
        row.0.iter().map(sort_value_to_datum).collect(),
    );
    expression
        .Eval(eval_context, mutable_row.ToRow())
        .map_err(|error| PhysicalRuntimeError(error.to_string()))
}

/// 按条件列表过滤行；NULL 条件视为不满足（三值逻辑短路为丢弃）。
fn filter_rows(
    plan: &dyn PhysicalPlan,
    conditions: &[astersql_expression::ExprBox],
    rows: Vec<Row>,
) -> PhysicalRuntimeResult<Vec<Row>> {
    if conditions.is_empty() {
        return Ok(rows);
    }
    let eval_context = plan.s_ctx().GetExprCtx().GetEvalCtx();
    rows.into_iter()
        .filter_map(
            |row| match row_matches_conditions(conditions, eval_context, &row) {
                Ok(true) => Some(Ok(row)),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            },
        )
        .collect()
}

/// 判断一行是否满足全部过滤条件；NULL 与 false 均不保留。
fn row_matches_conditions(
    conditions: &[astersql_expression::ExprBox],
    eval_context: &dyn astersql_expression_exprctx::EvalContext,
    row: &Row,
) -> PhysicalRuntimeResult<bool> {
    conditions.iter().try_fold(true, |keep, condition| {
        if !keep {
            return Ok(false);
        }
        let value = evaluate_expression(condition.as_ref(), eval_context, row)?;
        if value.IsNull() {
            return Ok(false);
        }
        value
            .ToBool(eval_context.TypeCtx())
            .map(|value| value != 0)
            .map_err(|error| PhysicalRuntimeError(error.to_string()))
    })
}

/// Resolve the output-column offsets which form a UnionScan row handle.
fn union_handle_columns(scan: &PhysicalUnionScan) -> PhysicalRuntimeResult<Vec<usize>> {
    scan.HandleCols
        .IterColumns()
        .map(|column| {
            usize::try_from(column.Index).map_err(|_| {
                PhysicalRuntimeError(format!(
                    "UnionScan handle column has invalid index {}",
                    column.Index
                ))
            })
        })
        .collect()
}

/// Compare rows by the handle columns used by Go UnionScan's merge step.
fn compare_union_rows(
    left: &Row,
    right: &Row,
    handle_columns: &[usize],
    descending: bool,
) -> PhysicalRuntimeResult<RowOrdering> {
    for &index in handle_columns {
        let left_value = left.0.get(index).ok_or_else(|| {
            PhysicalRuntimeError(format!(
                "UnionScan handle column index {index} exceeds row width {}",
                left.0.len()
            ))
        })?;
        let right_value = right.0.get(index).ok_or_else(|| {
            PhysicalRuntimeError(format!(
                "UnionScan handle column index {index} exceeds row width {}",
                right.0.len()
            ))
        })?;
        let order = left_value
            .partial_cmp(right_value)
            .unwrap_or(RowOrdering::Equal);
        if order != RowOrdering::Equal {
            return Ok(if descending { order.reverse() } else { order });
        }
    }
    Ok(RowOrdering::Equal)
}

/// Merge the ordered snapshot and dirty-row streams. Equal handles select the
/// dirty row, matching Go UnionScanExec::getOneRow.
fn merge_union_rows(
    mut snapshot_rows: Vec<Row>,
    mut added_rows: Vec<Row>,
    handle_columns: &[usize],
    descending: bool,
) -> PhysicalRuntimeResult<Vec<Row>> {
    for row in snapshot_rows.iter().chain(&added_rows) {
        compare_union_rows(row, row, handle_columns, descending)?;
    }
    snapshot_rows.sort_by(|left, right| {
        compare_union_rows(left, right, handle_columns, descending)
            .expect("UnionScan rows were validated before sorting")
    });
    added_rows.sort_by(|left, right| {
        compare_union_rows(left, right, handle_columns, descending)
            .expect("UnionScan rows were validated before sorting")
    });

    let mut merged = Vec::with_capacity(snapshot_rows.len() + added_rows.len());
    let (mut snapshot, mut added) = (
        snapshot_rows.into_iter().peekable(),
        added_rows.into_iter().peekable(),
    );
    while let (Some(snapshot_row), Some(added_row)) = (snapshot.peek(), added.peek()) {
        match compare_union_rows(snapshot_row, added_row, handle_columns, descending)? {
            RowOrdering::Less => merged.push(snapshot.next().expect("peeked snapshot row")),
            RowOrdering::Equal => {
                snapshot.next();
                merged.push(added.next().expect("peeked dirty row"));
            }
            RowOrdering::Greater => merged.push(added.next().expect("peeked dirty row")),
        }
    }
    merged.extend(snapshot);
    merged.extend(added);
    Ok(merged)
}

/// 标量聚合的常量大小状态；行数增加时不增加聚合内存。
enum ScalarAggregateState {
    /// COUNT 的累计值。
    Count(i64),
    /// FIRST_ROW 的首值；外层 Option 区分“尚未见行”和“首值为 NULL”。
    FirstRow(Option<SortValue>),
}

/// 校验标量聚合描述并分配与聚合函数数量成正比的状态。
fn initialize_scalar_aggregate(
    aggregate: &BasePhysicalAgg,
) -> PhysicalRuntimeResult<Vec<ScalarAggregateState>> {
    if !aggregate.GroupByItems.is_empty() {
        return Err(PhysicalRuntimeError(
            "physical aggregate runtime currently requires scalar aggregation".to_owned(),
        ));
    }
    aggregate
        .AggFuncs
        .iter()
        .map(|function| match function.Name.as_str() {
            astersql_parser_ast::AggFuncCount => {
                if function.HasDistinct {
                    return Err(PhysicalRuntimeError(
                        "physical scalar COUNT(DISTINCT) runtime is not supported".to_owned(),
                    ));
                }
                match function.Mode {
                    CompleteMode | Partial1Mode => {}
                    FinalMode | Partial2Mode if function.Args.is_empty() => {
                        return Err(PhysicalRuntimeError(
                            "final scalar COUNT requires one argument".to_owned(),
                        ));
                    }
                    FinalMode | Partial2Mode => {}
                    mode => {
                        return Err(PhysicalRuntimeError(format!(
                            "physical scalar COUNT does not support aggregate mode {}",
                            mode.ToString()
                        )));
                    }
                }
                Ok(ScalarAggregateState::Count(0))
            }
            astersql_parser_ast::AggFuncFirstRow => {
                if function.HasDistinct {
                    return Err(PhysicalRuntimeError(
                        "physical scalar FIRST_ROW(DISTINCT) runtime is not supported".to_owned(),
                    ));
                }
                if function.Args.is_empty() {
                    return Err(PhysicalRuntimeError(
                        "scalar FIRST_ROW requires one argument".to_owned(),
                    ));
                }
                match function.Mode {
                    CompleteMode | Partial1Mode | FinalMode | Partial2Mode => {
                        Ok(ScalarAggregateState::FirstRow(None))
                    }
                    mode => Err(PhysicalRuntimeError(format!(
                        "physical scalar FIRST_ROW does not support aggregate mode {}",
                        mode.ToString()
                    ))),
                }
            }
            _ => Err(PhysicalRuntimeError(format!(
                "physical scalar aggregate runtime does not support {}",
                function.Name
            ))),
        })
        .collect()
}

/// 将一行归约进标量聚合状态，对齐 Go partial result 的逐 child chunk 更新模型。
fn update_scalar_aggregate(
    plan: &dyn PhysicalPlan,
    aggregate: &BasePhysicalAgg,
    states: &mut [ScalarAggregateState],
    row: &Row,
) -> PhysicalRuntimeResult<()> {
    let eval_context = plan.s_ctx().GetExprCtx().GetEvalCtx();
    for (function, state) in aggregate.AggFuncs.iter().zip(states) {
        match state {
            ScalarAggregateState::Count(count) => match function.Mode {
                CompleteMode | Partial1Mode => {
                    let mut all_non_null = true;
                    for argument in &function.Args {
                        if evaluate_expression(argument.as_ref(), eval_context, row)?.IsNull() {
                            all_non_null = false;
                            break;
                        }
                    }
                    if all_non_null {
                        *count = count.checked_add(1).ok_or_else(|| {
                            PhysicalRuntimeError("scalar COUNT overflow".to_owned())
                        })?;
                    }
                }
                FinalMode | Partial2Mode => {
                    let argument = &function.Args[0];
                    let value = evaluate_expression(argument.as_ref(), eval_context, row)?;
                    if !value.IsNull() {
                        *count = count.checked_add(value.GetInt64()).ok_or_else(|| {
                            PhysicalRuntimeError("final scalar COUNT overflow".to_owned())
                        })?;
                    }
                }
                _ => unreachable!("aggregate mode was validated during initialization"),
            },
            ScalarAggregateState::FirstRow(first) => {
                if first.is_none() {
                    *first = Some(
                        evaluate_expression(function.Args[0].as_ref(), eval_context, row)
                            .and_then(datum_to_sort_value)?,
                    );
                }
            }
        }
    }
    Ok(())
}

/// 将完成的标量聚合状态输出为恰好一行。
fn finish_scalar_aggregate(states: Vec<ScalarAggregateState>) -> Vec<Row> {
    let output = states
        .into_iter()
        .map(|state| match state {
            ScalarAggregateState::Count(count) => SortValue::Int(count),
            ScalarAggregateState::FirstRow(first) => first.unwrap_or(SortValue::Null),
        })
        .collect();
    vec![Row(output)]
}

/// 标量聚合（无 GROUP BY）：当前支持 COUNT 与 FIRST_ROW 及常见聚合模式。
fn execute_scalar_aggregate(
    plan: &dyn PhysicalPlan,
    aggregate: &BasePhysicalAgg,
    rows: Vec<Row>,
) -> PhysicalRuntimeResult<Vec<Row>> {
    let mut states = initialize_scalar_aggregate(aggregate)?;
    for row in &rows {
        update_scalar_aggregate(plan, aggregate, &mut states, row)?;
    }
    Ok(finish_scalar_aggregate(states))
}

/// 解析物理投影中的列下标。
fn projection_columns(projection: &PhysicalProjection) -> PhysicalRuntimeResult<Vec<usize>> {
    projection
        .Exprs
        .iter()
        .map(|expression| {
            expression
                .as_column()
                .and_then(|column| usize::try_from(column.Index).ok())
                .ok_or_else(|| {
                    PhysicalRuntimeError(
                        "physical projection runtime requires resolved column expressions"
                            .to_owned(),
                    )
                })
        })
        .collect()
}

/// 对单行执行列投影。
fn project_row(row: Row, columns: &[usize]) -> PhysicalRuntimeResult<Row> {
    columns
        .iter()
        .map(|index| {
            row.0.get(*index).cloned().ok_or_else(|| {
                PhysicalRuntimeError(format!(
                    "projection column index {index} exceeds row width {}",
                    row.0.len()
                ))
            })
        })
        .collect::<PhysicalRuntimeResult<Vec<_>>>()
        .map(Row)
}

/// 判断子树能否通过 visitor 逐行执行，而不先生成完整 `Vec<Row>`。
fn can_stream_rows(plan: &dyn PhysicalPlan) -> bool {
    if let Some(reader) = plan.as_any().downcast_ref::<PhysicalTableReader>() {
        return reader.GetTablePlan().is_some_and(can_stream_rows);
    }
    if let Some(reader) = plan.as_any().downcast_ref::<PhysicalIndexLookUpReader>() {
        return reader.TablePlan.as_deref().is_some_and(can_stream_rows);
    }
    if plan.as_any().is::<PhysicalTableScan>() {
        return true;
    }
    if plan.as_any().is::<PhysicalUnionScan>()
        || plan.as_any().is::<PhysicalSelection>()
        || plan.as_any().is::<PhysicalProjection>()
    {
        return one_child(plan, "streaming physical operator").is_ok_and(can_stream_rows);
    }
    false
}

/// 解开 reader，识别没有过滤、投影或排序的直接表扫描。
fn direct_table_scan(plan: &dyn PhysicalPlan) -> Option<&PhysicalTableScan> {
    if let Some(reader) = plan.as_any().downcast_ref::<PhysicalTableReader>() {
        return direct_table_scan(reader.GetTablePlan()?);
    }
    if let Some(reader) = plan.as_any().downcast_ref::<PhysicalIndexLookUpReader>() {
        return direct_table_scan(reader.TablePlan.as_deref()?);
    }
    plan.as_any().downcast_ref::<PhysicalTableScan>()
}

/// COUNT(*) 在规划器中规范化为单个非 NULL 常量参数。
fn is_scalar_count_star(aggregate: &BasePhysicalAgg) -> bool {
    if !aggregate.GroupByItems.is_empty() || aggregate.AggFuncs.len() != 1 {
        return false;
    }
    let function = &aggregate.AggFuncs[0];
    function.Name == astersql_parser_ast::AggFuncCount
        && !function.HasDistinct
        && matches!(function.Mode, CompleteMode | Partial1Mode)
        && matches!(
            function.Args.as_slice(),
            [argument]
                if argument
                    .as_constant()
                    .is_some_and(|constant| !constant.Value.IsNull())
        )
}

/// 按 Go executor 的 child.Next 模型流式执行扫描、过滤、UnionScan 与投影子树。
fn stream_rows(
    plan: &dyn PhysicalPlan,
    source: &dyn PhysicalTableSource,
    visitor: &mut PhysicalRowVisitor<'_>,
) -> PhysicalRuntimeResult<()> {
    stream_rows_while(plan, source, &mut |row| {
        visitor(row)?;
        Ok(true)
    })?;
    Ok(())
}

/// 按 Go executor 的 child.Next 模型流式执行，并把 LIMIT 的停止信号传到表扫描。
fn stream_rows_while(
    plan: &dyn PhysicalPlan,
    source: &dyn PhysicalTableSource,
    visitor: &mut PhysicalRowWhileVisitor<'_>,
) -> PhysicalRuntimeResult<bool> {
    if let Some(reader) = plan.as_any().downcast_ref::<PhysicalTableReader>() {
        let table_plan = reader.GetTablePlan().ok_or_else(|| {
            PhysicalRuntimeError("PhysicalTableReader has no TablePlan".to_owned())
        })?;
        return stream_rows_while(table_plan, source, visitor);
    }
    if let Some(reader) = plan.as_any().downcast_ref::<PhysicalIndexLookUpReader>() {
        let table_plan = reader.TablePlan.as_deref().ok_or_else(|| {
            PhysicalRuntimeError("PhysicalIndexLookUpReader has no TablePlan".to_owned())
        })?;
        return stream_rows_while(table_plan, source, visitor);
    }
    if let Some(scan) = plan.as_any().downcast_ref::<PhysicalTableScan>() {
        return source.ScanRowsWhile(scan, visitor);
    }
    if let Some(union_scan) = plan.as_any().downcast_ref::<PhysicalUnionScan>() {
        let child = one_child(plan, "PhysicalUnionScan")?;
        let snapshot_rows =
            filter_rows(plan, &union_scan.Conditions, execute_node(child, source)?)?;
        let added_rows = filter_rows(
            plan,
            &union_scan.Conditions,
            source.UnionScanRows(union_scan)?,
        )?;
        let handle_columns = union_handle_columns(union_scan)?;
        let descending = direct_table_scan(child).is_some_and(|scan| scan.Desc);
        for row in merge_union_rows(snapshot_rows, added_rows, &handle_columns, descending)? {
            if !visitor(row)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    if let Some(selection) = plan.as_any().downcast_ref::<PhysicalSelection>() {
        let eval_context = plan.s_ctx().GetExprCtx().GetEvalCtx();
        let mut filtered = |row| {
            if row_matches_conditions(&selection.Conditions, eval_context, &row)? {
                return visitor(row);
            }
            Ok(true)
        };
        return stream_rows_while(one_child(plan, "PhysicalSelection")?, source, &mut filtered);
    }
    if let Some(projection) = plan.as_any().downcast_ref::<PhysicalProjection>() {
        let columns = projection_columns(projection)?;
        let mut projected = |row| visitor(project_row(row, &columns)?);
        return stream_rows_while(
            one_child(plan, "PhysicalProjection")?,
            source,
            &mut projected,
        );
    }
    Err(PhysicalRuntimeError(format!(
        "physical operator {} cannot stream rows",
        plan.tp(&[])
    )))
}

/// 流式执行标量聚合，内存仅包含一个输入行和常量大小 partial result。
fn execute_streaming_scalar_aggregate(
    plan: &dyn PhysicalPlan,
    aggregate: &BasePhysicalAgg,
    child: &dyn PhysicalPlan,
    source: &dyn PhysicalTableSource,
) -> PhysicalRuntimeResult<Vec<Row>> {
    let mut states = initialize_scalar_aggregate(aggregate)?;
    stream_rows(child, source, &mut |row| {
        update_scalar_aggregate(plan, aggregate, &mut states, &row)
    })?;
    Ok(finish_scalar_aggregate(states))
}

/// 递归执行物理计划节点，按算子类型分发到扫描 / 过滤 / 聚合 / TopN 等。
fn execute_node(
    plan: &dyn PhysicalPlan,
    source: &dyn PhysicalTableSource,
) -> PhysicalRuntimeResult<Vec<Row>> {
    // TableReader / IndexLookUpReader：下钻到其 TablePlan。
    if let Some(reader) = plan.as_any().downcast_ref::<PhysicalTableReader>() {
        let table_plan = reader.GetTablePlan().ok_or_else(|| {
            PhysicalRuntimeError("PhysicalTableReader has no TablePlan".to_owned())
        })?;
        return execute_node(table_plan, source);
    }
    if let Some(reader) = plan.as_any().downcast_ref::<PhysicalIndexLookUpReader>() {
        let table_plan = reader.TablePlan.as_deref().ok_or_else(|| {
            PhysicalRuntimeError("PhysicalIndexLookUpReader has no TablePlan".to_owned())
        })?;
        return execute_node(table_plan, source);
    }
    if let Some(scan) = plan.as_any().downcast_ref::<PhysicalTableScan>() {
        return source.Scan(scan);
    }
    if let Some(union_scan) = plan.as_any().downcast_ref::<PhysicalUnionScan>() {
        let child = one_child(plan, "PhysicalUnionScan")?;
        let snapshot_rows =
            filter_rows(plan, &union_scan.Conditions, execute_node(child, source)?)?;
        let added_rows = filter_rows(
            plan,
            &union_scan.Conditions,
            source.UnionScanRows(union_scan)?,
        )?;
        let handle_columns = union_handle_columns(union_scan)?;
        let descending = direct_table_scan(child).is_some_and(|scan| scan.Desc);
        return merge_union_rows(snapshot_rows, added_rows, &handle_columns, descending);
    }
    if let Some(selection) = plan.as_any().downcast_ref::<PhysicalSelection>() {
        let rows = execute_node(one_child(plan, "PhysicalSelection")?, source)?;
        return filter_rows(plan, &selection.Conditions, rows);
    }
    if let Some(aggregate) = plan.as_any().downcast_ref::<PhysicalStreamAgg>() {
        let child = one_child(plan, "PhysicalStreamAgg")?;
        if is_scalar_count_star(&aggregate.BasePhysicalAgg)
            && let Some(scan) = direct_table_scan(child)
        {
            return source
                .CountRows(scan)
                .map(|count| vec![Row(vec![SortValue::Int(count)])]);
        }
        if can_stream_rows(child) {
            return execute_streaming_scalar_aggregate(
                plan,
                &aggregate.BasePhysicalAgg,
                child,
                source,
            );
        }
        let rows = execute_node(child, source)?;
        return execute_scalar_aggregate(plan, &aggregate.BasePhysicalAgg, rows);
    }
    if let Some(aggregate) = plan.as_any().downcast_ref::<PhysicalHashAgg>() {
        let child = one_child(plan, "PhysicalHashAgg")?;
        if is_scalar_count_star(&aggregate.BasePhysicalAgg)
            && let Some(scan) = direct_table_scan(child)
        {
            return source
                .CountRows(scan)
                .map(|count| vec![Row(vec![SortValue::Int(count)])]);
        }
        if can_stream_rows(child) {
            return execute_streaming_scalar_aggregate(
                plan,
                &aggregate.BasePhysicalAgg,
                child,
                source,
            );
        }
        let rows = execute_node(child, source)?;
        return execute_scalar_aggregate(plan, &aggregate.BasePhysicalAgg, rows);
    }
    if let Some(topn) = plan.as_any().downcast_ref::<PhysicalTopN>() {
        // TopN：委托 sortexec::TopNExec，而非自行堆排序。
        let rows = execute_node(one_child(plan, "PhysicalTopN")?, source)?;
        let child = Box::new(astersql_executor_sortexec::sort::VecRowSource::new(vec![
            DataChunk::new(rows),
        ]));
        let executor = TopNExec::new(
            child,
            sort_keys(&topn.ByItems)?,
            Limit {
                Offset: topn.Offset as usize,
                Count: topn.Count as usize,
            },
            None,
            1,
            1024,
            -1,
        );
        return drain_topn(executor);
    }
    if let Some(sort) = plan.as_any().downcast_ref::<PhysicalSort>() {
        let rows = execute_node(one_child(plan, "PhysicalSort")?, source)?;
        let child = Box::new(astersql_executor_sortexec::sort::VecRowSource::new(vec![
            DataChunk::new(rows),
        ]));
        return drain_sort(SortExec::new(child, sort_keys(&sort.ByItems)?, 1, 1024, -1));
    }
    if let Some(limit) = plan.as_any().downcast_ref::<PhysicalLimit>() {
        let child = one_child(plan, "PhysicalLimit")?;
        if can_stream_rows(child) {
            let offset = limit.Offset as usize;
            let count = limit.Count as usize;
            if count == 0 {
                return Ok(Vec::new());
            }
            let mut seen = 0_usize;
            let mut rows = Vec::with_capacity(count.min(1024));
            stream_rows_while(child, source, &mut |row| {
                if seen < offset {
                    seen += 1;
                    return Ok(true);
                }
                rows.push(row);
                Ok(rows.len() < count)
            })?;
            return Ok(rows);
        }
        let rows = execute_node(child, source)?;
        return ExecuteLimitRows(
            rows,
            limit.Offset as usize,
            limit.Count as usize,
            1024,
            None,
        );
    }
    if let Some(projection) = plan.as_any().downcast_ref::<PhysicalProjection>() {
        // 投影仅支持已解析的列引用表达式。
        let rows = execute_node(one_child(plan, "PhysicalProjection")?, source)?;
        let columns = projection_columns(projection)?;
        return rows
            .into_iter()
            .map(|row| project_row(row, &columns))
            .collect();
    }
    Err(PhysicalRuntimeError(format!(
        "unsupported physical operator {}",
        plan.tp(&[])
    )))
}

/// Runs a canonical physical plan against caller-owned decoded table rows.
/// 对调用方持有的表数据源执行规范物理计划，返回结果行。
pub fn ExecutePhysicalPlan(
    plan: &dyn PhysicalPlan,
    source: &dyn PhysicalTableSource,
) -> PhysicalRuntimeResult<Vec<Row>> {
    execute_node(plan, source)
}
