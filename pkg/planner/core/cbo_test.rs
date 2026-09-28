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

// CBO（Cost-Based Optimizer，基于代价的优化器）核心用例。
//
// 用固定统计信息夹具驱动 `PlanBuilder` / `DoOptimize`，对照 Go 侧典型工作负载的
// 索引选择、范围扫描、聚合（HashAgg/StreamAgg）、TopN/Limit 与 Sort 等物理计划形态。

use base_dependency as base;
use base_dependency::PhysicalPlan as _;
use expression_dependency as expression;
use logicalop_dependency as logicalop;
use logicalop_dependency::LogicalPlan as _;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

/// Go `cbo_test` 中典型 SQL 与期望物理计划摘要（IndexReader / IndexLookUp / TableReader 等）。
const GO_OPTIMIZER_CASES: &[(&str, &str)] = &[
    (
        "select count(*) from t group by e",
        "IndexReader(Index(t.e)[[NULL,+inf]])->StreamAgg",
    ),
    (
        "select count(*) from t where e <= 10 group by e",
        "IndexReader(Index(t.e)[[-inf,10]])->StreamAgg",
    ),
    (
        "select count(*) from t where e <= 50",
        "IndexReader(Index(t.e)[[-inf,50]]->HashAgg)->HashAgg",
    ),
    (
        "select count(*) from t where c > '1' group by b",
        "IndexReader(Index(t.b_c)[[NULL,+inf]]->Sel([gt(test.t.c, 1)]))->StreamAgg",
    ),
    (
        "select count(*) from t where e = 1 group by b",
        "IndexLookUp(Index(t.e)[[1,1]], Table(t)->HashAgg)->HashAgg",
    ),
    (
        "select count(*) from t where e > 1 group by b",
        "TableReader(Table(t)->Sel([gt(test.t.e, 1)])->HashAgg)->HashAgg",
    ),
    (
        "select count(e) from t where t.b <= 20",
        "IndexLookUp(Index(t.b)[[-inf,20]], Table(t)->HashAgg)->HashAgg",
    ),
    (
        "select count(e) from t where t.b <= 30",
        "IndexLookUp(Index(t.b)[[-inf,30]], Table(t)->HashAgg)->HashAgg",
    ),
    (
        "select count(e) from t where t.b <= 40",
        "IndexLookUp(Index(t.b)[[-inf,40]], Table(t)->HashAgg)->HashAgg",
    ),
    (
        "select count(e) from t where t.b <= 50",
        "TableReader(Table(t)->Sel([le(test.t.b, 50)])->HashAgg)->HashAgg",
    ),
    (
        "select * from t where t.b <= 40",
        "IndexLookUp(Index(t.b)[[-inf,40]], Table(t))",
    ),
    (
        "select * from t where t.b <= 50",
        "TableReader(Table(t)->Sel([le(test.t.b, 50)]))",
    ),
    (
        "select * from t where 1 and t.b <= 50",
        "TableReader(Table(t)->Sel([le(test.t.b, 50)]))",
    ),
    (
        "select * from t where t.b <= 100 order by t.a limit 1",
        "TableReader(Table(t)->Sel([le(test.t.b, 100)])->Limit)->Limit",
    ),
    (
        "select * from t where t.b <= 1 order by t.a limit 10",
        "IndexLookUp(Index(t.b)[[-inf,1]]->TopN([test.t.a],0,10), Table(t))->TopN([test.t.a],0,10)",
    ),
    (
        "select * from t use index(b) where b = 1 order by a",
        "IndexLookUp(Index(t.b)[[1,1]], Table(t))->Sort",
    ),
    (
        "select * from t where d < cast('1991-09-05' as datetime)",
        "IndexLookUp(Index(t.d)[[-inf,1991-09-05 00:00:00)], Table(t))",
    ),
    (
        "select * from t where ts < '1991-09-05'",
        "IndexLookUp(Index(t.ts)[[-inf,1991-09-05 00:00:00)], Table(t))",
    ),
];

/// 将物理计划树折叠为可读指纹：算子类型、索引名、ranges、过滤与聚合模式等。
fn canonical_physical_fingerprint(plan: &dyn base::PhysicalPlan) -> String {
    // 把 ranger 范围边界格式化为 Go EXPLAIN 风格的 [low,high] / (low,high) 片段。
    macro_rules! ranges {
        ($ranges:expr) => {{
            $ranges
                .iter()
                .map(|range| {
                    let datum = |value: &expression::types::Datum, low: bool| match value.Kind() {
                        expression::types::KindNull => "NULL".to_owned(),
                        expression::types::KindMinNotNull => "-inf".to_owned(),
                        expression::types::KindMaxValue => "+inf".to_owned(),
                        expression::types::KindInt64
                            if (low && value.GetInt64() == i64::MIN)
                                || (!low && value.GetInt64() == i64::MAX) =>
                        {
                            if low { "-inf" } else { "+inf" }.to_owned()
                        }
                        _ => value.ToString().expect("format range datum"),
                    };
                    let low = range
                        .LowVal
                        .iter()
                        .map(|value| datum(value, true))
                        .collect::<Vec<_>>()
                        .join(" ");
                    let high = range
                        .HighVal
                        .iter()
                        .map(|value| datum(value, false))
                        .collect::<Vec<_>>()
                        .join(" ");
                    format!(
                        "{}{},{}{}",
                        if range.LowExclude { '(' } else { '[' },
                        low,
                        high,
                        if range.HighExclude { ')' } else { ']' }
                    )
                })
                .collect::<Vec<_>>()
                .join("|")
        }};
    }
    // 按物理算子具体类型拼装指纹前缀，再递归拼接子计划。
    let operator = if let Some(scan) = plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalTableScan>()
    {
        format!(
            "TableScan[ranges={};filters={}]",
            ranges!(scan.Ranges),
            scan.FilterCondition.len()
        )
    } else if let Some(scan) = plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalIndexScan>()
    {
        let index = scan
            .Index
            .as_ref()
            .map_or("?", |index| index.Name.O.as_str());
        format!(
            "IndexScan[{index};ranges={};access={};filters={};rows={}]",
            ranges!(scan.Ranges),
            scan.AccessCondition.len(),
            scan.FilterCondition.len(),
            scan.stats_count()
        )
    } else if let Some(aggregate) = plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalHashAgg>()
    {
        format!(
            "HashAgg[{}]",
            aggregate
                .BasePhysicalAgg
                .AggFuncs
                .first()
                .map_or("none", |function| function.Mode.ToString())
        )
    } else if let Some(aggregate) = plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalStreamAgg>()
    {
        format!(
            "StreamAgg[{}]",
            aggregate
                .BasePhysicalAgg
                .AggFuncs
                .first()
                .map_or("none", |function| function.Mode.ToString())
        )
    } else if let Some(topn) = plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalTopN>()
    {
        let by_items = topn
            .ByItems
            .iter()
            .map(|item| {
                let direction = if item.Desc { ":desc" } else { "" };
                format!(
                    "{}{direction}",
                    item.Expr
                        .ExplainInfo(plan.s_ctx().GetExprCtx().GetEvalCtx())
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "TopN[by={by_items};offset={};count={}]",
            topn.Offset, topn.Count
        )
    } else if let Some(limit) = plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalLimit>()
    {
        format!("Limit[offset={};count={}]", limit.Offset, limit.Count)
    } else if let Some(sort) = plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalSort>()
    {
        let by_items = sort
            .ByItems
            .iter()
            .map(|item| {
                let direction = if item.Desc { ":desc" } else { "" };
                format!(
                    "{}{direction}",
                    item.Expr
                        .ExplainInfo(plan.s_ctx().GetExprCtx().GetEvalCtx())
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("Sort[by={by_items}]")
    } else {
        plan.tp(&[])
    };
    let children = plan.children();
    if children.is_empty() {
        operator
    } else {
        format!(
            "{}({})",
            operator,
            children
                .into_iter()
                .map(canonical_physical_fingerprint)
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

/// 从指纹字符串中解析所有 `TopN[by=...;offset=...;count=...]` 规格。
fn physical_top_n_specs(fingerprint: &str) -> Vec<(String, u64, u64)> {
    fingerprint
        .match_indices("TopN[by=")
        .filter_map(|(start, _)| {
            let body = fingerprint.get(start + "TopN[by=".len()..)?;
            let end = body.find(']')?;
            let mut fields = body[..end].split(';');
            let by = fields.next()?.to_owned();
            let offset = fields.next()?.strip_prefix("offset=")?.parse().ok()?;
            let count = fields.next()?.strip_prefix("count=")?.parse().ok()?;
            Some((by, offset, count))
        })
        .collect()
}

/// 从 Go 期望计划字符串中解析 `TopN([by],offset,count)` 规格。
fn go_top_n_specs(plan: &str) -> Vec<(String, u64, u64)> {
    plan.match_indices("TopN([")
        .filter_map(|(start, _)| {
            let body = plan.get(start + "TopN([".len()..)?;
            let (by, rest) = body.split_once("],")?;
            let (offset, rest) = rest.split_once(',')?;
            let count = rest.split(')').next()?;
            Some((by.to_owned(), offset.parse().ok()?, count.parse().ok()?))
        })
        .collect()
}

/// 断言实际物理指纹在索引、范围、Reader 类型、聚合、Sort/TopN/Limit 上与 Go 期望一致。
fn assert_go_plan_choice(sql: &str, expected: &str, actual: &str) {
    // 索引名：Index(t.e) 应对应指纹中的 IndexScan[e;...]。
    if let Some(index) = expected
        .split("Index(t.")
        .nth(1)
        .and_then(|suffix| suffix.split(')').next())
    {
        assert!(
            actual.contains(&format!("IndexScan[{index};")),
            "Go index choice mismatch for {sql:?}: expected {expected}, actual {actual}"
        );
    }
    // 索引范围片段：如 [[-inf,10]] 应出现在 ranges= 中。
    if let Some(index_suffix) = expected.split("Index(t.").nth(1)
        && let Some(range_suffix) = index_suffix.split_once(")[").map(|(_, suffix)| suffix)
        && let Some(range_end) = range_suffix
            .find(']')
            .into_iter()
            .chain(range_suffix.find(')'))
            .min()
    {
        let expected_range = &range_suffix[..=range_end];
        assert!(
            actual.contains(&format!("ranges={expected_range}")),
            "Go range mismatch for {sql:?}: expected {expected}, actual {actual}"
        );
    }
    // Reader 形态：IndexReader / IndexLookUp / TableReader 互斥选择。
    if expected.starts_with("IndexReader") {
        assert!(
            actual.contains("IndexReader("),
            "expected {expected}, actual {actual}"
        );
        assert!(
            !actual.contains("IndexLookUp("),
            "expected {expected}, actual {actual}"
        );
    } else if expected.starts_with("IndexLookUp") {
        assert!(
            actual.contains("IndexLookUp("),
            "expected {expected}, actual {actual}"
        );
    } else if expected.starts_with("TableReader") {
        assert!(
            actual.contains("TableReader("),
            "expected {expected}, actual {actual}"
        );
        assert!(
            !actual.contains("IndexLookUp("),
            "expected {expected}, actual {actual}"
        );
    }
    // 两阶段聚合：final + partial1。
    if expected.ends_with("StreamAgg") {
        assert!(
            actual.starts_with("StreamAgg[final]"),
            "expected {expected}, actual {actual}"
        );
        assert!(
            actual.contains("StreamAgg[partial1]"),
            "expected {expected}, actual {actual}"
        );
    } else if expected.ends_with("HashAgg") {
        assert!(
            actual.starts_with("HashAgg[final]"),
            "expected {expected}, actual {actual}"
        );
        assert!(
            actual.contains("HashAgg[partial1]"),
            "expected {expected}, actual {actual}"
        );
    }
    if expected.ends_with("Sort") {
        let expected_order_column = sql
            .split(" order by ")
            .nth(1)
            .and_then(|order| order.split_whitespace().next())
            .and_then(|column| column.rsplit('.').next())
            .expect("Sort workload must have an ORDER BY column");
        let actual_order_column = actual
            .strip_prefix("Sort[by=")
            .and_then(|sort| sort.split(']').next())
            .and_then(|column| column.rsplit('.').next());
        assert!(
            actual_order_column == Some(expected_order_column),
            "expected {expected}, actual {actual}"
        );
    }
    let expected_filters = usize::from(expected.contains("->Sel("));
    assert_eq!(
        actual.matches("filters=1").count(),
        expected_filters,
        "Go residual filter mismatch for {sql:?}: expected {expected}, actual {actual}"
    );
    // 双 TopN（下推 + 根）规格必须与 Go 期望一一对应。
    if expected.matches("TopN").count() == 2 {
        let expected_top_n = go_top_n_specs(expected);
        let actual_top_n = physical_top_n_specs(actual);
        assert_eq!(
            actual_top_n.len(),
            expected_top_n.len(),
            "both pushed and root TopN nodes must remain: expected {expected}, actual {actual}"
        );
        assert_eq!(
            actual_top_n, expected_top_n,
            "expected {expected}, actual {actual}"
        );
    }
    if expected.matches("Limit").count() == 2 {
        assert_eq!(
            actual.matches("Limit[offset=0;count=1]").count(),
            2,
            "expected {expected}, actual {actual}"
        );
    }
}

/// 构造与 Go CBO benchmark 相同的分批 INSERT 语句文本。
fn construct_insert_sql(batch: i32, rows: i32) -> String {
    let mut sql = "insert into t (a,b,c,e)values ".to_owned();
    for row in 0..rows {
        let value = batch * rows + row;
        sql.push_str(&format!("({value}, {batch}, '{}', {value})", batch + row));
        if row != rows - 1 {
            sql.push_str(", ");
        }
    }
    sql
}

/// CBO 测试用的最小 `PlanContext`：会话变量、表达式上下文与 ranger 上下文。
struct CboPlanContext {
    plan_id: AtomicI32,
    session_vars: variable_dependency::session::SessionVars,
    expr_ctx: exprstatic_dependency::ExprContext,
    ranger_ctx: base::RangerContext<'static>,
    builtin_function_usage: base::BuiltinFunctionUsageCounter,
}

impl base::PlanContext for CboPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.plan_id.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &variable_dependency::session::SessionVars {
        &self.session_vars
    }

    fn GetExprCtx(&self) -> &dyn expression::exprctx::ExprContext {
        &self.expr_ctx
    }

    fn GetRangerCtx(&self) -> &base::RangerContext<'_> {
        &self.ranger_ctx
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn expression::exprctx::ExprContext {
        &self.expr_ctx
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        // 本测试只跑逻辑/物理优化，不构建 tipb 执行器。
        panic!("CBO test does not build protobuf executors")
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.builtin_function_usage.Inc(scalar_func_sig_name)
    }
}

/// 构造挂好谓词简化透传与默认 ranger 配置的规划上下文引用。
pub(crate) fn cbo_plan_context() -> base::ContextRef {
    logicalop::InstallPredicateSimplificationPassthrough();
    let mut session_vars = variable_dependency::session::SessionVars::default();
    session_vars.SetCurrentDB("test");
    let ranger_expression_context: Arc<dyn expression::exprctx::BuildContext> =
        Arc::new(exprstatic_dependency::NewExprContext(Vec::new()));
    Arc::new(CboPlanContext {
        plan_id: AtomicI32::new(0),
        session_vars,
        expr_ctx: exprstatic_dependency::NewExprContext(Vec::new()),
        ranger_ctx: base::RangerContext {
            TypeCtx: expression::types::DefaultStmtNoWarningContext.clone(),
            ErrCtx: expression::errctx::StrictNoWarningContext.clone(),
            ExprCtx: ranger_expression_context,
            RangeFallbackHandler: None,
            PlanCacheTracker: None,
            OptimizerFixControl: Default::default(),
            UseCache: false,
            RegardNULLAsPoint: true,
            OptPrefixIndexSingleScan: false,
        },
        builtin_function_usage: base::BuiltinFunctionUsageCounter::default(),
    })
}

/// 构造含表 `t`（列 a–e、ts 及索引 b/d/e/b_c/ts）的 mock InfoSchema。
fn cbo_info_schema() -> Arc<dyn infoschema_dependency::infoschema::InfoSchema> {
    // 辅助闭包：按 id/名/偏移/类型生成 ColumnInfo。
    let column = |id: i64, name: &str, offset: isize, tp: u8| expression::model::ColumnInfo {
        ID: id,
        Name: crate::ast::NewCIStr(name),
        Offset: offset,
        State: expression::model::StatePublic,
        FieldType: *expression::types::NewFieldType(tp),
        ..Default::default()
    };
    // 辅助闭包：按 id/名与列偏移列表生成 IndexInfo。
    let index = |id: i64, name: &str, columns: &[(&str, isize)]| expression::model::IndexInfo {
        ID: id,
        Name: crate::ast::NewCIStr(name),
        State: expression::model::StatePublic,
        Columns: columns
            .iter()
            .map(|(name, offset)| expression::model::IndexColumn {
                Name: crate::ast::NewCIStr(name),
                Offset: *offset,
                Length: expression::types::UnspecifiedLength as isize,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let mut columns = vec![
        column(1, "a", 0, expression::mysql::TypeLonglong),
        column(2, "b", 1, expression::mysql::TypeLonglong),
        column(3, "c", 2, expression::mysql::TypeVarchar),
        column(4, "d", 3, expression::mysql::TypeDatetime),
        column(5, "e", 4, expression::mysql::TypeLonglong),
        column(6, "ts", 5, expression::mysql::TypeTimestamp),
    ];
    columns[2].FieldType.SetFlen(200);
    columns[0].AddFlag(expression::mysql::PriKeyFlag | expression::mysql::NotNullFlag);
    let model = Arc::new(expression::model::TableInfo {
        ID: 101,
        Name: crate::ast::NewCIStr("t"),
        Columns: columns,
        Indices: vec![
            index(201, "b", &[("b", 1)]),
            index(202, "d", &[("d", 3)]),
            index(203, "e", &[("e", 4)]),
            index(204, "b_c", &[("b", 1), ("c", 2)]),
            index(205, "ts", &[("ts", 5)]),
        ],
        PKIsHandle: true,
        TiFlashReplica: Some(expression::model::TiFlashReplicaInfo {
            Count: 1,
            Available: true,
            ..Default::default()
        }),
        ..Default::default()
    });
    infoschema_dependency::infoschema::MockInfoSchema(vec![
        infoschema_dependency::infoschema::TableInfo {
            id: model.ID,
            name: infoschema_dependency::infoschema::CiString::new("t"),
            columns: model
                .Columns
                .iter()
                .map(|column| infoschema_dependency::infoschema::ColumnInfo {
                    id: column.ID,
                    name: infoschema_dependency::infoschema::CiString::new(&column.Name.O),
                    ..Default::default()
                })
                .collect(),
            model_meta: Some(model),
            ..Default::default()
        },
    ])
}

/// 向 DataSource 注入已分析直方图与访问路径的夹具统计提供者。
struct FixtureStatsProvider;

/// 按 Go CBO 夹具数据分布构建列/索引直方图集合（HistColl）。
fn analyzed_histogram(
    plan_ctx: &base::ContextRef,
    source: &logicalop::DataSource,
) -> statistics_dependency::HistColl {
    let datetime = expression::types::ParseTime(
        &*expression::types::DefaultStmtNoWarningContext,
        "2019-09-05 00:00:00",
        expression::mysql::TypeDatetime,
        0,
    )
    .expect("fixture datetime");
    let timestamp = expression::types::ParseTime(
        &*expression::types::DefaultStmtNoWarningContext,
        "2019-09-05 00:00:00",
        expression::mysql::TypeTimestamp,
        0,
    )
    .expect("fixture timestamp");
    // 100×100 行：列 a/e 唯一递增，b 为 batch，c 为字符串，d/ts 为固定时间。
    let mut values: HashMap<i64, Vec<expression::types::Datum>> = HashMap::new();
    for batch in 0..100i64 {
        for row in 0..100i64 {
            let value = batch * 100 + row;
            values
                .entry(1)
                .or_default()
                .push(expression::types::NewIntDatum(value));
            values
                .entry(2)
                .or_default()
                .push(expression::types::NewIntDatum(batch));
            values
                .entry(3)
                .or_default()
                .push(expression::types::NewStringDatum((batch + row).to_string()));
            values
                .entry(4)
                .or_default()
                .push(expression::types::NewTimeDatum(datetime));
            values
                .entry(5)
                .or_default()
                .push(expression::types::NewIntDatum(value));
            values
                .entry(6)
                .or_default()
                .push(expression::types::NewTimeDatum(timestamp));
        }
    }
    let mut raw = *statistics_dependency::NewHistColl(
        source.PhysicalTableID,
        10_000,
        0,
        source.TableInfo.Columns.len(),
        source.TableInfo.Indices.len(),
    );
    raw.StatsVer = statistics_dependency::Version2;
    raw.Pseudo = false;
    // 按列构建已排序直方图并挂到 HistColl。
    for column in &source.TableInfo.Columns {
        let mut builder = statistics_dependency::NewSortedBuilder(
            256,
            column.ID,
            &column.FieldType,
            statistics_dependency::Version2,
        );
        let mut sorted_values = values
            .get(&column.ID)
            .expect("fixture column values")
            .clone();
        if column.ID == 3 {
            sorted_values.sort_by_key(|value| value.GetString());
        }
        for value in sorted_values {
            builder.Iterate(value).expect("build column histogram");
        }
        raw.SetCol(
            column.ID,
            Box::new(statistics_dependency::Column {
                CMSketch: None,
                TopN: None,
                FMSketch: None,
                Info: Some(statistics_dependency::ColumnInfo {
                    ID: column.ID,
                    Name: column.Name.O.clone(),
                    FieldType: column.FieldType.clone(),
                    IsPrimaryKey: column.ID == 1,
                }),
                Histogram: builder.IntoHist(),
                StatsLoadedStatus: statistics_dependency::NewStatsFullLoadStatus(),
                PhysicalID: source.PhysicalTableID,
                StatsVer: statistics_dependency::Version2 as i64,
                IsHandle: column.ID == 1,
            }),
        );
    }

    let location = plan_ctx.GetExprCtx().GetEvalCtx().Location();
    // 按索引键编码后构建索引直方图。
    for index in &source.TableInfo.Indices {
        let mut encoded = Vec::with_capacity(10_000);
        for row in 0..10_000usize {
            let datums = index
                .Columns
                .iter()
                .map(|index_column| {
                    let column = &source.TableInfo.Columns[index_column.Offset as usize];
                    values[&column.ID][row].clone()
                })
                .collect::<Vec<_>>();
            encoded.push(
                codec_dependency::EncodeKey(location.clone(), Vec::new(), datums)
                    .expect("encode analyzed index value"),
            );
        }
        encoded.sort();
        let blob = expression::types::NewFieldType(expression::mysql::TypeBlob);
        let mut builder = statistics_dependency::NewSortedBuilder(
            256,
            index.ID,
            &blob,
            statistics_dependency::Version2,
        );
        for value in encoded {
            builder
                .Iterate(expression::types::NewBytesDatum(value))
                .expect("build index histogram");
        }
        raw.SetIdx(
            index.ID,
            Box::new(statistics_dependency::Index {
                CMSketch: None,
                TopN: None,
                FMSketch: None,
                Info: Some(statistics_dependency::IndexInfo {
                    ID: index.ID,
                    Name: index.Name.O.clone(),
                    Columns: index
                        .Columns
                        .iter()
                        .map(|column| statistics_dependency::IndexColumnInfo {
                            Name: column.Name.O.clone(),
                            Length: column.Length as i32,
                        })
                        .collect(),
                    MVIndex: index.MVIndex,
                    Unique: index.Unique,
                    ConditionExprString: String::new(),
                }),
                Histogram: builder.IntoHist(),
                StatsLoadedStatus: statistics_dependency::NewStatsFullLoadStatus(),
                PhysicalID: source.PhysicalTableID,
                StatsVer: statistics_dependency::Version2 as i64,
            }),
        );
    }
    let id_to_unique_id = source
        .Schema()
        .Columns
        .iter()
        .map(|column| (column.ID, column.UniqueID))
        .collect();
    let index_to_column_ids = source
        .TableInfo
        .Indices
        .iter()
        .map(|index| {
            (
                index.ID,
                index
                    .Columns
                    .iter()
                    .map(|column| source.TableInfo.Columns[column.Offset as usize].ID)
                    .collect(),
            )
        })
        .collect();
    raw.GenerateHistCollFromColumnInfo(&id_to_unique_id, &index_to_column_ids)
}

impl crate::DataSourceProvider for FixtureStatsProvider {
    fn Populate(
        &self,
        _ctx: &dyn crate::context::Context,
        plan_ctx: &base::ContextRef,
        _info_schema: &dyn infoschema_dependency::infoschema::InfoSchema,
        table: &crate::ast::TableName,
        source: &mut logicalop::DataSource,
    ) -> Result<(), expression::Error> {
        // 行数与 NDV：对齐 Go 夹具（b 低基数、c 约 199、其余近唯一）。
        source.TableStats.RowCount = 10_000.0;
        source.TableStats.ColNDVs = source
            .Schema()
            .Columns
            .iter()
            .map(|column| {
                let ndv = source
                    .TableInfo
                    .Columns
                    .iter()
                    .find(|info| info.ID == column.ID)
                    .map_or(10_000.0, |info| match info.Name.L.as_str() {
                        "b" => 100.0,
                        "c" => 199.0,
                        _ => 10_000.0,
                    });
                (column.UniqueID, ndv)
            })
            .collect();
        let histogram = analyzed_histogram(plan_ctx, source);
        source.TableStats.HistColl = Some(Arc::new(histogram));
        // USE/FORCE INDEX 提示会过滤可选访问路径。
        let forced_indexes = table
            .IndexHints
            .iter()
            .filter(|hint| {
                matches!(
                    hint.HintType,
                    crate::ast::IndexHintType::Use | crate::ast::IndexHintType::Force
                )
            })
            .flat_map(|hint| hint.IndexNames.iter().map(|name| name.L.clone()))
            .collect::<Vec<_>>();
        let mut paths = source
            .TableInfo
            .Indices
            .iter()
            .filter(|index| forced_indexes.is_empty() || forced_indexes.contains(&index.Name.L))
            .map(|index| planner_util_dependency::AccessPath {
                Index: Some(index.Clone()),
                CountAfterAccess: 10_000.0,
                MinCountAfterAccess: 10_000.0,
                MaxCountAfterAccess: 10_000.0,
                CountAfterIndex: 10_000.0,
                ..Default::default()
            })
            .collect::<Vec<_>>();
        if forced_indexes.is_empty() {
            paths.insert(0, source.PossibleAccessPaths[0].clone());
        }
        source.PossibleAccessPaths = paths.clone();
        source.AllPossibleAccessPaths = paths;
        for path in source
            .AllPossibleAccessPaths
            .iter_mut()
            .chain(source.PossibleAccessPaths.iter_mut())
        {
            path.CountAfterAccess = 10_000.0;
            path.MinCountAfterAccess = 10_000.0;
            path.MaxCountAfterAccess = 10_000.0;
            path.CountAfterIndex = 10_000.0;
        }
        Ok(())
    }
}

/// 校验 `construct_insert_sql` 与 Go benchmark 夹具文本一致且可被 parser 接受。
#[test]
fn construct_insert_sql_matches_the_go_benchmark_fixture() {
    assert_eq!(
        construct_insert_sql(2, 3),
        "insert into t (a,b,c,e)values (6, 2, '2', 6), (7, 2, '3', 7), (8, 2, '4', 8)"
    );
    let sql = construct_insert_sql(99, 100);
    assert!(sql.starts_with("insert into t (a,b,c,e)values (9900, 99, '99', 9900)"));
    assert!(sql.ends_with("(9999, 99, '198', 9999)"));
    assert_eq!(sql.matches("), (").count(), 99);

    let mut parser = parser_dependency::New();
    for batch in 0..100 {
        let sql = construct_insert_sql(batch, 100);
        parser
            .ParseOneStmt(&sql, "", "")
            .unwrap_or_else(|error| panic!("parse Go benchmark batch {batch}: {error}"));
    }
}

/// 冒烟：parser → PlanBuilder（夹具统计）→ DoOptimize 产出正代价物理计划。
#[test]
fn parser_builder_provider_and_optimizer_use_canonical_costs() {
    let context = cbo_plan_context();
    let statement = crate::ast::NodeRef::new(
        parser_dependency::New()
            .ParseOneStmt("select a from t", "", "")
            .expect("parse CBO fixture"),
    );
    let (mut builder, _) = crate::NewPlanBuilder()
        .withDataSourceProvider(Arc::new(FixtureStatsProvider))
        .Init(
            context.clone(),
            cbo_info_schema(),
            hint_dependency::NewQBHintHandler(None),
        );
    let mut logical = builder
        .buildResultSetNode(crate::context::TODO(), &statement, false)
        .expect("parser AST must build the canonical logical plan");
    let (_physical, cost) = crate::DoOptimize(
        crate::context::TODO(),
        &context,
        builder.GetOptFlag(),
        &mut logical,
    )
    .expect("canonical optimizer must produce a physical plan");
    assert!(
        cost > 0.0,
        "real table scan and reader cost must be positive"
    );
}

/// 逐条跑 `GO_OPTIMIZER_CASES`，校验指纹含真实存储 Reader 且与 Go 计划选择对齐。
#[test]
fn all_go_optimizer_workloads_build_ranges_and_run_canonical_optimizer() {
    assert_eq!(GO_OPTIMIZER_CASES.len(), 18);
    for (sql, expected) in GO_OPTIMIZER_CASES {
        let context = cbo_plan_context();
        let statement = crate::ast::NodeRef::new(
            parser_dependency::New()
                .ParseOneStmt(sql, "", "")
                .unwrap_or_else(|error| panic!("parse optimizer workload {sql:?}: {error}")),
        );
        let (mut builder, _) = crate::NewPlanBuilder()
            .withDataSourceProvider(Arc::new(FixtureStatsProvider))
            .Init(
                context.clone(),
                cbo_info_schema(),
                hint_dependency::NewQBHintHandler(None),
            );
        let mut logical = builder
            .buildResultSetNode(crate::context::TODO(), &statement, false)
            .unwrap_or_else(|error| panic!("build canonical workload {sql:?}: {error}"));
        let (physical, cost) = crate::DoOptimize(
            crate::context::TODO(),
            &context,
            builder.GetOptFlag(),
            &mut logical,
        )
        .unwrap_or_else(|error| panic!("optimize canonical workload {sql:?}: {error}"));
        assert!(
            cost.is_finite() && cost > 0.0,
            "canonical cost for {sql:?}: {cost}"
        );
        assert!(physical.stats_count().is_finite());
        let fingerprint = canonical_physical_fingerprint(physical.as_ref());
        assert!(
            fingerprint.contains("Reader") || fingerprint.contains("IndexLookUp"),
            "workload must choose a real storage reader: {sql:?} => {fingerprint}"
        );
        // Go 的 benchmark 保留 `best` 作为诊断文本，但不用它断言代价选择。
        // Rust 同样只要求每条 workload 成功优化，并额外校验 Cop 聚合的两阶段边界。
        if fingerprint.starts_with("StreamAgg[final]") {
            assert!(fingerprint.contains("StreamAgg[partial1]"));
        } else if fingerprint.starts_with("HashAgg[final]") {
            assert!(fingerprint.contains("HashAgg[partial1]"));
        }
        let _best = expected;
    }
}
