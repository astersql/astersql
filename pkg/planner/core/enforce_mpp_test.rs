// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Enforce MPP / TiFlash 下推相关集成测试。
//
// MPP（Massively Parallel Processing）指在 TiFlash 等节点上并行执行查询片段。
// 本文件覆盖：行宽对 MPP 代价的影响、标量函数/窗口/聚合下推到 TiFlash reader，
// 以及虚拟生成列不得下推等约束。

use base_dependency::PhysicalPlan as _;
use logicalop_dependency::LogicalPlan as _;
use std::sync::Arc;

/// 为 DataSource 注入固定行数/NDV 统计，便于稳定比较 MPP 代价。
struct MppStatsProvider;

impl crate::DataSourceProvider for MppStatsProvider {
    fn Populate(
        &self,
        _ctx: &dyn crate::context::Context,
        _plan_ctx: &base_dependency::ContextRef,
        _info_schema: &dyn infoschema_dependency::infoschema::InfoSchema,
        _table: &crate::ast::TableName,
        source: &mut logicalop_dependency::DataSource,
    ) -> Result<(), expression_dependency::Error> {
        source.TableStats.RowCount = 1_000.0;
        source.TableStats.ColNDVs = source
            .Schema()
            .Columns
            .iter()
            .map(|column| (column.UniqueID, 1_000.0))
            .collect();
        for path in source
            .AllPossibleAccessPaths
            .iter_mut()
            .chain(source.PossibleAccessPaths.iter_mut())
        {
            path.CountAfterAccess = 1_000.0;
            path.MinCountAfterAccess = 1_000.0;
            path.MaxCountAfterAccess = 1_000.0;
            path.CountAfterIndex = 1_000.0;
        }
        Ok(())
    }
}

/// 构造带 TiFlash 副本的 mock 表 `t` 及其列类型（含 duration、IP、整型等）。
fn mpp_info_schema() -> Arc<dyn infoschema_dependency::infoschema::InfoSchema> {
    let column = |id: i64, name: &str, offset: isize, width: isize| {
        let mut field_type =
            *expression_dependency::types::NewFieldType(expression_dependency::mysql::TypeVarchar);
        field_type.SetFlen(width);
        field_type.SetCharset("utf8mb4".to_owned());
        field_type.SetCollate("utf8mb4_bin".to_owned());
        expression_dependency::model::ColumnInfo {
            ID: id,
            Name: crate::ast::NewCIStr(name),
            Offset: offset,
            State: expression_dependency::model::StatePublic,
            FieldType: field_type,
            ..Default::default()
        }
    };
    let model = Arc::new(expression_dependency::model::TableInfo {
        ID: 301,
        Name: crate::ast::NewCIStr("t"),
        Columns: vec![
            column(1, "a", 0, 10),
            column(2, "b", 1, 20),
            column(3, "c", 2, 256),
            {
                let mut column = column(4, "d", 3, 16);
                column.FieldType = *expression_dependency::types::NewFieldType(
                    expression_dependency::mysql::TypeDuration,
                );
                column
            },
            column(5, "v4", 4, 100),
            column(6, "v6", 5, 100),
            {
                let mut column = column(7, "p", 6, 11);
                column.FieldType = *expression_dependency::types::NewFieldType(
                    expression_dependency::mysql::TypeLonglong,
                );
                column
            },
            {
                let mut column = column(8, "o", 7, 11);
                column.FieldType = *expression_dependency::types::NewFieldType(
                    expression_dependency::mysql::TypeLonglong,
                );
                column
            },
            {
                let mut column = column(9, "v", 8, 11);
                column.FieldType = *expression_dependency::types::NewFieldType(
                    expression_dependency::mysql::TypeLonglong,
                );
                column
            },
        ],
        TiFlashReplica: Some(expression_dependency::model::TiFlashReplicaInfo {
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

/// 解析 SQL、绑定 mock 统计与 InfoSchema，构建逻辑计划并优化为物理计划。
fn build_and_optimize_tiflash(
    context: &base_dependency::ContextRef,
    sql: &str,
) -> Box<dyn base_dependency::PhysicalPlan> {
    let statement = crate::ast::NodeRef::new(
        parser_dependency::New()
            .ParseOneStmt(sql, "", "")
            .unwrap_or_else(|error| panic!("parse TiFlash query {sql:?}: {error}")),
    );
    let (mut builder, _) = crate::NewPlanBuilder()
        .withDataSourceProvider(Arc::new(MppStatsProvider))
        .Init(
            context.clone(),
            mpp_info_schema(),
            hint_dependency::NewQBHintHandler(None),
        );
    let mut logical = builder
        .buildResultSetNode(crate::context::TODO(), &statement, false)
        .unwrap_or_else(|error| panic!("build TiFlash query {sql:?}: {error}"));
    crate::DoOptimize(
        crate::context::TODO(),
        context,
        builder.GetOptFlag(),
        &mut logical,
    )
    .unwrap_or_else(|error| panic!("optimize TiFlash query {sql:?}: {error}"))
    .0
}

/// 递归检查物理计划中是否存在 TiFlash TableReader，且其子树含类型 T 的算子。
fn reader_contains<T: 'static>(plan: &dyn base_dependency::PhysicalPlan) -> bool {
    if let Some(reader) = plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalTableReader>()
    {
        return reader.StoreType == kv_dependency::StoreType::TiFlash
            && reader
                .children()
                .into_iter()
                .any(|child| contains_operator::<T>(child));
    }
    plan.children()
        .into_iter()
        .any(|child| reader_contains::<T>(child))
}

/// 递归判断计划树是否包含类型为 T 的物理算子。
fn contains_operator<T: 'static>(plan: &dyn base_dependency::PhysicalPlan) -> bool {
    plan.as_any().is::<T>()
        || plan
            .children()
            .into_iter()
            .any(|child| contains_operator::<T>(child))
}

/// 检查计划树是否含真实 TiFlash MPP TableReader 与 IsMPPOrBatchCop 的 TableScan。
/// 返回 (found_reader, found_scan)。
fn assert_real_mpp_reader(plan: &dyn base_dependency::PhysicalPlan) -> (bool, bool) {
    let reader = plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalTableReader>()
        .is_some_and(|reader| {
            reader.StoreType == kv_dependency::StoreType::TiFlash
                && matches!(reader.ReadReqType, physicalop_dependency::ReadReqType::MPP)
        });
    let scan = plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalTableScan>()
        .is_some_and(|scan| {
            scan.StoreType == kv_dependency::StoreType::TiFlash && scan.IsMPPOrBatchCop
        });
    plan.children()
        .into_iter()
        .fold((reader, scan), |found, child| {
            let nested = assert_real_mpp_reader(child);
            (found.0 || nested.0, found.1 || nested.1)
        })
}

/// 对指定列做 TiFlash 投影，返回代价模型 v1 与 v2 的代价。
fn optimize_tiflash_projection(
    context: &base_dependency::ContextRef,
    info_schema: Arc<dyn infoschema_dependency::infoschema::InfoSchema>,
    column: &str,
) -> (f64, f64) {
    let sql = format!("select /*+ read_from_storage(tiflash[t]) */ {column} from t");
    let statement = crate::ast::NodeRef::new(
        parser_dependency::New()
            .ParseOneStmt(&sql, "", "")
            .unwrap_or_else(|error| panic!("parse Go MPP query {sql:?}: {error}")),
    );
    let (mut builder, _) = crate::NewPlanBuilder()
        .withDataSourceProvider(Arc::new(MppColumnSizeStatsProvider))
        .Init(
            context.clone(),
            info_schema,
            hint_dependency::NewQBHintHandler(None),
        );
    let mut logical = builder
        .buildResultSetNode(crate::context::TODO(), &statement, false)
        .unwrap_or_else(|error| panic!("build Go MPP query {sql:?}: {error}"));
    let (physical, v1) = crate::DoOptimize(
        crate::context::TODO(),
        context,
        builder.GetOptFlag(),
        &mut logical,
    )
    .unwrap_or_else(|error| panic!("optimize Go MPP query {sql:?}: {error}"));
    let (reader, scan) = assert_real_mpp_reader(physical.as_ref());
    assert!(
        reader && scan,
        "query must use a real TiFlash MPP reader and scan: {sql}"
    );

    let mut v2_plan = physical
        .clone_physical(context.clone())
        .expect("clone real MPP physical plan for cost v2");
    let v2 = v2_plan
        .get_plan_cost_ver2(
            property_dependency::RootTaskType,
            &costusage_dependency::new_default_plan_cost_option(),
            &[],
        )
        .expect("cost real MPP physical plan with v2")
        .get_cost();
    (v1, v2)
}

/// Cost v1 uses observed column sizes, not VARCHAR's declared maximum length.
/// Populate real size statistics so both models measure the wider input rows.
struct MppColumnSizeStatsProvider;

impl crate::DataSourceProvider for MppColumnSizeStatsProvider {
    fn Populate(
        &self,
        ctx: &dyn crate::context::Context,
        plan_ctx: &base_dependency::ContextRef,
        info_schema: &dyn infoschema_dependency::infoschema::InfoSchema,
        table: &crate::ast::TableName,
        source: &mut logicalop_dependency::DataSource,
    ) -> Result<(), expression_dependency::Error> {
        crate::DataSourceProvider::Populate(
            &MppStatsProvider,
            ctx,
            plan_ctx,
            info_schema,
            table,
            source,
        )?;
        let mut histogram = *statistics_dependency::NewHistColl(
            source.PhysicalTableID,
            1_000,
            0,
            source.Schema().Len(),
            0,
        );
        histogram.Pseudo = false;
        histogram.StatsVer = statistics_dependency::Version2;
        for column in &source.Schema().Columns {
            let field_type = column.RetType.as_ref().expect("MPP column type");
            histogram.SetCol(
                column.UniqueID,
                Box::new(statistics_dependency::Column {
                    CMSketch: None,
                    TopN: None,
                    FMSketch: None,
                    Info: None,
                    Histogram: statistics_dependency::NewHistogram(
                        column.UniqueID,
                        1_000,
                        0,
                        0,
                        field_type,
                        0,
                        field_type.GetFlen().max(8) as i64 * 1_000,
                    ),
                    StatsLoadedStatus: statistics_dependency::NewStatsFullLoadStatus(),
                    PhysicalID: source.PhysicalTableID,
                    StatsVer: statistics_dependency::Version2 as i64,
                    IsHandle: false,
                }),
            );
        }
        source.TableStats.HistColl = Some(Arc::new(histogram));
        Ok(())
    }
}

#[test]
/// 列宽增大时应使真实 MPP 计划的 v1/v2 代价单调上升。
fn row_size_increases_real_builder_mpp_cost() {
    let context = crate::cbo_test::cbo_plan_context();
    let info_schema = mpp_info_schema();
    let costs = [
        optimize_tiflash_projection(&context, info_schema.clone(), "a"),
        optimize_tiflash_projection(&context, info_schema.clone(), "b"),
        optimize_tiflash_projection(&context, info_schema, "c"),
    ];
    assert!(
        costs[0].0 < costs[1].0 && costs[1].0 < costs[2].0,
        "row size must increase real MPP cost v1: {costs:?}"
    );
    assert!(
        costs[0].1 < costs[1].1 && costs[1].1 < costs[2].1,
        "row size must increase real MPP cost v2: {costs:?}"
    );
}

#[test]
/// 若干标量函数应在 TiFlash reader 内以 Projection 执行，并带非零 TiPB 签名。
fn tiflash_scalar_sql_builds_signed_projection_inside_mpp_reader() {
    let context = crate::cbo_test::cbo_plan_context();
    let cases = [
        ("time_to_sec(d)", crate::ast::functions::TimeToSec),
        ("is_ipv4(v4)", crate::ast::functions::IsIPv4),
        ("is_ipv6(v6)", crate::ast::functions::IsIPv6),
        (
            "regexp_instr(a, b, 1, 1, 0, c)",
            crate::ast::functions::RegexpInStr,
        ),
        (
            "regexp_substr(a, b, 1, 1, c)",
            crate::ast::functions::RegexpSubstr,
        ),
        (
            "regexp_replace(a, b, c, 1, 1, '')",
            crate::ast::functions::RegexpReplace,
        ),
    ];
    for (expression, expected_name) in cases {
        let sql = format!("select /*+ read_from_storage(tiflash[t]) */ {expression} from t");
        let physical = build_and_optimize_tiflash(&context, &sql);
        assert!(
            reader_contains::<physicalop_dependency::PhysicalProjection>(physical.as_ref()),
            "projection must execute inside the TiFlash reader: {sql}"
        );
        let mut signatures = Vec::new();
        collect_scalar_signatures(physical.as_ref(), expected_name, &mut signatures);
        assert!(
            signatures.iter().any(|signature| *signature > 0),
            "{expected_name} must carry a nonzero TiPB signature: {sql}"
        );
    }
}

/// 收集计划树中指定函数名的 ScalarFunction 的 TiPB PbCode。
fn collect_scalar_signatures(
    plan: &dyn base_dependency::PhysicalPlan,
    name: &str,
    signatures: &mut Vec<i32>,
) {
    if let Some(projection) = plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalProjection>()
    {
        for expression in &projection.Exprs {
            if let Some(function) = expression
                .as_any()
                .downcast_ref::<expression_dependency::ScalarFunction>()
                && function.FuncName.L == name
            {
                signatures.push(function.Function.PbCode());
            }
        }
    }
    for child in plan.children() {
        collect_scalar_signatures(child, name, signatures);
    }
}

#[test]
/// RANGE 窗口应下推到 TiFlash reader，且 StoreTp 为 TiFlash。
fn window_range_sql_builds_window_inside_tiflash_reader() {
    let context = crate::cbo_test::cbo_plan_context();
    let sql = "select /*+ read_from_storage(tiflash[t]) */ first_value(v) over (partition by p order by o range between 3 preceding and 0 following) from t";
    let physical = build_and_optimize_tiflash(&context, sql);
    assert!(
        reader_contains::<physicalop_dependency::PhysicalWindow>(physical.as_ref()),
        "window must execute inside the TiFlash reader"
    );
    let window = find_operator::<physicalop_dependency::PhysicalWindow>(physical.as_ref())
        .expect("physical window");
    assert_eq!(window.StoreTp, kv_dependency::StoreType::TiFlash);
    assert_eq!(window.WindowFuncDescs.len(), 1);
    assert_eq!(
        window.WindowFuncDescs[0].Name,
        crate::ast::functions::WindowFuncFirstValue
    );
}

#[test]
/// 无 GROUP BY 的 approx_count_distinct 应使用两阶段 StreamAgg（根 + reader 内）。
fn approx_count_distinct_without_group_by_builds_two_phase_stream_agg() {
    let context = crate::cbo_test::cbo_plan_context();
    let sql = "select /*+ read_from_storage(tiflash[t]) */ approx_count_distinct(a) from t";
    let physical = build_and_optimize_tiflash(&context, sql);
    assert!(
        physical
            .as_any()
            .is::<physicalop_dependency::PhysicalStreamAgg>(),
        "final aggregation must be a root PhysicalStreamAgg"
    );
    assert!(
        reader_contains::<physicalop_dependency::PhysicalStreamAgg>(physical.as_ref()),
        "partial PhysicalStreamAgg must execute inside the TiFlash reader"
    );
}

#[test]
/// TiFlash HashAgg 的会话预聚合模式应与 Go 默认值一致。
fn tiflash_pre_aggregation_mode_defaults_to_go_session_value() {
    let context = crate::cbo_test::cbo_plan_context();
    assert_eq!(
        context.GetSessionVars().TiFlashPreAggMode,
        vardef_dependency::DefTiFlashPreAggMode,
        "TiFlash hash aggregation must inherit the Go session default"
    );
}

#[test]
/// 虚拟生成列上的聚合/窗口不得下推到 TiFlash。
fn virtual_generated_columns_are_not_pushed_into_tiflash_operators() {
    let context = crate::cbo_test::cbo_plan_context();
    let field_type =
        *expression_dependency::types::NewFieldType(expression_dependency::mysql::TypeLonglong);
    let mut virtual_column = expression_dependency::Column::new(field_type.Clone(), 10, 10, 0);
    virtual_column.VirtualExpr = Some(Box::new(expression_dependency::Constant::with_type(
        expression_dependency::types::NewIntDatum(1),
        field_type,
    )));
    let aggregate = aggregation_dependency::NewAggFuncDesc(
        context.GetExprCtx(),
        crate::ast::functions::AggFuncSum,
        vec![Box::new(virtual_column.Clone())],
        false,
    )
    .expect("aggregate descriptor");
    assert!(
        !physicalop_dependency::CheckAggCanPushCop(
            context.as_ref(),
            &[aggregate],
            &[],
            kv_dependency::StoreType::TiFlash,
        ),
        "aggregation over a virtual generated column must not push to TiFlash"
    );

    let mut window = logicalop_dependency::LogicalWindow::default().Init(context.clone(), 0);
    window.WindowFuncDescs = vec![logicalop_dependency::WindowFuncDesc {
        Name: crate::ast::functions::WindowFuncFirstValue.to_owned(),
        Args: vec![Box::new(virtual_column)],
    }];
    window
        .LogicalSchemaProducer
        .SetSchema(expression_dependency::NewSchema(Vec::new()));
    let plans = physicalop_dependency::ExhaustPhysicalPlans4LogicalWindow(
        &window,
        &property_dependency::PhysicalProperty::default(),
    );
    assert!(
        plans.iter().all(|plan| {
            plan.as_any()
                .downcast_ref::<physicalop_dependency::PhysicalWindow>()
                .is_none_or(|window| window.StoreTp != kv_dependency::StoreType::TiFlash)
        }),
        "window over a virtual generated column must not push to TiFlash"
    );
}

/// 深度优先查找第一个类型为 T 的物理算子。
fn find_operator<T: 'static>(plan: &dyn base_dependency::PhysicalPlan) -> Option<&T> {
    if let Some(found) = plan.as_any().downcast_ref::<T>() {
        return Some(found);
    }
    plan.children().into_iter().find_map(find_operator::<T>)
}
