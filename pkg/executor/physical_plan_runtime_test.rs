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

// 物理计划运行时（physical_plan_runtime）的单元测试。
//
// 覆盖规范物理树上的 TopN、UnionScan + Selection + COUNT，
// 以及 FIRST_ROW 聚合在各 AggFunctionMode 下对空输入 / NULL / 首行的语义。
// 物理计划是优化器选定的可执行算子树；TopN 表示排序后取前 N 行。

use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use astersql_executor_sortexec::{Row, SortValue};
use astersql_expression_aggregation::{
    AggFunctionMode, CompleteMode, FinalMode, Partial1Mode, Partial2Mode,
};
use astersql_planner_core_base::{PhysicalPlan as _, PlanContext};
use astersql_planner_core_operator_physicalop::{
    BasePhysicalAgg, BasePhysicalPlan, PhysicalHashAgg, PhysicalLimit, PhysicalSchemaProducer,
    PhysicalSelection, PhysicalStreamAgg, PhysicalTableReader, PhysicalTableScan, PhysicalTopN,
    PhysicalUnionScan,
};

use crate::physical_plan_runtime::{
    ExecutePhysicalPlan, KVRetrieverTableSource, PhysicalRowVisitor, PhysicalRuntimeError,
    PhysicalRuntimeResult, PhysicalTableSource,
};

/// 测试用 PlanContext：分配 plan_id，并提供会话 / 表达式上下文桩。
struct TestPlanContext {
    /// 递增的物理计划节点 ID 计数器。
    plan_id: AtomicI32,
    /// 会话变量桩。
    vars: astersql_sessionctx_variable::session::SessionVars,
    /// 表达式求值上下文桩。
    expr: astersql_expression_exprstatic::ExprContext,
    /// 内建函数使用计数。
    usage: astersql_planner_core_base::BuiltinFunctionUsageCounter,
}

impl PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.plan_id.fetch_add(1, Ordering::SeqCst) + 1
    }
    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }
    fn GetSessionVars(&self) -> &astersql_sessionctx_variable::session::SessionVars {
        &self.vars
    }
    fn GetExprCtx(&self) -> &dyn astersql_expression_exprctx::ExprContext {
        &self.expr
    }
    fn GetRangerCtx(&self) -> &astersql_planner_core_base::RangerContext<'_> {
        panic!("unused")
    }
    fn GetNullRejectCheckExprCtx(&self) -> &dyn astersql_expression_exprctx::ExprContext {
        &self.expr
    }
    fn GetBuildPBCtx(&self) -> &astersql_planner_core_base::BuildPBContext {
        panic!("unused")
    }
    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.usage.Inc(name)
    }
}

/// 构造默认测试 PlanContext。
pub(crate) fn context() -> astersql_planner_core_base::ContextRef {
    context_with_dialect(astersql_parser_ast::SqlDialect::MySQL)
}

pub(crate) fn context_with_dialect(
    dialect: astersql_parser_ast::SqlDialect,
) -> astersql_planner_core_base::ContextRef {
    Arc::new(TestPlanContext {
        plan_id: AtomicI32::new(0),
        vars: Default::default(),
        expr: astersql_expression_exprstatic::NewExprContext(vec![
            astersql_expression_exprstatic::WithEvalCtx(Arc::new(
                astersql_expression_exprstatic::NewEvalContext(vec![
                    astersql_expression_exprstatic::WithSQLDialect(dialect),
                ]),
            )),
        ]),
        usage: Default::default(),
    })
}

/// 固定行表源：Scan 直接返回预置行，不走 KV。
#[derive(Clone)]
struct FixedRowsSource(Vec<Row>);

impl PhysicalTableSource for FixedRowsSource {
    fn Scan(&self, _scan: &PhysicalTableScan) -> PhysicalRuntimeResult<Vec<Row>> {
        Ok(self.0.clone())
    }
}

/// Snapshot rows plus transaction-local rows used to verify UnionScan merging.
struct UnionRowsSource {
    snapshot: Vec<Row>,
    added: Vec<Row>,
}

impl PhysicalTableSource for UnionRowsSource {
    fn Scan(&self, _scan: &PhysicalTableScan) -> PhysicalRuntimeResult<Vec<Row>> {
        Ok(self.snapshot.clone())
    }

    fn UnionScanRows(&self, _scan: &PhysicalUnionScan) -> PhysicalRuntimeResult<Vec<Row>> {
        Ok(self.added.clone())
    }
}

/// 仅允许流式扫描的生成型数据源；整表 Scan 被调用即报错。
struct StreamingRowsSource {
    rows: usize,
    materialized_scans: AtomicUsize,
    streamed_rows: AtomicUsize,
    count_requests: AtomicUsize,
}

impl StreamingRowsSource {
    fn new(rows: usize) -> Self {
        Self {
            rows,
            materialized_scans: AtomicUsize::new(0),
            streamed_rows: AtomicUsize::new(0),
            count_requests: AtomicUsize::new(0),
        }
    }
}

impl PhysicalTableSource for StreamingRowsSource {
    fn Scan(&self, _scan: &PhysicalTableScan) -> PhysicalRuntimeResult<Vec<Row>> {
        self.materialized_scans.fetch_add(1, Ordering::SeqCst);
        Err(PhysicalRuntimeError(
            "streaming source must not materialize all rows".to_owned(),
        ))
    }

    fn ScanRows(
        &self,
        _scan: &PhysicalTableScan,
        visitor: &mut PhysicalRowVisitor<'_>,
    ) -> PhysicalRuntimeResult<()> {
        for value in 0..self.rows {
            visitor(Row(vec![SortValue::Int(value as i64)]))?;
            self.streamed_rows.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }

    fn CountRows(&self, _scan: &PhysicalTableScan) -> PhysicalRuntimeResult<i64> {
        self.count_requests.fetch_add(1, Ordering::SeqCst);
        i64::try_from(self.rows)
            .map_err(|_| PhysicalRuntimeError("test row count exceeds i64".to_owned()))
    }
}

/// 构造 HashAgg(COUNT(*)) → TableScan 标量聚合树。
fn count_plan() -> PhysicalHashAgg {
    let ctx = context();
    let value_column = astersql_expression::Column::new(
        *astersql_expression::types::NewFieldType(astersql_expression::mysql::TypeLonglong),
        1,
        1,
        0,
    );
    let input_schema = astersql_expression::NewSchema(vec![value_column.clone()]);
    let mut scan = PhysicalTableScan::New(ctx.clone());
    scan.Table = Some(astersql_meta_model::TableInfo {
        ID: 502,
        Name: astersql_parser_ast::NewCIStr("streaming_count_source"),
        ..Default::default()
    });
    scan.Columns = vec![astersql_meta_model::ColumnInfo {
        ID: 1,
        Name: astersql_parser_ast::NewCIStr("value"),
        Offset: 0,
        FieldType: astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeLonglong),
        ..Default::default()
    }];
    scan.PhysicalSchemaProducer.SetSchema(input_schema);

    // 规划器会把 COUNT(*) 规范化为 COUNT(常量 1)。
    let count_star_argument = astersql_expression::Constant::with_type(
        astersql_types::datum::NewIntDatum(1),
        *astersql_expression::types::NewFieldType(astersql_expression::mysql::TypeLonglong),
    );
    let count = astersql_expression_aggregation::NewAggFuncDesc(
        ctx.GetExprCtx(),
        astersql_parser_ast::AggFuncCount,
        vec![Box::new(count_star_argument)],
        false,
    )
    .expect("build COUNT descriptor");
    let mut aggregate = PhysicalHashAgg {
        BasePhysicalAgg: BasePhysicalAgg::New(PhysicalSchemaProducer::New(BasePhysicalPlan::New(
            ctx, "HashAgg", 0,
        ))),
        TiflashPreAggMode: String::new(),
    };
    aggregate.BasePhysicalAgg.AggFuncs = vec![count];
    aggregate.set_children(vec![Box::new(scan)]);
    aggregate
}

/// 构造仅含 FIRST_ROW 的 StreamAgg 物理树，子节点为单列表扫描。
fn first_row_plan(mode: AggFunctionMode) -> PhysicalStreamAgg {
    let ctx = context();
    let value_column = astersql_expression::Column::new(
        *astersql_expression::types::NewFieldType(astersql_expression::mysql::TypeLonglong),
        1,
        1,
        0,
    );
    let input_schema = astersql_expression::NewSchema(vec![value_column.clone()]);
    let mut scan = PhysicalTableScan::New(ctx.clone());
    scan.Table = Some(astersql_meta_model::TableInfo {
        ID: 501,
        Name: astersql_parser_ast::NewCIStr("first_row_source"),
        ..Default::default()
    });
    scan.Columns = vec![astersql_meta_model::ColumnInfo {
        ID: 1,
        Name: astersql_parser_ast::NewCIStr("value"),
        Offset: 0,
        FieldType: astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeLonglong),
        ..Default::default()
    }];
    scan.PhysicalSchemaProducer.SetSchema(input_schema);

    let mut first_row = astersql_expression_aggregation::NewAggFuncDesc(
        ctx.GetExprCtx(),
        astersql_parser_ast::AggFuncFirstRow,
        vec![Box::new(value_column)],
        false,
    )
    .expect("build FIRST_ROW descriptor");
    first_row.Mode = mode;

    let mut aggregate =
        PhysicalStreamAgg {
            BasePhysicalAgg: BasePhysicalAgg::New(PhysicalSchemaProducer::New(
                BasePhysicalPlan::New(ctx, "StreamAgg", 0),
            )),
        };
    aggregate.BasePhysicalAgg.AggFuncs = vec![first_row];
    aggregate
        .BasePhysicalAgg
        .PhysicalSchemaProducer
        .SetSchema(astersql_expression::NewSchema(vec![
            astersql_expression::Column::new(
                *astersql_expression::types::NewFieldType(astersql_expression::mysql::TypeLonglong),
                2,
                2,
                0,
            ),
        ]));
    aggregate.set_children(vec![Box::new(scan)]);
    aggregate
}

/// 内存 KV Retriever：有序存放 (key, value)，供 TableReader 扫描。
#[derive(Default)]
pub(crate) struct MemoryRetriever {
    /// 按 key 字节序维护的条目列表。
    entries: Mutex<Vec<(astersql_kv::Key, Vec<u8>)>>,
    iter_calls: AtomicUsize,
}

impl MemoryRetriever {
    /// 插入或覆盖一条 KV，并保持 key 有序。
    pub(crate) fn Put(&self, key: astersql_kv::Key, value: Vec<u8>) {
        let mut entries = self.entries.lock().expect("memory KV lock");
        if let Some((_, old_value)) = entries.iter_mut().find(|(old_key, _)| *old_key == key) {
            *old_value = value;
        } else {
            entries.push((key, value));
            entries.sort_by(|left, right| left.0.0.cmp(&right.0.0));
        }
    }

    pub(crate) fn IterCalls(&self) -> usize {
        self.iter_calls.load(Ordering::Acquire)
    }
}

/// 内存 KV 迭代器：在已过滤的条目切片上前进。
struct MemoryIterator {
    /// 本次扫描可见的条目快照。
    entries: Vec<(astersql_kv::Key, Vec<u8>)>,
    /// 当前游标下标。
    offset: usize,
}

impl astersql_kv::Iterator for MemoryIterator {
    fn Valid(&self) -> bool {
        self.offset < self.entries.len()
    }
    fn Key(&self) -> astersql_kv::Key {
        self.entries[self.offset].0.clone()
    }
    fn Value(&self) -> Vec<u8> {
        self.entries[self.offset].1.clone()
    }
    fn Next(&mut self) -> Result<(), astersql_errors::SharedError> {
        self.offset += 1;
        Ok(())
    }
    fn Close(&mut self) {}
}

impl astersql_kv::Getter for MemoryRetriever {
    fn Get(
        &self,
        _ctx: &astersql_kv::context::Context,
        key: astersql_kv::Key,
        _options: &[astersql_kv::GetOption],
    ) -> Result<astersql_kv::ValueEntry, astersql_errors::SharedError> {
        self.entries
            .lock()
            .expect("memory KV lock")
            .iter()
            .find(|(entry_key, _)| *entry_key == key)
            .map(|(_, value)| astersql_kv::NewValueEntry(value.clone(), 0))
            .ok_or_else(|| astersql_kv::ErrNotExist.FastGenByArgs(&[]))
    }
}

impl astersql_kv::Retriever for MemoryRetriever {
    fn Iter(
        &self,
        start: astersql_kv::Key,
        upper_bound: Option<astersql_kv::Key>,
    ) -> Result<Box<dyn astersql_kv::Iterator>, astersql_errors::SharedError> {
        self.iter_calls.fetch_add(1, Ordering::AcqRel);
        let entries = self
            .entries
            .lock()
            .expect("memory KV lock")
            .iter()
            .filter(|(key, _)| {
                key.0 >= start.0
                    && upper_bound
                        .as_ref()
                        .is_none_or(|upper_bound| key.0 < upper_bound.0)
            })
            .cloned()
            .collect();
        Ok(Box::new(MemoryIterator { entries, offset: 0 }))
    }

    fn IterReverse(
        &self,
        start: Option<astersql_kv::Key>,
        lower_bound: Option<astersql_kv::Key>,
    ) -> Result<Box<dyn astersql_kv::Iterator>, astersql_errors::SharedError> {
        let mut entries = self
            .entries
            .lock()
            .expect("memory KV lock")
            .iter()
            .filter(|(key, _)| {
                start.as_ref().is_none_or(|start| key.0 < start.0)
                    && lower_bound
                        .as_ref()
                        .is_none_or(|lower_bound| key.0 >= lower_bound.0)
            })
            .cloned()
            .collect::<Vec<_>>();
        entries.reverse();
        Ok(Box::new(MemoryIterator { entries, offset: 0 }))
    }
}

/// 将 (a, b) 两列编码为表 `table_id` 上 handle 对应的行键与行值。
/// 编码一张表的单行记录键值（列 a:int, b:string）。
pub(crate) fn encode_row(
    table_id: i64,
    handle: i64,
    a: i64,
    b: &str,
) -> (astersql_kv::Key, Vec<u8>) {
    let key = astersql_tablecodec::EncodeRowKeyWithHandle(
        table_id,
        Box::new(astersql_tablecodec::kv::IntHandle(handle)),
    );
    let value = astersql_tablecodec::EncodeRow(
        Some(astersql_tablecodec::time::UTC),
        vec![
            astersql_types::datum::NewIntDatum(a),
            astersql_types::datum::NewStringDatum(b.to_owned()),
        ],
        vec![1, 2],
        Vec::new(),
        None,
        None,
        astersql_tablecodec::rowcodec::Encoder::new(true),
    )
    .expect("encode row");
    (astersql_kv::Key(key.0), value)
}

/// TableScan → TableReader → TopN：按 (a,b) 升序取 offset=1,count=2，且只扫本表前缀。
/// TopN 物理树经真实 KV 前缀扫描；只读本表记录，且随 KV 变更结果变化。
#[test]
fn strict_t_multi_uses_physical_tree_and_real_topn_executor() {
    let ctx = context();
    let columns = vec![
        astersql_expression::Column::new(
            *astersql_expression::types::NewFieldType(astersql_expression::mysql::TypeLonglong),
            1,
            1,
            0,
        ),
        astersql_expression::Column::new(
            *astersql_expression::types::NewFieldType(astersql_expression::mysql::TypeVarchar),
            2,
            2,
            1,
        ),
    ];
    let schema = astersql_expression::NewSchema(columns.clone());
    let table = astersql_meta_model::TableInfo {
        ID: 88,
        Name: astersql_parser_ast::NewCIStr("t_multi"),
        ..Default::default()
    };
    let mut scan = PhysicalTableScan::New(ctx.clone());
    scan.Table = Some(table);
    scan.Columns = vec![
        astersql_meta_model::ColumnInfo {
            ID: 1,
            Name: astersql_parser_ast::NewCIStr("a"),
            Offset: 0,
            FieldType: astersql_parser_types::NewFieldType(
                astersql_parser_mysql::r#type::TypeLonglong,
            ),
            ..Default::default()
        },
        astersql_meta_model::ColumnInfo {
            ID: 2,
            Name: astersql_parser_ast::NewCIStr("b"),
            Offset: 1,
            FieldType: astersql_parser_types::NewFieldType(
                astersql_parser_mysql::r#type::TypeVarchar,
            ),
            ..Default::default()
        },
    ];
    scan.PhysicalSchemaProducer.SetSchema(schema.Clone());

    let mut reader = PhysicalTableReader::New(ctx.clone());
    reader.SetTablePlanForTest(Box::new(scan));

    let mut topn = PhysicalTopN::New(ctx, 1, 2);
    topn.ByItems = vec![
        astersql_planner_util::ByItems {
            Expr: Box::new(columns[0].Clone()),
            Desc: false,
        },
        astersql_planner_util::ByItems {
            Expr: Box::new(columns[1].Clone()),
            Desc: false,
        },
    ];
    topn.PhysicalSchemaProducer.SetSchema(schema);
    topn.set_children(vec![Box::new(reader)]);

    let storage = MemoryRetriever::default();
    for (handle, a, b) in [
        (1, 3, "c"),
        (2, 1, "z"),
        (3, 1, "b"),
        (4, 2, "x"),
        (5, 1, "a"),
        (6, 4, "q"),
    ] {
        let (key, value) = encode_row(88, handle, a, b);
        storage.Put(key, value);
    }
    let (other_key, other_value) = encode_row(99, 1, -100, "other-table");
    storage.Put(other_key, other_value);

    let source = KVRetrieverTableSource::New(&storage);
    // 全局按 (a,b) 排序后跳过 1 行再取 2 行：期望 (1,"b")、(1,"z")。
    let rows = ExecutePhysicalPlan(&topn, &source).expect("execute canonical physical TopN");
    assert_eq!(
        rows,
        vec![
            Row(vec![SortValue::Int(1), SortValue::Bytes(b"b".to_vec())]),
            Row(vec![SortValue::Int(1), SortValue::Bytes(b"z".to_vec())]),
        ]
    );
    // 表 99 的行不得进入表 88 的 record 前缀扫描。
    assert_eq!(
        source.ScannedRows(),
        6,
        "other tables must stay outside the record prefix"
    );

    // 改写 KV 后结果应随当前存储变化，而非缓存旧结果。
    let (changed_key, changed_value) = encode_row(88, 6, 0, "changed");
    storage.Put(changed_key, changed_value);
    let changed_source = KVRetrieverTableSource::New(&storage);
    let changed_rows =
        ExecutePhysicalPlan(&topn, &changed_source).expect("execute after KV mutation");
    assert_eq!(
        changed_rows,
        vec![
            Row(vec![SortValue::Int(1), SortValue::Bytes(b"a".to_vec())]),
            Row(vec![SortValue::Int(1), SortValue::Bytes(b"b".to_vec())]),
        ]
    );
    assert_ne!(
        changed_rows, rows,
        "result must be derived from current KV values"
    );
    assert_eq!(changed_source.ScannedRows(), 6);

    // 空表扫描应返回 0 行且不计入其他表的 KV。
    let mut empty_scan = PhysicalTableScan::New(context());
    empty_scan.Table = Some(astersql_meta_model::TableInfo {
        ID: 77,
        Name: astersql_parser_ast::NewCIStr("empty_table"),
        ..Default::default()
    });
    empty_scan.Columns = vec![astersql_meta_model::ColumnInfo {
        ID: 1,
        Name: astersql_parser_ast::NewCIStr("a"),
        FieldType: astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeLonglong),
        ..Default::default()
    }];
    let empty_source = KVRetrieverTableSource::New(&storage);
    assert!(
        ExecutePhysicalPlan(&empty_scan, &empty_source)
            .expect("empty table scan")
            .is_empty()
    );
    assert_eq!(empty_source.ScannedRows(), 0);
}

/// Selection 后的 root Limit 必须像 Go LimitExec 一样，取满后停止向 child 拉行。
#[test]
fn limit_stops_kv_scan_after_enough_filtered_rows() {
    let ctx = context();
    let value_column = astersql_expression::Column::new(
        *astersql_expression::types::NewFieldType(astersql_expression::mysql::TypeLonglong),
        1,
        1,
        0,
    );
    let schema = astersql_expression::NewSchema(vec![value_column.clone()]);
    let mut scan = PhysicalTableScan::New(ctx.clone());
    scan.Table = Some(astersql_meta_model::TableInfo {
        ID: 109,
        Name: astersql_parser_ast::NewCIStr("limit_source"),
        ..Default::default()
    });
    scan.Columns = vec![astersql_meta_model::ColumnInfo {
        ID: 1,
        Name: astersql_parser_ast::NewCIStr("user_id"),
        Offset: 0,
        FieldType: astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeLonglong),
        ..Default::default()
    }];
    scan.PhysicalSchemaProducer.SetSchema(schema.Clone());

    let zero = astersql_expression::Constant::with_type(
        astersql_types::datum::NewIntDatum(0),
        *astersql_expression::types::NewFieldType(astersql_expression::mysql::TypeLonglong),
    );
    let condition = astersql_expression::NewFunction(
        ctx.GetExprCtx(),
        astersql_parser_ast::GT,
        *astersql_expression::types::NewFieldType(astersql_expression::mysql::TypeTiny),
        vec![Box::new(value_column), Box::new(zero)],
    )
    .expect("build selection condition");
    let mut selection = PhysicalSelection::New(ctx.clone());
    selection.Conditions = vec![condition];
    selection.PhysicalSchemaProducer.SetSchema(schema.Clone());
    selection.set_children(vec![Box::new(scan)]);

    let mut reader = PhysicalTableReader::New(ctx.clone());
    reader.SetTablePlanForTest(Box::new(selection));
    reader.PhysicalSchemaProducer.SetSchema(schema.Clone());

    let mut limit = PhysicalLimit::New(ctx, 0, 4);
    limit.PhysicalSchemaProducer.SetSchema(schema);
    limit.set_children(vec![Box::new(reader)]);

    let storage = MemoryRetriever::default();
    for (handle, value) in [-2, -1, 1, 2, 3, 4, 5, 6].into_iter().enumerate() {
        let (key, encoded) = encode_row(109, handle as i64 + 1, value, "unused");
        storage.Put(key, encoded);
    }
    let source = KVRetrieverTableSource::New(&storage);
    let rows = ExecutePhysicalPlan(&limit, &source).expect("execute filtered limit");

    assert_eq!(
        rows,
        vec![
            Row(vec![SortValue::Int(1)]),
            Row(vec![SortValue::Int(2)]),
            Row(vec![SortValue::Int(3)]),
            Row(vec![SortValue::Int(4)]),
        ]
    );
    assert_eq!(
        source.ScannedRows(),
        6,
        "Limit must stop after four matching rows instead of scanning the table"
    );
}

/// Selection(a>0) → TableReader → UnionScan → StreamAgg(COUNT)：缓存表标量计数。
/// Selection → TableReader → UnionScan → StreamAgg(COUNT) 的规范物理树。
#[test]
fn cached_table_scalar_count_executes_canonical_union_scan_tree() {
    let ctx = context();
    let value_column = astersql_expression::Column::new(
        *astersql_expression::types::NewFieldType(astersql_expression::mysql::TypeLonglong),
        1,
        1,
        0,
    );
    let input_schema = astersql_expression::NewSchema(vec![value_column.clone()]);
    let mut scan = PhysicalTableScan::New(ctx.clone());
    scan.Table = Some(astersql_meta_model::TableInfo {
        ID: 108,
        Name: astersql_parser_ast::NewCIStr("cached_t"),
        ..Default::default()
    });
    scan.Columns = vec![astersql_meta_model::ColumnInfo {
        ID: 1,
        Name: astersql_parser_ast::NewCIStr("a"),
        Offset: 0,
        FieldType: astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeLonglong),
        ..Default::default()
    }];
    scan.PhysicalSchemaProducer.SetSchema(input_schema.Clone());

    let zero = astersql_expression::Constant::with_type(
        astersql_types::datum::NewIntDatum(0),
        *astersql_expression::types::NewFieldType(astersql_expression::mysql::TypeLonglong),
    );
    let condition = astersql_expression::NewFunction(
        ctx.GetExprCtx(),
        astersql_parser_ast::GT,
        *astersql_expression::types::NewFieldType(astersql_expression::mysql::TypeTiny),
        vec![Box::new(value_column.clone()), Box::new(zero)],
    )
    .expect("build real cached-table selection condition");
    let mut selection = PhysicalSelection::New(ctx.clone());
    selection.Conditions = vec![condition];
    selection
        .PhysicalSchemaProducer
        .SetSchema(input_schema.Clone());
    selection.set_children(vec![Box::new(scan)]);

    let mut reader = PhysicalTableReader::New(ctx.clone());
    reader.SetTablePlanForTest(Box::new(selection));
    reader
        .PhysicalSchemaProducer
        .SetSchema(input_schema.Clone());

    let mut union_scan = PhysicalUnionScan::New(ctx.clone());
    union_scan
        .PhysicalSchemaProducer
        .SetSchema(input_schema.Clone());
    union_scan.set_children(vec![Box::new(reader)]);

    let count = astersql_expression_aggregation::NewAggFuncDesc(
        ctx.GetExprCtx(),
        astersql_parser_ast::AggFuncCount,
        vec![Box::new(value_column)],
        false,
    )
    .expect("build real scalar count descriptor");
    let output_schema = astersql_expression::NewSchema(vec![astersql_expression::Column::new(
        *astersql_expression::types::NewFieldType(astersql_expression::mysql::TypeLonglong),
        2,
        2,
        0,
    )]);
    let mut aggregate =
        PhysicalStreamAgg {
            BasePhysicalAgg: BasePhysicalAgg::New(PhysicalSchemaProducer::New(
                BasePhysicalPlan::New(ctx, "StreamAgg", 0),
            )),
        };
    aggregate.BasePhysicalAgg.AggFuncs = vec![count];
    aggregate
        .BasePhysicalAgg
        .PhysicalSchemaProducer
        .SetSchema(output_schema);
    aggregate.set_children(vec![Box::new(union_scan)]);

    let storage = MemoryRetriever::default();
    for (handle, value) in [(1, 7), (2, 11)] {
        let (key, encoded) = encode_row(108, handle, value, "cached");
        storage.Put(key, encoded);
    }
    let source = KVRetrieverTableSource::New(&storage);
    let rows = ExecutePhysicalPlan(&aggregate, &source)
        .expect("execute cached-table canonical physical tree");
    assert_eq!(rows, vec![Row(vec![SortValue::Int(2)])]);
    assert_eq!(source.ScannedRows(), 2);
}

/// Go UnionScan merges ordered dirty rows with snapshot rows and lets a dirty
/// row replace the snapshot row with the same handle.
#[test]
fn union_scan_merges_by_handle_and_dirty_rows_shadow_snapshot_rows() {
    let ctx = context();
    let handle_column = astersql_expression::Column::new(
        *astersql_expression::types::NewFieldType(astersql_expression::mysql::TypeLonglong),
        1,
        1,
        0,
    );
    let schema = astersql_expression::NewSchema(vec![handle_column.clone()]);
    let mut scan = PhysicalTableScan::New(ctx.clone());
    scan.Table = Some(astersql_meta_model::TableInfo {
        ID: 110,
        Name: astersql_parser_ast::NewCIStr("union_source"),
        ..Default::default()
    });
    scan.PhysicalSchemaProducer.SetSchema(schema.Clone());

    let mut union_scan = PhysicalUnionScan::New(ctx);
    union_scan.HandleCols = astersql_planner_util::NewIntHandleCols(handle_column);
    union_scan.PhysicalSchemaProducer.SetSchema(schema);
    union_scan.set_children(vec![Box::new(scan)]);

    let source = UnionRowsSource {
        snapshot: vec![
            Row(vec![SortValue::Int(1)]),
            Row(vec![SortValue::Int(3)]),
            Row(vec![SortValue::Int(5)]),
        ],
        added: vec![
            Row(vec![SortValue::Int(2)]),
            Row(vec![SortValue::Int(3)]),
            Row(vec![SortValue::Int(4)]),
        ],
    };
    let rows = ExecutePhysicalPlan(&union_scan, &source).expect("execute ordered UnionScan merge");
    assert_eq!(
        rows,
        vec![
            Row(vec![SortValue::Int(1)]),
            Row(vec![SortValue::Int(2)]),
            Row(vec![SortValue::Int(3)]),
            Row(vec![SortValue::Int(4)]),
            Row(vec![SortValue::Int(5)]),
        ]
    );
}

/// 空输入时 FIRST_ROW 在所有支持模式下返回 SQL NULL。
#[test]
fn scalar_first_row_returns_null_for_empty_input_in_all_supported_modes() {
    for mode in [CompleteMode, Partial1Mode, FinalMode, Partial2Mode] {
        let rows = ExecutePhysicalPlan(&first_row_plan(mode), &FixedRowsSource(Vec::new()))
            .expect("execute FIRST_ROW over empty input");
        assert_eq!(
            rows,
            vec![Row(vec![SortValue::Null])],
            "mode {} must return SQL NULL for an empty input",
            mode.ToString()
        );
    }
}

/// 首行为 NULL 时 FIRST_ROW 不得跳过该 NULL。
#[test]
fn scalar_first_row_keeps_a_null_first_row_in_all_supported_modes() {
    let input = vec![
        Row(vec![SortValue::Null]),
        Row(vec![SortValue::Int(7)]),
        Row(vec![SortValue::Int(11)]),
    ];
    for mode in [CompleteMode, Partial1Mode, FinalMode, Partial2Mode] {
        let rows = ExecutePhysicalPlan(&first_row_plan(mode), &FixedRowsSource(input.clone()))
            .expect("execute FIRST_ROW with a NULL first row");
        assert_eq!(
            rows,
            vec![Row(vec![SortValue::Null])],
            "mode {} must not skip the first SQL NULL",
            mode.ToString()
        );
    }
}

/// 非空首行时 FIRST_ROW 取第一行值，不向前扫描。
#[test]
fn scalar_first_row_returns_the_first_non_null_position_without_scanning_ahead() {
    let input = vec![Row(vec![SortValue::Int(5)]), Row(vec![SortValue::Int(7)])];
    for mode in [CompleteMode, Partial1Mode, FinalMode, Partial2Mode] {
        let rows = ExecutePhysicalPlan(&first_row_plan(mode), &FixedRowsSource(input.clone()))
            .expect("execute FIRST_ROW with populated input");
        assert_eq!(rows, vec![Row(vec![SortValue::Int(5)])]);
    }
}

/// 大表 COUNT(*) 必须走只计数边界，禁止物化或解码输入行。
#[test]
fn scalar_count_streams_large_input_without_materializing_rows() {
    const ROWS: usize = 250_000;
    let source = StreamingRowsSource::new(ROWS);
    let rows = ExecutePhysicalPlan(&count_plan(), &source)
        .expect("stream scalar COUNT over generated rows");
    assert_eq!(rows, vec![Row(vec![SortValue::Int(ROWS as i64)])]);
    assert_eq!(
        source.materialized_scans.load(Ordering::SeqCst),
        0,
        "COUNT must use the streaming table-source boundary"
    );
    assert_eq!(source.streamed_rows.load(Ordering::SeqCst), 0);
    assert_eq!(source.count_requests.load(Ordering::SeqCst), 1);
}

#[test]
fn scalar_count_extrema_uses_typed_kernel_for_nulls_and_duplicates() {
    for name in [
        astersql_parser_ast::AggFuncMaxCount,
        astersql_parser_ast::AggFuncMinCount,
    ] {
        for mode in [CompleteMode, Partial1Mode, FinalMode, Partial2Mode] {
            let mut plan = first_row_plan(mode);
            plan.BasePhysicalAgg.AggFuncs[0].Name = name.to_owned();
            let input = vec![
                Row(vec![SortValue::Null]),
                Row(vec![SortValue::Int(1)]),
                Row(vec![SortValue::Int(1)]),
                Row(vec![SortValue::Int(2)]),
            ];
            let result = ExecutePhysicalPlan(&plan, &FixedRowsSource(input)).unwrap();
            assert_eq!(
                result,
                vec![Row(vec![SortValue::Int(
                    if name == astersql_parser_ast::AggFuncMaxCount {
                        1
                    } else {
                        2
                    }
                )])]
            );
            assert_eq!(
                ExecutePhysicalPlan(&plan, &FixedRowsSource(vec![])).unwrap(),
                vec![Row(vec![SortValue::Int(0)])]
            );
        }
    }
}

#[test]
fn datum_mysql_time_converts_without_loss() {
    use crate::physical_plan_runtime::datum_to_result_value;
    use astersql_parser_mysql::r#type as mysql;
    use astersql_types::datum::{Datum, KindMysqlTime, NewTimeDatum};
    use astersql_types::time::{ParseTime, StrictContext};

    for (tp, fsp, text) in [
        (mysql::TypeDate, 0, "2000-01-02"),
        (mysql::TypeDatetime, 0, "2000-01-01 00:10:00"),
        (mysql::TypeDatetime, 3, "2000-01-01 00:10:00.120"),
        (mysql::TypeDatetime, 6, "2000-01-01 00:10:00.123456"),
        (mysql::TypeTimestamp, 6, "2000-01-01 00:10:00.000001"),
    ] {
        let time = ParseTime(&StrictContext, text, tp, fsp).unwrap();
        let datum = NewTimeDatum(time);
        assert_eq!(datum.Kind(), KindMysqlTime);
        assert_eq!(datum.GetMysqlTime().Type(), tp);
        assert_eq!(datum.GetMysqlTime().Fsp(), fsp);
        let value = datum_to_result_value(datum).expect("convert typed time result");
        let SortValue::Bytes(bytes) = value else {
            panic!("expected final result text")
        };
        let actual = String::from_utf8(bytes).unwrap();
        assert_eq!(actual, text);
        // Re-read with the result field's type/FSP, as GetDatum does: check
        // temporal value and precision rather than only text equality.
        let restored = ParseTime(&StrictContext, &actual, tp, fsp).unwrap();
        assert_eq!(restored.Type(), time.Type());
        assert_eq!(restored.Fsp(), time.Fsp());
        assert_eq!(restored.Compare(time), 0);
    }
    let earlier = ParseTime(
        &StrictContext,
        "2000-01-01 00:10:00.123456",
        mysql::TypeDatetime,
        6,
    )
    .unwrap();
    let later = ParseTime(
        &StrictContext,
        "2000-01-01 00:10:00.123457",
        mysql::TypeDatetime,
        6,
    )
    .unwrap();
    assert!(earlier.Compare(later) < 0);
    assert!(
        datum_to_result_value(NewTimeDatum(earlier)).unwrap()
            < datum_to_result_value(NewTimeDatum(later)).unwrap()
    );
    assert_eq!(
        datum_to_result_value(Datum::default()).unwrap(),
        SortValue::Null
    );
}

#[test]
fn pg09_23_execution() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let session = astersql_session::runtime::ConcreteSession::new_with_dialect(
        domain,
        astersql_parser_ast::SqlDialect::PostgreSQL,
    );
    let mut result = session
        .execute("SELECT g.v AS c FROM generate_series(1::int4,3::int4) AS g(v) ORDER BY g.v")
        .unwrap();
    let mut rows = Vec::new();
    while let Some(row) = result[0].next_row().unwrap() {
        rows.push(row);
    }
    assert_eq!(
        rows,
        vec![
            vec!["1".to_owned()],
            vec!["2".to_owned()],
            vec!["3".to_owned()]
        ]
    );
    result[0].close().unwrap();
}

#[test]
fn pg09_23_execution_chunks() {
    for (arguments, expected) in [
        ([Some(1), Some(7), Some(2)], vec![1, 3, 5, 7]),
        ([Some(3), Some(-3), Some(-2)], vec![3, 1, -1, -3]),
        (
            [Some(i32::MAX - 1), Some(i32::MAX), Some(1)],
            vec![i32::MAX - 1, i32::MAX],
        ),
        (
            [Some(i32::MIN + 1), Some(i32::MIN), Some(-1)],
            vec![i32::MIN + 1, i32::MIN],
        ),
        (
            [Some(i32::MAX), Some(i32::MAX), Some(i32::MAX)],
            vec![i32::MAX],
        ),
        (
            [Some(i32::MIN), Some(i32::MIN), Some(i32::MIN)],
            vec![i32::MIN],
        ),
        ([Some(3), Some(1), Some(1)], vec![]),
        ([None, Some(1), Some(0)], vec![]),
        ([Some(1), None, Some(0)], vec![]),
        ([Some(1), Some(1), None], vec![]),
    ] {
        let mut source = crate::builder::BuildInt4Series(arguments, 2);
        let mut chunk = source.NewChunk();
        assert!(source.Next(&mut chunk).is_err());
        for _ in 0..2 {
            source.Open().unwrap();
            let mut actual = Vec::new();
            loop {
                source.Next(&mut chunk).unwrap();
                assert!(chunk.NumRows() <= 2);
                if chunk.NumRows() == 0 {
                    break;
                }
                for row in 0..chunk.NumRows() {
                    actual.push(chunk.GetRow(row).GetInt64(0) as i32);
                }
            }
            assert_eq!(actual, expected);
            source.Next(&mut chunk).unwrap();
            assert_eq!(chunk.NumRows(), 0);
            source.Close().unwrap();
            source.Close().unwrap();
            assert!(source.Next(&mut chunk).is_err());
        }
    }
    let mut source = crate::builder::BuildInt4Series([Some(1), Some(2), Some(0)], 2);
    assert_eq!(
        source.Open().unwrap_err().to_string(),
        "step size cannot equal zero"
    );
    source.Close().unwrap();
    // Early close discards the unread page tail; Open restarts at the argument.
    let mut source = crate::builder::BuildInt4Series([Some(1), Some(9), Some(1)], 2);
    source.Open().unwrap();
    let mut chunk = source.NewChunk();
    source.Next(&mut chunk).unwrap();
    assert_eq!(chunk.NumRows(), 2);
    source.Close().unwrap();
    source.Open().unwrap();
    source.Next(&mut chunk).unwrap();
    assert_eq!(chunk.GetRow(0).GetInt64(0), 1);
    source.Close().unwrap();
}

#[test]
fn pg09_24_execution() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let session = astersql_session::runtime::ConcreteSession::new_with_dialect(
        domain,
        astersql_parser_ast::SqlDialect::PostgreSQL,
    );
    let mut result = session
        .execute("SELECT g.v AS c FROM generate_series('9223372036854775806'::int8,'9223372036854775807'::int8) AS g(v) ORDER BY g.v")
        .unwrap();
    let mut rows = Vec::new();
    while let Some(row) = result[0].next_row().unwrap() {
        rows.push(row);
    }
    assert_eq!(
        rows,
        vec![
            vec!["9223372036854775806".to_owned()],
            vec!["9223372036854775807".to_owned()]
        ]
    );
    result[0].close().unwrap();
}

#[test]
fn pg09_24_execution_chunks() {
    for (arguments, expected) in [
        ([Some(1), Some(7), Some(2)], vec![1, 3, 5, 7]),
        ([Some(3), Some(-3), Some(-2)], vec![3, 1, -1, -3]),
        (
            [Some(i64::MAX - 1), Some(i64::MAX), Some(1)],
            vec![i64::MAX - 1, i64::MAX],
        ),
        (
            [Some(i64::MIN + 1), Some(i64::MIN), Some(-1)],
            vec![i64::MIN + 1, i64::MIN],
        ),
        (
            [Some(i64::MAX), Some(i64::MAX), Some(i64::MAX)],
            vec![i64::MAX],
        ),
        (
            [Some(i64::MIN), Some(i64::MIN), Some(i64::MIN)],
            vec![i64::MIN],
        ),
        ([Some(3), Some(1), Some(1)], vec![]),
        (
            [Some(9007199254740993), Some(9007199254740997), Some(1)],
            vec![
                9007199254740993,
                9007199254740994,
                9007199254740995,
                9007199254740996,
                9007199254740997,
            ],
        ),
        ([None, Some(1), Some(0)], vec![]),
        ([Some(1), None, Some(0)], vec![]),
        ([Some(1), Some(1), None], vec![]),
    ] {
        let mut source = crate::builder::BuildInt8Series(arguments, 2);
        assert_eq!(
            source.Schema()[0].field_type.GetType(),
            astersql_parser_mysql::r#type::TypeLonglong
        );
        let mut chunk = source.NewChunk();
        assert!(source.Next(&mut chunk).is_err());
        for _ in 0..2 {
            source.Open().unwrap();
            let mut actual = Vec::new();
            loop {
                source.Next(&mut chunk).unwrap();
                assert!(chunk.NumRows() <= 2);
                if chunk.NumRows() == 0 {
                    break;
                }
                for row in 0..chunk.NumRows() {
                    actual.push(chunk.GetRow(row).GetInt64(0));
                }
            }
            assert_eq!(actual, expected);
            source.Next(&mut chunk).unwrap();
            assert_eq!(chunk.NumRows(), 0);
            source.Close().unwrap();
            source.Close().unwrap();
            assert!(source.Next(&mut chunk).is_err());
        }
    }
    let mut source = crate::builder::BuildInt8Series([Some(1), Some(2), Some(0)], 2);
    assert_eq!(
        source.Open().unwrap_err().to_string(),
        "step size cannot equal zero"
    );
    source.Close().unwrap();
    // Early close discards the unread page tail; Open restarts at the argument.
    let mut source = crate::builder::BuildInt8Series([Some(1), Some(9), Some(1)], 2);
    source.Open().unwrap();
    let mut chunk = source.NewChunk();
    source.Next(&mut chunk).unwrap();
    assert_eq!(chunk.NumRows(), 2);
    source.Close().unwrap();
    source.Open().unwrap();
    source.Next(&mut chunk).unwrap();
    assert_eq!(chunk.GetRow(0).GetInt64(0), 1);
    source.Close().unwrap();
}

#[test]
fn pg09_25_execution() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let session = astersql_session::runtime::ConcreteSession::new_with_dialect(
        domain,
        astersql_parser_ast::SqlDialect::PostgreSQL,
    );
    let mut results = session
        .execute("SELECT u.v AS c FROM unnest(ARRAY[2,NULL,2]::int4[]) AS u(v)")
        .unwrap();
    let mut rows = Vec::new();
    while let Some(row) = results[0].next_row().unwrap() {
        rows.push(row);
    }
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0], vec!["2".to_string()]);
    assert_eq!(rows[2], vec!["2".to_string()]);
    results[0].close().unwrap();
}

#[test]
fn pg09_25_execution_chunks() {
    for elements in [
        vec![],
        vec![None, None],
        vec![Some(2), None, Some(2), Some(i32::MIN), Some(i32::MAX)],
    ] {
        let mut source = crate::builder::BuildInt4Unnest(elements.clone(), 2);
        let mut chunk = source.NewChunk();
        assert!(source.Next(&mut chunk).is_err());
        for _ in 0..2 {
            source.Open().unwrap();
            let mut values = Vec::new();
            loop {
                source.Next(&mut chunk).unwrap();
                assert!(chunk.NumRows() <= 2);
                if chunk.NumRows() == 0 {
                    break;
                }
                for i in 0..chunk.NumRows() {
                    let row = chunk.GetRow(i);
                    values.push(if row.IsNull(0) {
                        None
                    } else {
                        Some(row.GetInt64(0) as i32)
                    });
                }
            }
            assert_eq!(values, elements);
            source.Close().unwrap();
            source.Close().unwrap();
            assert!(source.Next(&mut chunk).is_err());
        }
        source.Open().unwrap();
        source.Next(&mut chunk).unwrap();
        source.Close().unwrap();
        source.Open().unwrap();
        source.Next(&mut chunk).unwrap();
        assert_eq!(chunk.NumRows(), elements.len().min(2));
        source.Close().unwrap();
    }
}

#[test]
fn pg09_26_execution() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let session = astersql_session::runtime::ConcreteSession::new_with_dialect(
        domain,
        astersql_parser_ast::SqlDialect::PostgreSQL,
    );
    // The server adapter maps PG's bare tuples to the shared VALUES ROW AST.
    for (join, count) in [("CROSS JOIN", 2), ("LEFT JOIN LATERAL", 4)] {
        let on = if join.starts_with("LEFT") {
            " ON true"
        } else {
            ""
        };
        let sql = format!(
            "SELECT t.id AS id,g.v AS v FROM (VALUES ROW(1::int4,2::int4),ROW(2,0),ROW(3,NULL::int4)) t(id,n) {join} generate_series(1::int4,t.n,1::int4) AS g(v){on} ORDER BY t.id,g.v"
        );
        let mut sets = session.execute(&sql).unwrap();
        let mut rows = Vec::new();
        while let Some(row) = sets[0].next_row().unwrap() {
            rows.push(row);
        }
        assert_eq!(rows.len(), count, "{sql}: {rows:?}");
        assert_eq!(rows[0], vec!["1".to_string(), "1".to_string()]);
        assert_eq!(rows[1], vec!["1".to_string(), "2".to_string()]);
        sets[0].close().unwrap();
    }
}

#[test]
fn pg09_26_execution_chunks() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let session = astersql_session::runtime::ConcreteSession::new_with_dialect(
        domain,
        astersql_parser_ast::SqlDialect::PostgreSQL,
    );
    let mut sets = session.execute("SELECT t.id AS id,g.v AS v FROM (VALUES ROW(1::int4,1025::int4),ROW(2,0),ROW(3,NULL::int4),ROW(4,2)) t(id,n) CROSS JOIN generate_series(1::int4,t.n) g(v) ORDER BY t.id,g.v").unwrap();
    let mut rows = Vec::new();
    while let Some(row) = sets[0].next_row().unwrap() {
        rows.push(row);
    }
    assert_eq!(rows.len(), 1027);
    for (i, row) in rows[..1025].iter().enumerate() {
        assert_eq!(row, &vec!["1".to_string(), (i + 1).to_string()]);
    }
    assert_eq!(rows[1025], vec!["4".to_string(), "1".to_string()]);
    assert_eq!(rows[1026], vec!["4".to_string(), "2".to_string()]);
    sets[0].close().unwrap();
}

#[test]
fn pg09_27_execution() {
    let (domain, _) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let session = astersql_session::runtime::ConcreteSession::new_with_dialect(
        domain,
        astersql_parser_ast::SqlDialect::PostgreSQL,
    );
    let sql = "SELECT t.id AS id,generate_series(1::int4,t.n,1::int4) AS v FROM (VALUES ROW(1::int4,2::int4),ROW(2,0),ROW(3,NULL::int4),ROW(4,1)) t(id,n) ORDER BY t.id,v";
    let mut sets = session.execute(sql).unwrap();
    let mut rows = Vec::new();
    while let Some(row) = sets[0].next_row().unwrap() {
        rows.push(row);
    }
    assert_eq!(rows, vec![vec!["1", "1"], vec!["1", "2"], vec!["4", "1"]]);
    sets[0].close().unwrap();
}
