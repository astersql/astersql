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

// 规划器集成测试：真实优化路径、表达式下推与 DML/回归场景。
//
// 覆盖隔离读引擎过滤、聚合/标量函数下推到 TiFlash（列存引擎）、
// Point Get（主键/唯一键点查）、计划缓存 range fallback、以及历史 issue 复现。
// 对应 Go `pkg/planner/core/integration_test.go`。

// 本文件对等迁移 pkg/planner/core/integration_test.go，覆盖真实 planner、表达式下推、
// fast point-get、typed DML 和回归场景。
//

#![allow(dead_code, non_snake_case, non_camel_case_types, unused_variables)]

use base_dependency as base;
use base_dependency::PhysicalPlan as _;
use base_dependency::Plan as _;
use expression_dependency as expression;
use logicalop_dependency as logicalop;
use logicalop_dependency::LogicalPlan as _;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

/// 集成测试用的 PlanContext：分配计划 ID、会话变量与 Ranger/表达式上下文。
struct IntegrationPlanContext {
    /// 单调递增的逻辑/物理计划节点 ID 分配器。
    plan_id: AtomicI32,
    /// 会话级系统变量与语句上下文。
    session_vars: variable_dependency::session::SessionVars,
    /// 表达式求值/构建上下文。
    expr_ctx: exprstatic_dependency::ExprContext,
    /// 范围推导（Ranger）上下文，供索引范围裁剪。
    ranger_ctx: base::RangerContext<'static>,
    /// 内置函数使用计数，供 EXPLAIN/诊断断言。
    builtin_usage: base::BuiltinFunctionUsageCounter,
}

impl base::PlanContext for IntegrationPlanContext {
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
        panic!("integration planner tests do not build protobuf executors")
    }

    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.builtin_usage.Inc(name)
    }
}

/// 构造默认集成 PlanContext（无绑定参数、无额外系统变量）。
fn integration_plan_context(
    engines: &[kv_dependency::StoreType],
    engine_value: &str,
    strict_write: bool,
    enforce_mpp: bool,
) -> base::ContextRef {
    integration_plan_context_with_params(
        engines,
        engine_value,
        strict_write,
        enforce_mpp,
        Vec::new(),
        None,
    )
}

/// 带计划缓存参数列表与可选 RangeMaxSize 的 PlanContext。
fn integration_plan_context_with_params(
    engines: &[kv_dependency::StoreType],
    engine_value: &str,
    strict_write: bool,
    enforce_mpp: bool,
    params: Vec<expression::types::Datum>,
    range_max_size: Option<i64>,
) -> base::ContextRef {
    integration_plan_context_with_params_and_vars(
        engines,
        engine_value,
        strict_write,
        enforce_mpp,
        params,
        range_max_size,
        &[],
    )
}

/// 完整构造：隔离读引擎、MPP 强制、参数、RangeMaxSize 与额外系统变量。
fn integration_plan_context_with_params_and_vars(
    engines: &[kv_dependency::StoreType],
    engine_value: &str,
    strict_write: bool,
    enforce_mpp: bool,
    params: Vec<expression::types::Datum>,
    range_max_size: Option<i64>,
    system_vars: &[(&str, &str)],
) -> base::ContextRef {
    logicalop::InstallPredicateSimplificationPassthrough();
    let mut session_vars = variable_dependency::session::SessionVars::default();
    session_vars.SetCurrentDB("test");
    session_vars.StmtCtx.TiFlashEngineRemovedDueToStrictSQLMode = strict_write;
    session_vars.AllowMPPExecution = enforce_mpp;
    session_vars.EnforceMPPExecution = enforce_mpp;
    if let Some(range_max_size) = range_max_size {
        session_vars.RangeMaxSize = range_max_size;
    }
    session_vars
        .SetSystemVar(vardef_dependency::TiDBIsolationReadEngines, engine_value)
        .expect("set canonical isolation-read system variable");
    // `StrictTiFlashGuard` keeps the configured sysvar text for diagnostics while
    // removing TiFlash from the engines that are actually available to planning.
    // 诊断仍保留 sysvar 原文，但实际可供规划的引擎集合已去掉 TiFlash。
    session_vars.IsolationReadEngines = engines.iter().copied().collect::<HashSet<_>>();
    for (name, value) in system_vars {
        session_vars
            .SetSystemVar(name, value)
            .unwrap_or_else(|error| {
                panic!("set integration system variable {name}={value}: {error}")
            });
    }
    let ranger_params = params.clone();
    let expression_context =
        exprstatic_dependency::NewExprContext(vec![exprstatic_dependency::WithEvalCtx(Arc::new(
            exprstatic_dependency::NewEvalContext(vec![exprstatic_dependency::WithParamList(
                params,
            )]),
        ))]);
    let ranger_expression: Arc<dyn expression::exprctx::BuildContext> =
        Arc::new(exprstatic_dependency::NewExprContext(vec![
            exprstatic_dependency::WithEvalCtx(Arc::new(exprstatic_dependency::NewEvalContext(
                vec![exprstatic_dependency::WithParamList(ranger_params)],
            ))),
        ]));
    Arc::new(IntegrationPlanContext {
        plan_id: AtomicI32::new(0),
        session_vars,
        expr_ctx: expression_context,
        ranger_ctx: base::RangerContext {
            TypeCtx: expression::types::DefaultStmtNoWarningContext.clone(),
            ErrCtx: expression::errctx::StrictNoWarningContext.clone(),
            ExprCtx: ranger_expression,
            RangeFallbackHandler: None,
            PlanCacheTracker: None,
            OptimizerFixControl: Default::default(),
            UseCache: false,
            RegardNULLAsPoint: true,
            OptPrefixIndexSingleScan: false,
        },
        builtin_usage: base::BuiltinFunctionUsageCounter::default(),
    })
}

/// 构造单表 Mock InfoSchema：可配置 TiFlash 副本、分区键、JSON 列与缓存表。
fn integration_info_schema(
    tiflash: bool,
    datetime_partition_key: bool,
    table_name: &str,
    json_column_a: bool,
    cached_table: bool,
) -> Arc<dyn infoschema_dependency::infoschema::InfoSchema> {
    let mut column_a_type = *expression::types::NewFieldType(if json_column_a {
        expression::mysql::TypeJSON
    } else if datetime_partition_key {
        expression::mysql::TypeDatetime
    } else {
        expression::mysql::TypeLonglong
    });
    if !datetime_partition_key && !json_column_a {
        column_a_type.AddFlag(expression::mysql::PriKeyFlag | expression::mysql::NotNullFlag);
    }
    let mut varchar_type = *expression::types::NewFieldType(expression::mysql::TypeVarchar);
    varchar_type.SetFlen(20);
    let int_type = *expression::types::NewFieldType(expression::mysql::TypeLonglong);
    let model = Arc::new(expression::model::TableInfo {
        ID: 1001,
        Name: crate::ast::NewCIStr(table_name),
        Columns: vec![
            expression::model::ColumnInfo {
                ID: 1,
                Name: crate::ast::NewCIStr("a"),
                Offset: 0,
                State: expression::model::StatePublic,
                FieldType: column_a_type,
                ..Default::default()
            },
            expression::model::ColumnInfo {
                ID: 2,
                Name: crate::ast::NewCIStr("b"),
                Offset: 1,
                State: expression::model::StatePublic,
                FieldType: varchar_type.clone(),
                ..Default::default()
            },
            expression::model::ColumnInfo {
                ID: 3,
                Name: crate::ast::NewCIStr("p"),
                Offset: 2,
                State: expression::model::StatePublic,
                FieldType: int_type.clone(),
                ..Default::default()
            },
            expression::model::ColumnInfo {
                ID: 4,
                Name: crate::ast::NewCIStr("o"),
                Offset: 3,
                State: expression::model::StatePublic,
                FieldType: int_type.clone(),
                ..Default::default()
            },
            expression::model::ColumnInfo {
                ID: 5,
                Name: crate::ast::NewCIStr("v"),
                Offset: 4,
                State: expression::model::StatePublic,
                FieldType: int_type.clone(),
                ..Default::default()
            },
            expression::model::ColumnInfo {
                ID: 6,
                Name: crate::ast::NewCIStr("c1"),
                Offset: 5,
                State: expression::model::StatePublic,
                FieldType: int_type,
                ..Default::default()
            },
            expression::model::ColumnInfo {
                ID: 7,
                Name: crate::ast::NewCIStr("process_code"),
                Offset: 6,
                State: expression::model::StatePublic,
                FieldType: varchar_type.clone(),
                ..Default::default()
            },
        ],
        PKIsHandle: !datetime_partition_key && !json_column_a,
        TiFlashReplica: tiflash.then_some(expression::model::TiFlashReplicaInfo {
            Count: 1,
            Available: true,
            ..Default::default()
        }),
        TableCacheStatusType: if cached_table {
            expression::model::TableCacheStatusEnable
        } else {
            expression::model::TableCacheStatusDisable
        },
        Partition: datetime_partition_key.then_some(expression::model::PartitionInfo {
            Type: expression::model::ast::PartitionTypeRange,
            Expr: "weekday(`a`)".to_owned(),
            Enable: true,
            Definitions: vec![
                expression::model::PartitionDefinition {
                    ID: 1101,
                    Name: crate::ast::NewCIStr("p0"),
                    LessThan: vec!["10".to_owned()],
                    ..Default::default()
                },
                expression::model::PartitionDefinition {
                    ID: 1102,
                    Name: crate::ast::NewCIStr("p1"),
                    LessThan: vec!["100".to_owned()],
                    ..Default::default()
                },
            ],
            Num: 2,
            ..Default::default()
        }),
        ..Default::default()
    });
    infoschema_dependency::infoschema::MockInfoSchema(vec![
        infoschema_dependency::infoschema::TableInfo {
            id: model.ID,
            name: infoschema_dependency::infoschema::CiString::new(table_name),
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

/// 向 DataSource 注入行数统计，并可按隔离读过滤访问路径。
struct IntegrationStatsProvider {
    /// 表级估计行数。
    row_count: f64,
    /// true 表示伪统计（StatsVersion=0）。
    pseudo: bool,
    /// 是否应用 IsolationRead 引擎过滤。
    apply_isolation_filter: bool,
}

impl crate::DataSourceProvider for IntegrationStatsProvider {
    fn Populate(
        &self,
        _ctx: &dyn crate::context::Context,
        plan_ctx: &base::ContextRef,
        _info_schema: &dyn infoschema_dependency::infoschema::InfoSchema,
        _table: &crate::ast::TableName,
        source: &mut logicalop::DataSource,
    ) -> Result<(), expression::Error> {
        source.TableStats.RowCount = self.row_count;
        source.TableStats.StatsVersion = if self.pseudo { 0 } else { 1 };
        let mut histogram = *statistics_dependency::NewHistColl(
            source.PhysicalTableID,
            self.row_count.round() as i64,
            0,
            source.TableInfo.Columns.len(),
            source.TableInfo.Indices.len(),
        );
        histogram.Pseudo = self.pseudo;
        histogram.StatsVer = if self.pseudo {
            0
        } else {
            statistics_dependency::Version2
        };
        source.TableStats.HistColl = Some(Arc::new(histogram));
        source.TableStats.ColNDVs = source
            .Schema()
            .Columns
            .iter()
            .map(|column| (column.UniqueID, self.row_count))
            .collect();
        for path in source
            .AllPossibleAccessPaths
            .iter_mut()
            .chain(source.PossibleAccessPaths.iter_mut())
        {
            path.CountAfterAccess = self.row_count;
            path.MinCountAfterAccess = self.row_count;
            path.MaxCountAfterAccess = self.row_count;
            path.CountAfterIndex = self.row_count;
        }
        if self.apply_isolation_filter {
            // 有 TiFlash 副本但路径缺失时补一条，再按隔离读过滤。
            if source
                .TableInfo
                .TiFlashReplica
                .as_ref()
                .is_some_and(|replica| replica.Available)
                && !source
                    .PossibleAccessPaths
                    .iter()
                    .any(|path| path.StoreType == kv_dependency::StoreType::TiFlash)
            {
                let mut tiflash_path = source.PossibleAccessPaths[0].clone();
                tiflash_path.StoreType = kv_dependency::StoreType::TiFlash;
                source.PossibleAccessPaths.push(tiflash_path);
            }
            source.PossibleAccessPaths = planner_util_dependency::FilterPathByIsolationRead(
                plan_ctx.as_ref(),
                std::mem::take(&mut source.PossibleAccessPaths),
                source.TableInfo.Name.clone(),
                source.DBName.clone(),
            )?;
        }
        Ok(())
    }
}

/// 默认表名 `t` 下优化 SQL，返回物理计划。
fn optimize_integration_query(
    sql: &str,
    context: &base::ContextRef,
    tiflash: bool,
    datetime_partition_key: bool,
) -> Box<dyn base::PhysicalPlan> {
    optimize_integration_query_for_table(
        sql,
        context,
        tiflash,
        datetime_partition_key,
        "t",
        false,
        false,
    )
}

/// 可指定表名、JSON 列与缓存表标志的优化入口。
fn optimize_integration_query_for_table(
    sql: &str,
    context: &base::ContextRef,
    tiflash: bool,
    datetime_partition_key: bool,
    table_name: &str,
    json_column_a: bool,
    cached_table: bool,
) -> Box<dyn base::PhysicalPlan> {
    optimize_integration_query_with_schema(
        sql,
        context,
        integration_info_schema(
            tiflash,
            datetime_partition_key,
            table_name,
            json_column_a,
            cached_table,
        ),
    )
}

/// 使用给定 InfoSchema 优化，默认伪统计 1000 行。
fn optimize_integration_query_with_schema(
    sql: &str,
    context: &base::ContextRef,
    info_schema: Arc<dyn infoschema_dependency::infoschema::InfoSchema>,
) -> Box<dyn base::PhysicalPlan> {
    optimize_integration_query_with_schema_and_stats(sql, context, info_schema, 1_000.0, true)
}

/// 指定行数与是否伪统计的优化；失败则 panic。
fn optimize_integration_query_with_schema_and_stats(
    sql: &str,
    context: &base::ContextRef,
    info_schema: Arc<dyn infoschema_dependency::infoschema::InfoSchema>,
    row_count: f64,
    pseudo: bool,
) -> Box<dyn base::PhysicalPlan> {
    try_optimize_integration_query_with_schema_and_stats(
        sql,
        context,
        info_schema,
        row_count,
        pseudo,
    )
    .unwrap_or_else(|error| panic!("optimize integration SQL {sql:?}: {error}"))
}

/// 可返回错误的优化包装，供需要断言失败场景的测试使用。
fn try_optimize_integration_query_with_schema_and_stats(
    sql: &str,
    context: &base::ContextRef,
    info_schema: Arc<dyn infoschema_dependency::infoschema::InfoSchema>,
    row_count: f64,
    pseudo: bool,
) -> Result<Box<dyn base::PhysicalPlan>, String> {
    try_optimize_integration_query_with_schema_and_stats_and_isolation_filter(
        sql,
        context,
        info_schema,
        row_count,
        pseudo,
        false,
    )
}

/// 完整优化路径：可开关隔离读过滤，经 Build + DoOptimize 产出物理计划。
fn try_optimize_integration_query_with_schema_and_stats_and_isolation_filter(
    sql: &str,
    context: &base::ContextRef,
    info_schema: Arc<dyn infoschema_dependency::infoschema::InfoSchema>,
    row_count: f64,
    pseudo: bool,
    apply_isolation_filter: bool,
) -> Result<Box<dyn base::PhysicalPlan>, String> {
    let statement = crate::ast::NodeRef::new(
        parser_dependency::New()
            .ParseOneStmt(sql, "", "")
            .map_err(|error| format!("parse integration SQL: {error}"))?,
    );
    let (mut builder, _) = crate::NewPlanBuilder()
        .withDataSourceProvider(Arc::new(IntegrationStatsProvider {
            row_count,
            pseudo,
            apply_isolation_filter,
        }))
        .Init(
            context.clone(),
            info_schema,
            hint_dependency::NewQBHintHandler(None),
        );
    let crate::BuiltRuntimePlan::Logical(mut logical) = builder
        .BuildNodeRef(crate::context::TODO(), &statement)
        .map_err(|error| format!("build integration SQL: {error}"))?
    else {
        return Err("build integration SQL: expected a physical query plan".to_owned());
    };
    let (physical, cost) = crate::DoOptimize(
        crate::context::TODO(),
        context,
        builder.GetOptFlag(),
        &mut logical,
    )
    .map_err(|error| format!("optimize integration SQL: {error}"))?;
    if !cost.is_finite() || cost <= 0.0 {
        return Err(format!("invalid cost {cost}"));
    }
    Ok(physical)
}

/// 多表 Mock InfoSchema：可共享索引定义与 varchar 列集合。
fn integration_multi_info_schema(
    table_names: &[&str],
    tiflash: bool,
    index: Option<(&str, &[&str])>,
    varchar_columns: &[&str],
) -> Arc<dyn infoschema_dependency::infoschema::InfoSchema> {
    let tables = table_names
        .iter()
        .enumerate()
        .map(|(table_offset, table_name)| {
            let table_id = 2_000 + table_offset as i64;
            // 统一列名池，按表覆盖类型与主键标志。
            let columns = [
                "a",
                "b",
                "c",
                "d",
                "id",
                "bid",
                "cid",
                "user_id",
                "hcode",
                "c0",
                "c1",
                "c2",
                "ref0",
                "ref1",
                "ref2",
                "ref3",
                "is_deleted",
                "deleted_at",
                "object_id",
                "p",
                "o",
                "v",
                "o_datetime",
                "o_time",
            ]
            .iter()
            .enumerate()
            .map(|(column_offset, column_name)| {
                let mut field_type = if *column_name == "o_datetime" {
                    *expression::types::NewFieldType(expression::mysql::TypeDatetime)
                } else if *column_name == "o_time" {
                    *expression::types::NewFieldType(expression::mysql::TypeDuration)
                } else if *table_name == "first_range_d64" && *column_name == "o" {
                    *expression::types::NewFieldType(expression::mysql::TypeDouble)
                } else if matches!(*column_name, "hcode" | "c0" | "c2")
                    || varchar_columns.contains(column_name)
                {
                    *expression::types::NewFieldType(expression::mysql::TypeVarchar)
                } else {
                    *expression::types::NewFieldType(expression::mysql::TypeLonglong)
                };
                if *table_name == "t3" && *column_name == "a" {
                    field_type
                        .AddFlag(expression::mysql::PriKeyFlag | expression::mysql::NotNullFlag);
                }
                expression::model::ColumnInfo {
                    ID: table_id * 100 + column_offset as i64 + 1,
                    Name: crate::ast::NewCIStr(column_name),
                    Offset: column_offset as isize,
                    State: expression::model::StatePublic,
                    FieldType: field_type,
                    ..Default::default()
                }
            })
            .collect::<Vec<_>>();
            let indices = index
                .map(|(index_name, index_columns)| expression::model::IndexInfo {
                    ID: table_id * 10 + 1,
                    Name: crate::ast::NewCIStr(index_name),
                    Table: crate::ast::NewCIStr(*table_name),
                    Columns: index_columns
                        .iter()
                        .map(|index_column| expression::model::IndexColumn {
                            Name: crate::ast::NewCIStr(index_column),
                            Offset: columns
                                .iter()
                                .position(|column| column.Name.L == *index_column)
                                .expect("integration index column must exist")
                                as isize,
                            Length: -1,
                            ..Default::default()
                        })
                        .collect(),
                    State: expression::model::StatePublic,
                    ..Default::default()
                })
                .into_iter()
                .collect();
            let model = Arc::new(expression::model::TableInfo {
                ID: table_id,
                Name: crate::ast::NewCIStr(*table_name),
                Columns: columns,
                Indices: indices,
                TiFlashReplica: tiflash.then_some(expression::model::TiFlashReplicaInfo {
                    Count: 1,
                    Available: true,
                    ..Default::default()
                }),
                PKIsHandle: *table_name == "t3",
                ..Default::default()
            });
            infoschema_dependency::infoschema::TableInfo {
                id: model.ID,
                name: infoschema_dependency::infoschema::CiString::new(*table_name),
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
            }
        })
        .collect();
    infoschema_dependency::infoschema::MockInfoSchema(tables)
}

/// 物理计划树是否包含类型 `T` 的算子。
fn physical_plan_contains<T: 'static>(plan: &dyn base::PhysicalPlan) -> bool {
    plan.as_any().is::<T>()
        || plan
            .children()
            .into_iter()
            .any(|child| physical_plan_contains::<T>(child))
}

/// 深度优先查找第一个 PhysicalIndexScan。
fn find_physical_index_scan(
    plan: &dyn base::PhysicalPlan,
) -> Option<&physicalop_dependency::PhysicalIndexScan> {
    plan.as_any()
        .downcast_ref::<physicalop_dependency::PhysicalIndexScan>()
        .or_else(|| {
            plan.children()
                .into_iter()
                .find_map(find_physical_index_scan)
        })
}

/// 深度优先查找第一个 PhysicalTableScan。
fn find_physical_table_scan(
    plan: &dyn base::PhysicalPlan,
) -> Option<&physicalop_dependency::PhysicalTableScan> {
    if let Some(scan) = plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalTableScan>()
    {
        return Some(scan);
    }
    plan.children()
        .iter()
        .find_map(|child| find_physical_table_scan(*child))
}

/// 深度优先查找第一个 PhysicalWindow（窗口函数算子）。
fn find_physical_window(
    plan: &dyn base::PhysicalPlan,
) -> Option<&physicalop_dependency::PhysicalWindow> {
    if let Some(window) = plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalWindow>()
    {
        return Some(window);
    }
    plan.children()
        .iter()
        .find_map(|child| find_physical_window(*child))
}

/// 深度优先查找第一个 PhysicalIndexJoin（索引嵌套循环连接）。
fn find_physical_index_join(
    plan: &dyn base::PhysicalPlan,
) -> Option<&physicalop_dependency::PhysicalIndexJoin> {
    plan.as_any()
        .downcast_ref::<physicalop_dependency::PhysicalIndexJoin>()
        .or_else(|| {
            plan.children()
                .into_iter()
                .find_map(find_physical_index_join)
        })
}

/// 前序收集各物理算子的粗粒度类型名。
fn physical_plan_types(plan: &dyn base::PhysicalPlan, output: &mut Vec<String>) {
    output.push(plan.tp(&[]));
    for child in plan.children() {
        physical_plan_types(child, output);
    }
}

/// 前序收集精确类型名（区分 StreamAgg/HashAgg 与 Scan 变体）。
fn physical_plan_exact_types(plan: &dyn base::PhysicalPlan, output: &mut Vec<String>) {
    let kind = if plan
        .as_any()
        .is::<physicalop_dependency::PhysicalStreamAgg>()
    {
        "StreamAgg".to_owned()
    } else if plan.as_any().is::<physicalop_dependency::PhysicalHashAgg>() {
        "HashAgg".to_owned()
    } else if let Some(scan) = plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalIndexScan>()
    {
        scan.TP()
    } else if let Some(scan) = plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalTableScan>()
    {
        scan.TP()
    } else {
        plan.tp(&[])
    };
    output.push(kind);
    for child in plan.children() {
        physical_plan_exact_types(child, output);
    }
}

/// 生成含访问条件等详情的诊断字符串列表，便于失败断言。
fn physical_plan_diagnostics(plan: &dyn base::PhysicalPlan, output: &mut Vec<String>) {
    if let Some(scan) = plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalIndexScan>()
    {
        output.push(format!("{} {}", scan.TP(), scan.ExplainInfo()));
    } else if let Some(scan) = plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalTableScan>()
    {
        let eval = base::Plan::s_ctx(scan).GetExprCtx().GetEvalCtx();
        let access_bytes = expression::SortedExplainExpressionList(eval, &scan.AccessCondition);
        let access = String::from_utf8_lossy(&access_bytes);
        output.push(format!(
            "{} {} access:{access}",
            scan.TP(),
            scan.ExplainInfo()
        ));
    } else if let Some(selection) = plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalSelection>()
    {
        output.push(format!("Selection {}", selection.ExplainInfo()));
    } else {
        output.push(plan.tp(&[]));
    }
    for child in plan.children() {
        physical_plan_diagnostics(child, output);
    }
}

/// 断言内置函数在注册表中的参数个数与 PB 签名映射契约。
fn assert_builtin_contract(
    name: &str,
    valid_arities: &[usize],
    invalid_arities: &[usize],
    signatures: &[(&str, &str)],
) {
    assert!(
        expression::formal_registry::IsFunctionSupported(name),
        "{name} must be present in the canonical builtin registry"
    );
    for arity in valid_arities {
        expression::formal_registry::VerifyArgsWrapper(name, *arity)
            .unwrap_or_else(|error| panic!("{name}/{arity} must be valid: {error}"));
    }
    for arity in invalid_arities {
        assert!(
            expression::formal_registry::VerifyArgsWrapper(name, *arity).is_err(),
            "{name}/{arity} must be rejected"
        );
    }
    for (signature, expected) in signatures {
        assert_eq!(
            expression::PBSignatureFunctionName(signature),
            Some(*expected),
            "PB signature {signature} must map to the Go builtin name"
        );
    }
}

/// 仅验证迁移后的集成 SQL 仍可被解析器接受。
fn assert_sql_parses(sql: &str) {
    parser_dependency::New()
        .ParseOneStmt(sql, "", "")
        .unwrap_or_else(|error| panic!("parse migrated integration SQL {sql:?}: {error}"));
}

/// 断言标量函数可下推到指定存储引擎（TiKV/TiFlash）。
fn assert_pushdown_contract(
    name: &str,
    signature: &str,
    arity: usize,
    store: expression::infer_pushdown::StoreType,
) {
    use expression::infer_pushdown::{
        Datum, Expression, FieldType, PushDownContext, Signature, can_expr_push_down,
    };

    expression::formal_registry::VerifyArgsWrapper(name, arity)
        .unwrap_or_else(|error| panic!("{name}/{arity} must be registered: {error}"));
    let expression = Expression::scalar(
        name,
        Signature::Generic(signature.to_owned()),
        (0..arity)
            .map(|value| Expression::constant(Datum::Int(value as i64)))
            .collect(),
        FieldType::integer(),
    );
    assert!(
        can_expr_push_down(
            &PushDownContext::new(false, None, None, 1024),
            &expression,
            store,
            false
        ),
        "{name}/{signature} must be pushable to {store:?}"
    );
}

/// 构造 typed planbuilder 用的简易表元数据（首列为主键）。
fn typed_table(name: &str, columns: &[&str]) -> crate::planbuilder::TableInfo {
    crate::planbuilder::TableInfo {
        id: 9_000,
        db: "test".into(),
        name: name.into(),
        columns: columns
            .iter()
            .enumerate()
            .map(|(offset, name)| crate::planbuilder::ColumnInfo {
                id: offset as i64 + 1,
                name: (*name).into(),
                offset,
                field_type: crate::task::FieldType {
                    code: crate::task::TypeCode::Int,
                    flen: 11,
                    decimal: 0,
                    unsigned: false,
                },
                generated: false,
                stored: false,
                hidden: false,
                primary_key: offset == 0,
            })
            .collect(),
        indices: Vec::new(),
        partitions: Vec::new(),
        common_handle: false,
        pk_is_handle: true,
        temporary: false,
    }
}

/// 将 TableInfo 包装为逻辑计划构建用的 TableSource ResultSet。
fn typed_table_source(
    table: crate::planbuilder::TableInfo,
) -> crate::logical_plan_builder::ResultSet {
    crate::logical_plan_builder::ResultSet::Table(crate::logical_plan_builder::TableSource {
        table,
        alias: None,
        lateral: false,
        prefer_tiflash: false,
        prefer_tikv: true,
    })
}

/// typed 计划树是否满足谓词（用于 DML 运行时路径断言）。
fn typed_plan_contains(
    plan: &crate::task::PlanNode,
    predicate: &dyn Fn(&crate::task::PlanKind) -> bool,
) -> bool {
    predicate(&plan.kind)
        || plan
            .children
            .iter()
            .any(|child| typed_plan_contains(child, predicate))
}

/// 前序收集 typed PlanKind 调试名。
fn typed_plan_kinds(plan: &crate::task::PlanNode, output: &mut Vec<String>) {
    output.push(format!("{:?}", plan.kind));
    for child in &plan.children {
        typed_plan_kinds(child, output);
    }
}

// —— 隔离读 / 聚合下推 / 分区裁剪 / 非只读写路径 ——
#[test]
pub fn test_none_access_paths_found_by_isolation_read() {
    let tiflash = integration_plan_context(
        &[kv_dependency::StoreType::TiFlash],
        "tiflash",
        false,
        false,
    );
    let tikv_path = planner_util_dependency::AccessPath {
        StoreType: kv_dependency::StoreType::TiKV,
        ..Default::default()
    };
    let error = match planner_util_dependency::FilterPathByIsolationRead(
        tiflash.as_ref(),
        vec![tikv_path.clone()],
        crate::ast::NewCIStr("t"),
        crate::ast::NewCIStr("test"),
    ) {
        Err(error) => error,
        Ok(_) => panic!("TiFlash-only isolation must reject the sole TiKV path"),
    };
    assert_eq!(
        error.to_string(),
        "No access path for table 't' is found with 'tidb_isolation_read_engines' = 'tiflash', valid values can be 'tikv'. Please check tiflash replica."
    );

    let system_paths = planner_util_dependency::FilterPathByIsolationRead(
        tiflash.as_ref(),
        vec![tikv_path.clone()],
        crate::ast::NewCIStr("stats_meta"),
        crate::ast::NewCIStr("mysql"),
    )
    .expect("system schemas bypass isolation-read filtering");
    assert_eq!(system_paths.len(), 1);

    let mixed = integration_plan_context(
        &[
            kv_dependency::StoreType::TiFlash,
            kv_dependency::StoreType::TiKV,
        ],
        "tiflash, tikv",
        false,
        false,
    );
    assert_eq!(
        planner_util_dependency::FilterPathByIsolationRead(
            mixed.as_ref(),
            vec![tikv_path],
            crate::ast::NewCIStr("t"),
            crate::ast::NewCIStr("test"),
        )
        .expect("TiKV remains available in the mixed setting")
        .len(),
        1
    );
}

#[test]
pub fn test_agg_push_down_engine() {
    let tiflash = integration_plan_context(
        &[kv_dependency::StoreType::TiFlash],
        "tiflash",
        false,
        false,
    );
    let tiflash_plan = optimize_integration_query(
        "select /*+ read_from_storage(tiflash[t]) */ approx_count_distinct(a) from t",
        &tiflash,
        true,
        false,
    );
    let mut tiflash_path = Vec::new();
    physical_plan_exact_types(tiflash_plan.as_ref(), &mut tiflash_path);
    assert_eq!(
        tiflash_path,
        ["StreamAgg", "TableReader", "StreamAgg", "TableFullScan"],
        "TiFlash approx_count_distinct must preserve the complete Go plan tree"
    );
    let final_agg = tiflash_plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalStreamAgg>()
        .expect("TiFlash root must be final StreamAgg");
    let partial_agg = tiflash_plan.children()[0].children()[0]
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalStreamAgg>()
        .expect("TableReader child must be partial StreamAgg");
    let final_arg = final_agg.BasePhysicalAgg.AggFuncs[0].Args[0]
        .as_any()
        .downcast_ref::<expression::Column>()
        .expect("final approx_count_distinct must consume the partial aggregate column");
    assert_eq!(
        final_arg.UniqueID,
        partial_agg.schema().Columns[0].UniqueID,
        "Go plan_tree renders this linked intermediate column as `Column`"
    );
    assert_eq!(
        tiflash_plan.explain_info(),
        format!(
            "funcs:approx_count_distinct(Column#{})->Column#{}",
            final_arg.UniqueID,
            tiflash_plan.schema().Columns[0].UniqueID
        )
    );
    let tiflash_scan = tiflash_plan.children()[0].children()[0].children()[0]
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalTableScan>()
        .expect("TiFlash partial StreamAgg child must be TableFullScan");
    assert_eq!(tiflash_scan.AccessObject(), "table:t");

    let tikv = integration_plan_context(&[kv_dependency::StoreType::TiKV], "tikv", false, false);
    let tikv_plan =
        optimize_integration_query("select approx_count_distinct(a) from t", &tikv, true, false);
    let mut tikv_path = Vec::new();
    physical_plan_exact_types(tikv_plan.as_ref(), &mut tikv_path);
    assert_eq!(
        tikv_path,
        ["HashAgg", "TableReader", "TableFullScan"],
        "TiKV approx_count_distinct must preserve the complete Go plan tree"
    );
    assert_eq!(
        tikv_plan.explain_info(),
        format!(
            "funcs:approx_count_distinct(test.t.a)->Column#{}",
            tikv_plan.schema().Columns[0].UniqueID
        )
    );
}
#[test]
pub fn test_partition_pruning_for_eq() {
    let context = integration_plan_context(&[kv_dependency::StoreType::TiKV], "tikv", false, false);
    let plan = optimize_integration_query(
        "select * from t where a = '2020-01-01 00:00:00'",
        &context,
        false,
        true,
    );
    let scan = find_physical_table_scan(plan.as_ref())
        .expect("partition-pruned plan must retain a physical table scan");
    assert!(
        scan.IsPartition,
        "non-monotonic weekday equality must produce a partition scan: {}",
        scan.AccessObject()
    );
    assert_eq!(scan.PhysicalTableID, 1101, "only p0 may remain");
    assert_eq!(scan.AccessObject(), "table:t, partition:p0");
}

#[test]
pub fn test_not_read_only_sql_on_ti_flash() {
    let sqls = [
        "select * from t for update",
        "insert into t select * from t",
        "insert into t select * from t where t.a = ?",
    ];
    for sql in sqls {
        parser_dependency::New()
            .ParseOneStmt(sql, "", "")
            .unwrap_or_else(|error| panic!("parse non-read-only TiFlash SQL {sql:?}: {error}"));
    }
    let context = integration_plan_context(&[], "tiflash", true, false);
    assert!(
        context
            .GetSessionVars()
            .GetIsolationReadEngines()
            .is_empty()
    );
    assert_eq!(
        context
            .GetSessionVars()
            .GetSystemVar(vardef_dependency::TiDBIsolationReadEngines),
        Some("tiflash".to_owned())
    );
    let expected = [
        "No access path for table 't' is found with 'tidb_isolation_read_engines' = 'tiflash', valid values can be 'tiflash, tikv'. Please check tiflash replica or check if the query is not readonly and sql mode is strict.",
        "No access path for table 't' is found with 'tidb_isolation_read_engines' = 'tiflash', valid values can be 'tikv, tiflash'. Please check tiflash replica or check if the query is not readonly and sql mode is strict.",
    ];
    let info_schema = integration_info_schema(true, false, "t", false, false);
    for sql in sqls {
        let actual = match try_optimize_integration_query_with_schema_and_stats_and_isolation_filter(
            sql,
            &context,
            info_schema.clone(),
            1_000.0,
            true,
            true,
        ) {
            Err(error) => error,
            Ok(_) => panic!("non-read-only TiFlash SQL must fail: {sql:?}"),
        };
        assert!(
            expected.iter().any(|expected| actual.ends_with(expected)),
            "non-read-only TiFlash SQL {sql:?} returned {actual:?}, expected {expected:?}"
        );
    }

    let readonly_context =
        integration_plan_context(&[kv_dependency::StoreType::TiFlash], "tiflash", false, true);
    let readonly_plan = try_optimize_integration_query_with_schema_and_stats_and_isolation_filter(
        "select * from t",
        &readonly_context,
        info_schema,
        1_000.0,
        true,
        true,
    )
    .expect("a later read-only statement must regain its configured TiFlash engine");
    assert!(
        readonly_context
            .GetSessionVars()
            .GetIsolationReadEngines()
            .contains(&kv_dependency::StoreType::TiFlash),
        "query-level strict filtering must not leak into the next read-only statement"
    );
    assert_eq!(
        readonly_context
            .GetSessionVars()
            .GetSystemVar(vardef_dependency::TiDBIsolationReadEngines),
        Some("tiflash".to_owned()),
        "strict filtering must preserve the configured system-variable text"
    );
    assert_eq!(
        find_physical_table_scan(readonly_plan.as_ref())
            .expect("read-only plan must contain a physical table scan")
            .StoreType,
        kv_dependency::StoreType::TiFlash,
        "the restored read-only statement must actually plan on TiFlash"
    );
}

/// 构造 INSERT…SELECT 的 typed 集成计划，供 DML 运行时断言。
fn build_insert_select_integration(
    sql: &str,
    context: &base::ContextRef,
) -> Result<Box<dyn base::Plan>, String> {
    let statement = crate::ast::NodeRef::new(
        parser_dependency::New()
            .ParseOneStmt(sql, "", "")
            .map_err(|error| format!("parse INSERT ... SELECT: {error}"))?,
    );
    let (mut builder, _) = crate::NewPlanBuilder()
        .withDataSourceProvider(Arc::new(IntegrationStatsProvider {
            row_count: 1_000.0,
            pseudo: true,
            apply_isolation_filter: true,
        }))
        .Init(
            context.clone(),
            integration_multi_info_schema(&["t", "s"], false, None, &[]),
            hint_dependency::NewQBHintHandler(None),
        );
    match builder
        .BuildNodeRef(crate::context::TODO(), &statement)
        .map_err(|error| format!("build INSERT ... SELECT: {error}"))?
    {
        crate::BuiltRuntimePlan::NonLogical(plan) => {
            assert!(builder.GetIsForUpdateRead());
            Ok(plan)
        }
        crate::BuiltRuntimePlan::Logical(_) => {
            Err("INSERT ... SELECT unexpectedly produced a logical statement plan".to_owned())
        }
    }
}

#[test]
fn test_insert_select_planbuilder_runtime() {
    let context = integration_plan_context_with_params(
        &[kv_dependency::StoreType::TiKV],
        "tikv",
        false,
        false,
        vec![
            expression::types::NewIntDatum(7),
            expression::types::NewIntDatum(8),
        ],
        None,
    );
    let plan = build_insert_select_integration(
        "insert into t (a) select ? on duplicate key update b = ?",
        &context,
    )
    .expect("parameterized INSERT ... SELECT ON DUPLICATE must build");
    let insert = plan
        .as_any()
        .downcast_ref::<physicalop_dependency::Insert>()
        .expect("top-level statement plan must be physicalop::Insert");
    assert_eq!(insert.RowLen, 1);
    assert_eq!(insert.Columns.len(), 1);
    assert_eq!(insert.Columns[0].Name.L, "a");
    assert_eq!(insert.OnDuplicate.len(), 1);
    assert_eq!(insert.OnDuplicate[0].ColName.L, "b");
    assert_eq!(
        insert
            .Table
            .as_ref()
            .expect("INSERT target table")
            .Meta()
            .Name
            .L,
        "t"
    );
    assert_eq!(
        insert
            .SelectPlan
            .as_ref()
            .expect("optimized SELECT input")
            .schema()
            .Len(),
        1
    );

    let repeated_target = build_insert_select_integration(
        "insert into t (a) select ? on duplicate key update b = default, b = ?",
        &context,
    )
    .expect("Go permits repeated ON DUPLICATE targets and binds bare DEFAULT to its target");
    let insert = repeated_target
        .as_any()
        .downcast_ref::<physicalop_dependency::Insert>()
        .expect("repeated-target statement plan must be Insert");
    assert_eq!(insert.OnDuplicate.len(), 2);

    let mismatch = match build_insert_select_integration("insert into t (a, b) select ?", &context)
    {
        Err(error) => error,
        Ok(_) => panic!("INSERT projection count mismatch must fail"),
    };
    assert!(
        mismatch.ends_with("Column count doesn't match value count at row 1"),
        "unexpected projection-count error: {mismatch}"
    );

    let extra_source = build_insert_select_integration(
        "insert into t (a) select s.a from s on duplicate key update b = s.c",
        &context,
    )
    .expect("ON DUPLICATE may reference an unprojected SELECT source column");
    let insert = extra_source
        .as_any()
        .downcast_ref::<physicalop_dependency::Insert>()
        .expect("extra-source statement plan must be Insert");
    assert_eq!(insert.RowLen, 1);
    assert_eq!(
        insert
            .SelectPlan
            .as_ref()
            .expect("optimized SELECT input")
            .schema()
            .Len(),
        2
    );
    assert_eq!(
        insert
            .Schema4OnDuplicate
            .as_ref()
            .expect("ON DUPLICATE schema")
            .Len(),
        insert.TableSchema.as_ref().expect("target schema").Len() * 2 + 1
    );
}

// —— TiFlash 标量/时间/位运算表达式下推契约 ——
#[test]
pub fn test_time_to_sec_push_down_to_ti_flash() {
    assert_builtin_contract(
        "time_to_sec",
        &[1],
        &[0, 2],
        &[("TimeToSec", "time_to_sec")],
    );
}
#[test]
pub fn test_right_shift_push_down_to_ti_flash() {
    assert_builtin_contract("rightshift", &[2], &[1, 3], &[("RightShift", "rightshift")]);
}
#[test]
pub fn test_bit_column_push_down() {
    use expression::infer_pushdown::StoreType;

    for sql in [
        "select b from t1 where t1.b > (select min(t2.b) from t2 where t2.a < t1.a)",
        "select a from t1 where ascii(a)=65",
        "select a from t1 where concat(a, 'A')='AA'",
        "select a from t1 where binary a='A'",
        "select a from t1 where cast(a as char)='A'",
    ] {
        assert_sql_parses(sql);
    }
    assert_pushdown_contract("ascii", "Ascii", 1, StoreType::TiKV);
    assert_pushdown_contract("concat", "Concat", 2, StoreType::TiKV);
    assert_pushdown_contract("cast", "CastString", 1, StoreType::TiKV);
}

#[test]
pub fn test_sysdate_push_down() {
    use expression::infer_pushdown::StoreType;

    let sql = "select /*+read_from_storage(tikv[t])*/ * from t where d > sysdate()";
    assert_sql_parses(sql);
    assert_pushdown_contract("sysdate", "SysdateWithoutFsp", 0, StoreType::TiKV);
    assert_pushdown_contract("gt", "GTDatetime", 2, StoreType::TiKV);
}

#[test]
pub fn test_time_scalar_function_push_down_result() {
    use expression::infer_pushdown::StoreType;

    let cases = [
        (
            "hour",
            1,
            "select col1, hour(col1) from t where hour(col1)=hour('2022-03-24 01:02:03.040506');",
        ),
        (
            "month",
            1,
            "select col1, month(col1) from t where month(col1)=month('2022-03-24 01:02:03.040506');",
        ),
        (
            "minute",
            1,
            "select col1, minute(col1) from t where minute(col1)=minute('2022-03-24 01:02:03.040506');",
        ),
        (
            "second",
            1,
            "select col1, second(col1) from t where second(col1)=second('2022-03-24 01:02:03.040506');",
        ),
        (
            "microsecond",
            1,
            "select col1, microsecond(col1) from t where microsecond(col1)=microsecond('2022-03-24 01:02:03.040506');",
        ),
        (
            "dayname",
            1,
            "select col1, dayName(col1) from t where dayName(col1)=dayName('2022-03-24 01:02:03.040506');",
        ),
        (
            "dayofmonth",
            1,
            "select col1, dayOfMonth(col1) from t where dayOfMonth(col1)=dayOfMonth('2022-03-24 01:02:03.040506');",
        ),
        (
            "dayofweek",
            1,
            "select col1, dayOfWeek(col1) from t where dayOfWeek(col1)=dayOfWeek('2022-03-24 01:02:03.040506');",
        ),
        (
            "dayofyear",
            1,
            "select col1, dayOfYear(col1) from t where dayOfYear(col1)=dayOfYear('2022-03-24 01:02:03.040506');",
        ),
        (
            "date",
            1,
            "select col1, Date(col1) from t where Date(col1)=Date('2022-03-24 01:02:03.040506');",
        ),
        (
            "week",
            1,
            "select col1, Week(col1) from t where Week(col1)=Week('2022-03-24 01:02:03.040506');",
        ),
        (
            "time_to_sec",
            1,
            "select col1, time_to_sec(col1) from t where time_to_sec(col1)=time_to_sec('2022-03-24 01:02:03.040506');",
        ),
        (
            "datediff",
            2,
            "select col1, DateDiff(col1, col2) from t where DateDiff(col1, col2)=DateDiff('2022-03-24 01:02:03.040506', '9999-12-31 23:59:59');",
        ),
        (
            "monthname",
            1,
            "select col1, MonthName(col1) from t where MonthName(col1)=MonthName('2022-03-24 01:02:03.040506');",
        ),
        (
            "makedate",
            2,
            "select col1, MakeDate(9999, 31) from t where MakeDate(y, d)=MakeDate(9999, 31);",
        ),
        (
            "maketime",
            3,
            "select col1, MakeTime(12, 12, 31) from t where MakeTime(m, m, d)=MakeTime(12, 12, 31);",
        ),
    ];
    for (name, arity, sql) in cases {
        assert_sql_parses(sql);
        if name == "dayname" {
            expression::formal_registry::VerifyArgsWrapper(name, arity)
                .expect("dayname must remain registered even though TiKV cannot encode it");
        } else {
            assert_pushdown_contract(name, name, arity, StoreType::TiKV);
        }
    }
}

#[test]
pub fn test_number_function_push_down() {
    use expression::infer_pushdown::StoreType;

    let cases = [
        (
            "mod",
            2,
            "select a, mod(a,2) from t where mod(-1,2)=mod(a,2);",
        ),
        (
            "mod",
            2,
            "select b, mod(b,2) from t where mod(61,2)=mod(b,2);",
        ),
        (
            "unhex",
            1,
            "select b,unhex(b) from t where unhex(61) = unhex(b)",
        ),
        ("oct", 1, "select b, oct(b) from t where oct(61) = oct(b)"),
        ("sin", 1, "select c, sin(c) from t where sin(4.4) = sin(c)"),
        (
            "asin",
            1,
            "select c, asin(c) from t where asin(4.4) = asin(c)",
        ),
        ("cos", 1, "select c, cos(c) from t where cos(4.4) = cos(c)"),
        (
            "acos",
            1,
            "select c, acos(c) from t where acos(4.4) = acos(c)",
        ),
        ("atan", 1, "select b,atan(b) from t where atan(61)=atan(b)"),
        (
            "atan2",
            2,
            "select b, atan2(b, c) from t where atan2(61,4.4)=atan2(b,c)",
        ),
        ("cot", 1, "select b,cot(b) from t where cot(61)=cot(b)"),
        ("pi", 0, "select c from t where pi() < c"),
    ];
    for (name, arity, sql) in cases {
        assert_sql_parses(sql);
        expression::formal_registry::VerifyArgsWrapper(name, arity)
            .unwrap_or_else(|error| panic!("{name}/{arity} must be registered: {error}"));
        if !matches!(name, "unhex" | "oct") {
            assert_pushdown_contract(name, name, arity, StoreType::TiKV);
        }
    }
}

#[test]
pub fn test_scalar_function_push_down() {
    use expression::infer_pushdown::StoreType;

    let cases = [
        (
            "right",
            2,
            "RightUTF8",
            "select /*+read_from_storage(tikv[t])*/ * from t where right(c,1);",
        ),
        (
            "mod",
            2,
            "ModInt",
            "select /*+read_from_storage(tikv[t])*/ * from t where mod(id, id);",
        ),
        (
            "mod",
            2,
            "ModInt",
            "select /*+read_from_storage(tikv[t])*/ * from t where mod(id, id2);",
        ),
        (
            "mod",
            2,
            "ModInt",
            "select /*+read_from_storage(tikv[t])*/ * from t where mod(id2, id);",
        ),
        (
            "mod",
            2,
            "ModInt",
            "select /*+read_from_storage(tikv[t])*/ * from t where mod(id2, id2);",
        ),
        (
            "sin",
            1,
            "Sin",
            "select /*+read_from_storage(tikv[t])*/ * from t where sin(id);",
        ),
        (
            "asin",
            1,
            "Asin",
            "select /*+read_from_storage(tikv[t])*/ * from t where asin(id);",
        ),
        (
            "cos",
            1,
            "Cos",
            "select /*+read_from_storage(tikv[t])*/ * from t where cos(id);",
        ),
        (
            "acos",
            1,
            "Acos",
            "select /*+read_from_storage(tikv[t])*/ * from t where acos(id);",
        ),
        (
            "atan",
            1,
            "Atan1Arg",
            "select /*+read_from_storage(tikv[t])*/ * from t where atan(id);",
        ),
        (
            "atan2",
            2,
            "Atan2Args",
            "select /*+read_from_storage(tikv[t])*/ * from t where atan2(id,id);",
        ),
        (
            "hour",
            1,
            "Hour",
            "select /*+read_from_storage(tikv[t])*/ * from t where hour(d);",
        ),
        (
            "minute",
            1,
            "Minute",
            "select /*+read_from_storage(tikv[t])*/ * from t where minute(d);",
        ),
        (
            "second",
            1,
            "Second",
            "select /*+read_from_storage(tikv[t])*/ * from t where second(d);",
        ),
        (
            "month",
            1,
            "Month",
            "select /*+read_from_storage(tikv[t])*/ * from t where month(d);",
        ),
        (
            "dayofmonth",
            1,
            "DayOfMonth",
            "select /*+read_from_storage(tikv[t])*/ * from t where dayofmonth(d);",
        ),
        (
            "from_days",
            1,
            "FromDays",
            "select /*+read_from_storage(tikv[t])*/ * from t where from_days(id);",
        ),
        (
            "pi",
            0,
            "Pi",
            "select /*+read_from_storage(tikv[t])*/ * from t where pi() > id;",
        ),
        (
            "round",
            1,
            "RoundReal",
            "select /*+read_from_storage(tikv[t])*/ * from t where round(b)",
        ),
        (
            "date",
            1,
            "Date",
            "select /*+read_from_storage(tikv[t])*/ * from t where date(d)",
        ),
        (
            "week",
            1,
            "WeekWithoutMode",
            "select /*+read_from_storage(tikv[t])*/ * from t where week(d)",
        ),
        (
            "datediff",
            2,
            "DateDiff",
            "select /*+read_from_storage(tikv[t])*/ * from t where datediff(d,d)",
        ),
        (
            "sysdate",
            0,
            "SysdateWithoutFsp",
            "select /*+read_from_storage(tikv[t])*/ * from t where d > sysdate()",
        ),
        (
            "ascii",
            1,
            "Ascii",
            "select /*+read_from_storage(tikv[t])*/ * from t where ascii(e);",
        ),
        (
            "json_valid",
            1,
            "JsonValidString",
            "select /*+read_from_storage(tikv[t])*/ * from t where json_valid(c)=1;",
        ),
        (
            "json_contains",
            2,
            "JsonContains",
            "select /*+read_from_storage(tikv[t])*/ * from t where json_contains(c, '1');",
        ),
    ];
    for (name, arity, signature, sql) in cases {
        assert_sql_parses(sql);
        assert_pushdown_contract(name, signature, arity, StoreType::TiKV);
    }
}

#[test]
pub fn test_reverse_utf8_push_down_to_ti_flash() {
    assert_builtin_contract("reverse", &[1], &[0, 2], &[("ReverseUTF8", "reverse")]);
}
#[test]
pub fn test_reverse_push_down_to_ti_flash() {
    assert_builtin_contract("reverse", &[1], &[0, 2], &[("ReverseBinary", "reverse")]);
}
#[test]
pub fn test_space_push_down_to_ti_flash() {
    assert_builtin_contract("space", &[1], &[0, 2], &[("Space", "space")]);
}
// —— EXPLAIN ANALYZE DML、读写冲突与 hypo index hint ——
#[test]
pub fn test_explain_analyze_dml2() {
    for sql in [
        "explain analyze insert into t () values ()",
        "explain analyze insert into t (a) values (99000000000)",
        "explain analyze insert into t (a) values (null), (99000000000)",
        "explain analyze insert ignore into t values (null,1), (2, 2), (99000000000, 3), (100000000000, 4)",
        "explain analyze insert into t values (null,null), (1,1),(2,2) on duplicate key update a = a + 100000000000",
        "explain analyze replace into t () values ()",
        "explain analyze replace into t (a) values (null), (99000000000)",
        "explain analyze update t set a=a*100000000000",
    ] {
        assert_sql_parses(sql);
    }
    let table = typed_table("t", &["a", "b"]);
    for replace in [false, true] {
        let statement = crate::planbuilder::Statement::Explain {
            format: "row".into(),
            analyze: true,
            explore: false,
            stmt: Box::new(crate::planbuilder::Statement::Insert(
                crate::planbuilder::InsertStatement {
                    table: table.clone(),
                    columns: vec!["a".into(), "b".into()],
                    values: vec![vec![
                        crate::planbuilder::Value::Int(1),
                        crate::planbuilder::Value::Int(2),
                    ]],
                    select: None,
                    on_duplicate: Vec::new(),
                    replace,
                },
            )),
        };
        let mut builder = crate::planbuilder::NewPlanBuilder(&[]);
        let crate::planbuilder::BuiltPlan::Explain {
            target, analyze, ..
        } = builder
            .Build(&statement)
            .expect("build typed EXPLAIN ANALYZE INSERT/REPLACE")
        else {
            panic!("typed explain must return BuiltPlan::Explain")
        };
        assert!(analyze);
        let crate::planbuilder::BuiltPlan::Insert {
            replace: actual, ..
        } = *target
        else {
            panic!("explain target must retain insert plan")
        };
        assert_eq!(actual, replace);
    }
    let insert_cases = vec![
        (
            Vec::<String>::new(),
            vec![vec![
                crate::planbuilder::Value::Default,
                crate::planbuilder::Value::Default,
            ]],
            Vec::new(),
            false,
        ),
        (
            vec!["a".into()],
            vec![vec![crate::planbuilder::Value::Int(99_000_000_000)]],
            Vec::new(),
            false,
        ),
        (
            vec!["a".into()],
            vec![
                vec![crate::planbuilder::Value::Null],
                vec![crate::planbuilder::Value::Int(99_000_000_000)],
            ],
            Vec::new(),
            false,
        ),
        (
            Vec::new(),
            vec![
                vec![
                    crate::planbuilder::Value::Null,
                    crate::planbuilder::Value::Null,
                ],
                vec![
                    crate::planbuilder::Value::Int(1),
                    crate::planbuilder::Value::Int(1),
                ],
                vec![
                    crate::planbuilder::Value::Int(2),
                    crate::planbuilder::Value::Int(2),
                ],
            ],
            vec![(
                "a".into(),
                crate::task::Expression {
                    name: "a + 100000000000".into(),
                    ..Default::default()
                },
            )],
            false,
        ),
        (
            Vec::new(),
            vec![vec![
                crate::planbuilder::Value::Default,
                crate::planbuilder::Value::Default,
            ]],
            Vec::new(),
            true,
        ),
        (
            vec!["a".into()],
            vec![
                vec![crate::planbuilder::Value::Null],
                vec![crate::planbuilder::Value::Int(99_000_000_000)],
            ],
            Vec::new(),
            true,
        ),
    ];
    for (columns, values, on_duplicate, replace) in insert_cases {
        let expected_rows = values.len();
        let expected_on_duplicate = on_duplicate.len();
        let statement = crate::planbuilder::Statement::Explain {
            format: "row".into(),
            analyze: true,
            explore: false,
            stmt: Box::new(crate::planbuilder::Statement::Insert(
                crate::planbuilder::InsertStatement {
                    table: table.clone(),
                    columns,
                    values,
                    select: None,
                    on_duplicate,
                    replace,
                },
            )),
        };
        let mut builder = crate::planbuilder::NewPlanBuilder(&[]);
        let crate::planbuilder::BuiltPlan::Explain {
            target, analyze, ..
        } = builder.Build(&statement).expect("build Go DML2 typed case")
        else {
            panic!("DML2 case must remain EXPLAIN")
        };
        assert!(analyze);
        let crate::planbuilder::BuiltPlan::Insert {
            rows,
            on_duplicate,
            replace: actual_replace,
            ..
        } = *target
        else {
            panic!("DML2 EXPLAIN target must remain INSERT/REPLACE")
        };
        assert_eq!(rows.len(), expected_rows);
        assert_eq!(on_duplicate.len(), expected_on_duplicate);
        assert_eq!(actual_replace, replace);
    }
    let mut builder = crate::planbuilder::NewPlanBuilder(&[]);
    let update = builder
        .buildUpdate(&crate::logical_plan_builder::UpdateStmt {
            source: typed_table_source(table.clone()),
            assignments: vec![crate::logical_plan_builder::Assignment {
                table_id: table.id,
                column: 1,
                expression: crate::task::Expression::default(),
            }],
            where_clause: Some(crate::task::Expression::default()),
            order_by: Vec::new(),
            limit: None,
            ignore: false,
        })
        .expect("build typed UPDATE plan");
    assert!(matches!(update.kind, crate::task::PlanKind::Other(ref name) if name == "Update"));
    let delete = builder
        .buildDelete(&crate::logical_plan_builder::DeleteStmt {
            source: typed_table_source(table.clone()),
            tables: vec![table.id],
            where_clause: Some(crate::task::Expression::default()),
            order_by: Vec::new(),
            limit: None,
            ignore: false,
        })
        .expect("build typed DELETE plan");
    assert!(matches!(delete.kind, crate::task::PlanKind::Other(ref name) if name == "Delete"));
}

#[test]
pub fn test_conflict_read_from_storage() {
    for sql in [
        "select /*+ read_from_storage(tikv[t partition(p0)], tiflash[t partition(p1, p2)]) */ * from t",
        "select /*+ read_from_storage(tikv[t], tiflash[t]) */ * from t",
    ] {
        let context = integration_plan_context(
            &[
                kv_dependency::StoreType::TiKV,
                kv_dependency::StoreType::TiFlash,
            ],
            "tikv, tiflash",
            false,
            false,
        );
        let plan = optimize_integration_query(sql, &context, true, false);
        assert!(physical_plan_contains::<
            physicalop_dependency::PhysicalTableScan,
        >(plan.as_ref()));
        let warnings = context.GetSessionVars().StmtCtx.GetWarnings();
        assert_eq!(
            warnings.len(),
            1,
            "unexpected warnings for {sql}: {warnings:?}"
        );
        assert_eq!(
            warnings[0].Err.as_ref().map(ToString::to_string).as_deref(),
            Some(
                "Storage hints are conflict, you can only specify one storage type of table test.t"
            ),
            "wrong warning for {sql}"
        );
    }
}

#[test]
pub fn test_hypo_index_hint() {
    let context = integration_plan_context(&[kv_dependency::StoreType::TiKV], "tikv", false, false);
    let info_schema = integration_multi_info_schema(&["t1", "t2"], false, None, &[]);
    for (plain, hinted) in [
        (
            "select a from t1 where a=1",
            "select /*+ HYPO_INDEX(t1, idx_a, a) */ a from t1 where a=1",
        ),
        (
            "select a from t1 where a=1 and b=1",
            "select /*+ HYPO_INDEX(t1, idx_a, a, b) */ a from t1 where a=1 and b=1",
        ),
        (
            "select a from t1 where a=1 and b=1 and c<1",
            "select /*+ HYPO_INDEX(t1, idx_a, a, b, c) */ a from t1 where a=1 and b=1 and c<1",
        ),
    ] {
        let plain_plan =
            optimize_integration_query_with_schema(plain, &context, info_schema.clone());
        let mut plain_path = Vec::new();
        physical_plan_exact_types(plain_plan.as_ref(), &mut plain_path);
        assert!(plain_path.iter().any(|kind| kind == "TableFullScan"));
        assert!(!plain_path.iter().any(|kind| kind == "IndexRangeScan"));

        let hinted_plan =
            optimize_integration_query_with_schema(hinted, &context, info_schema.clone());
        let mut hinted_path = Vec::new();
        physical_plan_exact_types(hinted_plan.as_ref(), &mut hinted_path);
        assert!(
            hinted_path.iter().any(|kind| kind == "IndexRangeScan"),
            "valid HYPO_INDEX must produce IndexRangeScan: {hinted_path:?}"
        );
    }

    let plain_join = optimize_integration_query_with_schema(
        "select 1 from t1, t2 where t1.a=1 and t1.b=t2.b",
        &context,
        info_schema.clone(),
    );
    assert!(physical_plan_contains::<
        physicalop_dependency::PhysicalHashJoin,
    >(plain_join.as_ref()));
    let hinted_join = optimize_integration_query_with_schema(
        "select /*+ HYPO_INDEX(t1, idx_ab, a, b), HYPO_INDEX(t2, idx_b, b) */ 1 from t1, t2 where t1.a=1 and t1.b=t2.b",
        &context,
        info_schema,
    );
    let mut hinted_join_path = Vec::new();
    physical_plan_exact_types(hinted_join.as_ref(), &mut hinted_join_path);
    assert!(
        hinted_join_path.iter().any(|kind| kind == "IndexHashJoin"),
        "valid two-table HYPO_INDEX must produce IndexHashJoin: {hinted_join_path:?}"
    );
}

/// 断言非法 HYPO_INDEX hint 产生期望告警文本。
fn assert_invalid_hypo_index(sql: &str, expected_warning: &str) {
    let context = integration_plan_context(&[kv_dependency::StoreType::TiKV], "tikv", false, false);
    let plan = optimize_integration_query_with_schema(
        sql,
        &context,
        integration_info_schema(false, false, "t1", false, false),
    );
    let mut types = Vec::new();
    physical_plan_types(plan.as_ref(), &mut types);
    assert!(
        types.iter().any(|kind| kind == "TableScan"),
        "invalid HYPO_INDEX must fall back to TableFullScan, got {types:?}"
    );
    assert!(
        !types.iter().any(|kind| kind == "IndexScan"),
        "invalid HYPO_INDEX unexpectedly installed an index: {types:?}"
    );
    let warnings = context.GetSessionVars().StmtCtx.GetWarnings();
    assert_eq!(
        warnings.len(),
        1,
        "unexpected warnings for {sql}: {warnings:?}"
    );
    assert_eq!(
        warnings[0].Err.as_ref().map(ToString::to_string).as_deref(),
        Some(expected_warning),
        "wrong warning for {sql}"
    );
}

#[test]
fn test_hypo_index_hint_missing_columns() {
    assert_invalid_hypo_index(
        "select /*+ HYPO_INDEX(t1, idx_a) */ a from t1 where a = 1",
        "Invalid HYPO_INDEX hint, valid usage: HYPO_INDEX(tableName, indexName, cols...)",
    );
}

#[test]
fn test_hypo_index_hint_unknown_column() {
    assert_invalid_hypo_index(
        "select /*+ HYPO_INDEX(t1, idx_a, a, d) */ a from t1 where a = 1",
        "invalid HYPO_INDEX hint: can't find column d in table test.t1",
    );
}

#[test]
fn test_hypo_index_hint_unknown_table() {
    assert_invalid_hypo_index(
        "select /*+ HYPO_INDEX(tttt, idx_a, a) */ a from t1 where a = 1",
        "invalid HYPO_INDEX hint: table 'test.tttt' doesn't exist",
    );
}

#[test]
fn test_hypo_index_hint_unknown_schema() {
    assert_invalid_hypo_index(
        "select /*+ HYPO_INDEX(test1.t1, idx_a, a) */ a from t1 where a = 1",
        "invalid HYPO_INDEX hint: table 'test1.t1' doesn't exist",
    );
}

// —— 缓存表聚合下推与更多 TiFlash 函数下推 ——
#[test]
pub fn test_agg_push_to_cop_for_cached_table() {
    let sql = "select /*+AGG_TO_COP()*/ count(*) from t32157 ignore index(primary) where process_code = 'GDEP0071'";
    assert_sql_parses(sql);
    let context = integration_plan_context(&[kv_dependency::StoreType::TiKV], "tikv", false, false);
    let plan =
        optimize_integration_query_for_table(sql, &context, false, false, "t32157", false, true);
    let mut exact_path = Vec::new();
    physical_plan_exact_types(plan.as_ref(), &mut exact_path);
    assert_eq!(
        exact_path,
        [
            "StreamAgg",
            "UnionScan",
            "TableReader",
            "Selection",
            "TableFullScan",
        ],
        "cached-table AGG_TO_COP must preserve the complete Go plan tree"
    );
    let count_column_sql = "select /*+AGG_TO_COP()*/ count(a) from t32157 ignore index(primary) where process_code = 'GDEP0071'";
    let count_column_plan = optimize_integration_query_for_table(
        count_column_sql,
        &context,
        false,
        false,
        "t32157",
        false,
        true,
    );
    let mut count_column_path = Vec::new();
    physical_plan_exact_types(count_column_plan.as_ref(), &mut count_column_path);
    assert_eq!(
        count_column_path,
        [
            "StreamAgg",
            "UnionScan",
            "TableReader",
            "Selection",
            "TableFullScan",
        ],
        "cached-table count(a) must preserve the complete count(*) plan tree"
    );
    let count_column_aggregate = count_column_plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalStreamAgg>()
        .expect("cached-table count(a) root must be StreamAgg");
    let count_column_argument = count_column_aggregate.BasePhysicalAgg.AggFuncs[0].Args[0]
        .as_column()
        .expect("count(a) argument must remain a resolved source column");
    assert_eq!(count_column_argument.ID, 1);
    assert_ne!(count_column_argument.UniqueID, 0);
    assert_eq!(count_column_argument.OrigName, "test.t32157.a");

    let aggregate = plan
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalStreamAgg>()
        .expect("root must be StreamAgg");
    assert_eq!(
        aggregate.BasePhysicalAgg.ExplainInfo(),
        format!(
            "funcs:count(1)->Column#{}",
            aggregate.schema().Columns[0].UniqueID
        )
    );
    let union_scan = plan.children()[0]
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalUnionScan>()
        .expect("StreamAgg child must be UnionScan");
    assert_eq!(
        union_scan.ExplainInfo(),
        "eq(test.t32157.process_code, \"GDEP0071\")"
    );
    let reader = union_scan.children()[0]
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalTableReader>()
        .expect("UnionScan child must be TableReader");
    let selection = reader.children()[0]
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalSelection>()
        .expect("TableReader child must be Selection");
    assert_eq!(
        reader.ExplainInfo(),
        format!("data:{}", selection.explain_id(&[]))
    );
    assert_eq!(
        selection.ExplainInfo(),
        "eq(test.t32157.process_code, \"GDEP0071\")"
    );
    let table_scan = selection.children()[0]
        .as_any()
        .downcast_ref::<physicalop_dependency::PhysicalTableScan>()
        .expect("Selection child must be TableFullScan");
    assert_eq!(table_scan.TP(), "TableFullScan");
    assert_eq!(table_scan.AccessObject(), "table:t32157");
}

#[test]
pub fn test_ti_flash_fine_grained_shuffle_with_max_ti_flash_threads() {
    let sql = "explain select row_number() over w1 from t1 window w1 as (partition by c1)";
    assert_sql_parses(sql);
    for (stream_count, max_threads, expected) in [
        ("0", "10", 10),
        (
            "0",
            "-1",
            vardef_dependency::DefStreamCountWhenMaxThreadsNotSet as u64,
        ),
        (
            "0",
            "0",
            vardef_dependency::DefStreamCountWhenMaxThreadsNotSet as u64,
        ),
        ("-1", "10", 0),
        ("16", "10", 16),
    ] {
        let context = integration_plan_context_with_params_and_vars(
            &[kv_dependency::StoreType::TiFlash],
            "tiflash",
            false,
            true,
            Vec::new(),
            None,
            &[
                (
                    vardef_dependency::TiFlashFineGrainedShuffleStreamCount,
                    stream_count,
                ),
                (vardef_dependency::TiDBMaxTiFlashThreads, max_threads),
            ],
        );
        let plan = optimize_integration_query_for_table(
            "select row_number() over w1 from t1 window w1 as (partition by c1)",
            &context,
            true,
            false,
            "t1",
            false,
            false,
        );
        let window = find_physical_window(plan.as_ref())
            .expect("fine-grained shuffle query must retain PhysicalWindow");
        assert_eq!(
            window
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .TiFlashFineGrainedShuffleStreamCount,
            expected,
            "stream_count={stream_count}, tidb_max_tiflash_threads={max_threads}",
        );
        assert!(physical_plan_contains::<
            physicalop_dependency::PhysicalTableScan,
        >(plan.as_ref()));
    }
}

#[test]
pub fn test_repeat_push_down_to_ti_flash() {
    assert_builtin_contract("repeat", &[2], &[1, 3], &[("Repeat", "repeat")]);
}
#[test]
pub fn test_get_format_push_down_to_ti_flash() {
    assert_builtin_contract("get_format", &[2], &[1, 3], &[("GetFormat", "get_format")]);
}
#[test]
pub fn test_agg_with_json_push_down_to_ti_flash() {
    let context =
        integration_plan_context(&[kv_dependency::StoreType::TiFlash], "tiflash", false, true);
    for sql in [
        "explain format = 'plan_tree' select avg(a) from t",
        "explain format = 'plan_tree' select sum(a) from t",
        "explain format = 'plan_tree' select /*+ hash_agg() */ group_concat(a) from t",
    ] {
        assert_sql_parses(sql);
    }
    for sql in [
        "select avg(a) from t",
        "select sum(a) from t",
        "select /*+ hash_agg() */ group_concat(a) from t",
    ] {
        let plan =
            optimize_integration_query_for_table(sql, &context, true, false, "t", true, false);
        let mut exact_path = Vec::new();
        physical_plan_exact_types(plan.as_ref(), &mut exact_path);
        assert_eq!(
            exact_path,
            ["HashAgg", "Projection", "TableReader", "TableFullScan"],
            "JSON aggregate must preserve the exact Go root-side cast plan for {sql:?}"
        );
    }
}

#[test]
pub fn test_left_shift_push_down_to_ti_flash() {
    assert_builtin_contract("leftshift", &[2], &[1, 3], &[("LeftShift", "leftshift")]);
}
#[test]
pub fn test_hex_int_or_str_push_down_to_ti_flash() {
    assert_builtin_contract(
        "hex",
        &[1],
        &[0, 2],
        &[("HexInt", "hex"), ("HexStr", "hex")],
    );
}
#[test]
pub fn test_bin_push_down_to_ti_flash() {
    assert_builtin_contract("bin", &[1], &[0, 2], &[("Bin", "bin")]);
}
#[test]
pub fn test_elt_push_down_to_ti_flash() {
    assert_builtin_contract("elt", &[2, 5], &[0, 1], &[("Elt", "elt")]);
}
#[test]
pub fn test_regexp_instr_push_down_to_ti_flash() {
    assert_builtin_contract(
        "regexp_instr",
        &[2, 3, 4, 5, 6],
        &[0, 1, 7],
        &[("RegexpInStrSig", "regexp_instr")],
    );
}
#[test]
pub fn test_regexp_substr_push_down_to_ti_flash() {
    assert_builtin_contract(
        "regexp_substr",
        &[2, 3, 4, 5],
        &[0, 1, 6],
        &[("RegexpSubstrUtf8Sig", "regexp_substr")],
    );
}
#[test]
pub fn test_regexp_replace_push_down_to_ti_flash() {
    assert_builtin_contract(
        "regexp_replace",
        &[3, 4, 5, 6],
        &[0, 1, 2, 7],
        &[("RegexpReplaceSig", "regexp_replace")],
    );
}
#[test]
pub fn test_cast_time_as_duration_to_ti_flash() {
    use expression::infer_pushdown::{
        CastFamily, Datum, Expression, FieldKind, FieldType, PushDownContext, Signature, StoreType,
        can_expr_push_down,
    };

    assert_sql_parses("select cast(a as time), cast(b as time) from t");
    let cast = Expression::scalar(
        "cast",
        Signature::Cast(CastFamily::Duration),
        vec![Expression::constant(Datum::String(
            "2021-10-26 11:11:11".into(),
        ))],
        FieldType::new(FieldKind::Duration),
    );
    assert!(can_expr_push_down(
        &PushDownContext::new(false, None, None, 1024),
        &cast,
        StoreType::TiFlash,
        false,
    ));
}

#[test]
pub fn test_unhex_push_down_to_ti_flash() {
    assert_builtin_contract("unhex", &[1], &[0, 2], &[("UnHex", "unhex")]);
}
#[test]
pub fn test_least_gretest_string_push_down_to_ti_flash() {
    assert_builtin_contract("least", &[2, 5], &[0, 1], &[("LeastString", "least")]);
    assert_builtin_contract(
        "greatest",
        &[2, 5],
        &[0, 1],
        &[("GreatestString", "greatest")],
    );
}
// —— 写语句读路径、Point Get 加锁与计划缓存 range fallback ——
#[test]
pub fn test_ti_flash_read_for_write_stmt() {
    for sql in [
        "explain insert into t2 select a+b from t",
        "explain insert into t2 select t.a from t2 join t on t2.a = t.a",
        "explain replace into t2 select a+b from t",
        "explain update t set a=a+1 where b in (select a from t2 where t.a > t2.a)",
        "explain insert into t4 select a, b from t3 where a in (1, 2)",
    ] {
        assert_sql_parses(sql);
    }
    let strict =
        integration_plan_context(&[kv_dependency::StoreType::TiFlash], "tiflash", true, true);
    assert!(
        strict
            .GetSessionVars()
            .StmtCtx
            .TiFlashEngineRemovedDueToStrictSQLMode
    );
    assert!(strict.GetSessionVars().EnforceMPPExecution);
    let mut select_plan = crate::task::PlanNode::new(crate::task::PlanKind::TableScan);
    select_plan.store = crate::task::StoreType::TiFlash;
    let statement = crate::planbuilder::Statement::Explain {
        format: "row".into(),
        analyze: false,
        explore: false,
        stmt: Box::new(crate::planbuilder::Statement::Insert(
            crate::planbuilder::InsertStatement {
                table: typed_table("t2", &["a", "b"]),
                columns: vec!["a".into(), "b".into()],
                values: Vec::new(),
                select: Some(Box::new(crate::planbuilder::Statement::Select {
                    plan: select_plan,
                    for_update: false,
                })),
                on_duplicate: Vec::new(),
                replace: false,
            },
        )),
    };
    let mut builder = crate::planbuilder::NewPlanBuilder(&[]);
    let crate::planbuilder::BuiltPlan::Explain { target, .. } = builder
        .Build(&statement)
        .expect("build EXPLAIN INSERT SELECT")
    else {
        panic!("write explain must retain its typed target")
    };
    assert!(matches!(
        *target,
        crate::planbuilder::BuiltPlan::Insert { .. }
    ));
}

#[test]
pub fn test_point_get_with_select_lock() {
    for sql in [
        "explain select a, b from t where (a = 1 and b = 2) or (a =2 and b = 1) for update",
        "explain select a, b from t where a = 1 and b = 2 for update",
        "explain select c, d from t1 where c = 1 for update",
        "explain select c, d from t1 where c = 1 and d = 1 for update",
        "explain select c, d from t1 where (c = 1 or c = 2) and d = 1 for update",
        "explain select c, d from t1 where c in (1,2,3,4) for update",
    ] {
        assert_sql_parses(sql);
    }
    let int_type = crate::task::FieldType {
        code: crate::task::TypeCode::Int,
        flen: 11,
        decimal: 0,
        unsigned: false,
    };
    let column = |id, name: &str, offset, primary_key| crate::planbuilder::ColumnInfo {
        id,
        name: name.to_owned(),
        offset,
        field_type: int_type.clone(),
        generated: false,
        stored: false,
        hidden: false,
        primary_key,
    };
    let lock = Some(crate::point_get_plan::LockInfo {
        for_update: true,
        nowait: false,
        wait_seconds: None,
    });
    let point_query = crate::point_get_plan::FastQuery {
        table: crate::planbuilder::TableInfo {
            id: 1,
            db: "test".into(),
            name: "t".into(),
            columns: vec![column(1, "a", 0, true), column(2, "b", 1, true)],
            indices: vec![crate::planbuilder::IndexMeta {
                id: 1,
                name: "PRIMARY".into(),
                columns: vec![0, 1],
                prefix_lengths: vec![None, None],
                unique: true,
                global: false,
                invisible: false,
                multi_valued: false,
                vector: false,
            }],
            partitions: Vec::new(),
            common_handle: true,
            pk_is_handle: false,
            temporary: false,
        },
        alias: None,
        fields: vec![
            crate::point_get_plan::FastField {
                column: "a".into(),
                alias: None,
                row_checksum: false,
            },
            crate::point_get_plan::FastField {
                column: "b".into(),
                alias: None,
                row_checksum: false,
            },
        ],
        predicates: vec![crate::point_get_plan::Predicate::And(vec![
            crate::point_get_plan::Predicate::Eq("a".into(), crate::planbuilder::Value::Int(1)),
            crate::point_get_plan::Predicate::Eq("b".into(), crate::planbuilder::Value::Int(2)),
        ])],
        lock: lock.clone(),
        order_desc: false,
        limit: None,
        index_hints: Vec::new(),
        ignore_index_hints: Vec::new(),
    };
    let unique_query = crate::point_get_plan::FastQuery {
        table: crate::planbuilder::TableInfo {
            id: 2,
            db: "test".into(),
            name: "t1".into(),
            columns: vec![column(3, "c", 0, false), column(4, "d", 1, false)],
            indices: vec![crate::planbuilder::IndexMeta {
                id: 2,
                name: "c".into(),
                columns: vec![0],
                prefix_lengths: vec![None],
                unique: true,
                global: false,
                invisible: false,
                multi_valued: false,
                vector: false,
            }],
            partitions: Vec::new(),
            common_handle: false,
            pk_is_handle: false,
            temporary: false,
        },
        alias: None,
        fields: vec![
            crate::point_get_plan::FastField {
                column: "c".into(),
                alias: None,
                row_checksum: false,
            },
            crate::point_get_plan::FastField {
                column: "d".into(),
                alias: None,
                row_checksum: false,
            },
        ],
        predicates: Vec::new(),
        lock: lock.clone(),
        order_desc: false,
        limit: None,
        index_hints: Vec::new(),
        ignore_index_hints: Vec::new(),
    };
    let mut queries = Vec::new();
    let mut composite_or = point_query.clone();
    composite_or.predicates = vec![crate::point_get_plan::Predicate::Or(vec![
        crate::point_get_plan::Predicate::And(vec![
            crate::point_get_plan::Predicate::Eq("a".into(), crate::planbuilder::Value::Int(1)),
            crate::point_get_plan::Predicate::Eq("b".into(), crate::planbuilder::Value::Int(2)),
        ]),
        crate::point_get_plan::Predicate::And(vec![
            crate::point_get_plan::Predicate::Eq("a".into(), crate::planbuilder::Value::Int(2)),
            crate::point_get_plan::Predicate::Eq("b".into(), crate::planbuilder::Value::Int(1)),
        ]),
    ])];
    queries.push(composite_or);
    queries.push(point_query);

    let mut unique_eq = unique_query.clone();
    unique_eq.predicates = vec![crate::point_get_plan::Predicate::Eq(
        "c".into(),
        crate::planbuilder::Value::Int(1),
    )];
    queries.push(unique_eq);
    let mut unique_eq_with_filter = unique_query.clone();
    unique_eq_with_filter.predicates = vec![crate::point_get_plan::Predicate::And(vec![
        crate::point_get_plan::Predicate::Eq("c".into(), crate::planbuilder::Value::Int(1)),
        crate::point_get_plan::Predicate::Eq("d".into(), crate::planbuilder::Value::Int(1)),
    ])];
    queries.push(unique_eq_with_filter);
    let mut unique_or_with_filter = unique_query.clone();
    unique_or_with_filter.predicates = vec![crate::point_get_plan::Predicate::And(vec![
        crate::point_get_plan::Predicate::Or(vec![
            crate::point_get_plan::Predicate::Eq("c".into(), crate::planbuilder::Value::Int(1)),
            crate::point_get_plan::Predicate::Eq("c".into(), crate::planbuilder::Value::Int(2)),
        ]),
        crate::point_get_plan::Predicate::Eq("d".into(), crate::planbuilder::Value::Int(1)),
    ])];
    queries.push(unique_or_with_filter);
    let mut unique_in = unique_query;
    unique_in.predicates = vec![crate::point_get_plan::Predicate::In(
        "c".into(),
        vec![1, 2, 3, 4]
            .into_iter()
            .map(crate::planbuilder::Value::Int)
            .collect(),
    )];
    queries.push(unique_in);

    let plans = queries
        .iter()
        .map(|query| crate::point_get_plan::TryFastPlan(query, true, 50_000))
        .collect::<Vec<_>>();
    let kinds = plans
        .iter()
        .map(|plan| match plan {
            Some(crate::point_get_plan::FastPlan::Point(_)) => "PointGet",
            Some(crate::point_get_plan::FastPlan::Batch(_)) => "BatchPointGet",
            Some(_) => "Other",
            None => "None",
        })
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        [
            "BatchPointGet",
            "PointGet",
            "PointGet",
            "PointGet",
            "BatchPointGet",
            "BatchPointGet",
        ],
        "all six Go SELECT FOR UPDATE trees must use the fast point paths"
    );
    let Some(crate::point_get_plan::FastPlan::Batch(composite_batch)) = &plans[0] else {
        unreachable!("exact kind assertion already checked composite OR")
    };
    let integer_rows = |rows: &[Vec<crate::planbuilder::Value>]| {
        rows.iter()
            .map(|row| {
                row.iter()
                    .map(|value| match value {
                        crate::planbuilder::Value::Int(value) => *value,
                        other => panic!("point key must remain an integer, got {other:?}"),
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        integer_rows(&composite_batch.index_values),
        [vec![1, 2], vec![2, 1]]
    );
    let Some(crate::point_get_plan::FastPlan::Batch(filtered_batch)) = &plans[4] else {
        unreachable!("exact kind assertion already checked filtered unique-key OR")
    };
    assert_eq!(
        integer_rows(&filtered_batch.index_values),
        [vec![1], vec![2]]
    );
    assert_eq!(filtered_batch.access_conditions.len(), 1);
    assert_eq!(filtered_batch.access_conditions[0].name, "and");
    let Some(crate::point_get_plan::FastPlan::Batch(in_batch)) = &plans[5] else {
        unreachable!("exact kind assertion already checked unique-key IN")
    };
    assert_eq!(in_batch.index_values.len(), 4);
    for plan in plans {
        match plan.expect("all six SELECT FOR UPDATE queries need a fast plan") {
            crate::point_get_plan::FastPlan::Point(point) => {
                assert!(point.lock);
                assert_eq!(point.lock_wait_time, 50_000);
                assert!(!point.access_conditions.is_empty());
            }
            crate::point_get_plan::FastPlan::Batch(batch) => {
                assert!(batch.lock);
                assert_eq!(batch.lock_wait_time, 50_000);
                assert!(!batch.index_values.is_empty() || !batch.handles.is_empty());
                assert!(!batch.access_conditions.is_empty());
            }
            _ => panic!("SELECT FOR UPDATE must not become a DML fast plan"),
        }
    }
}

#[test]
fn index_lookup_only_embeds_limit_without_table_filters() {
    fn lookup(
        plan: &dyn base::PhysicalPlan,
    ) -> Option<&physicalop_dependency::PhysicalIndexLookUpReader> {
        plan.as_any()
            .downcast_ref::<physicalop_dependency::PhysicalIndexLookUpReader>()
            .or_else(|| plan.children().into_iter().find_map(lookup))
    }
    for table_filter in [false, true] {
        let context = integration_plan_context_with_params(
            &[kv_dependency::StoreType::TiKV],
            "tikv",
            false,
            false,
            Vec::new(),
            None,
        );
        let predicate = if table_filter {
            "a > 1 and b > 0"
        } else {
            "a > 1"
        };
        let mut plan = optimize_integration_query_with_schema(
            &format!("select c from t use index(idx_a) where {predicate} limit 2"),
            &context,
            integration_multi_info_schema(&["t"], false, Some(("idx_a", &["a"])), &["a", "b", "c"]),
        );
        plan.resolve_indices()
            .expect("LIMIT keeps the selected output schema");
        assert_eq!(plan.schema().Len(), 1);
        let reader = lookup(plan.as_ref()).expect("forced non-covering index lookup");
        assert_eq!(reader.PushedLimit.is_some(), !table_filter);
        if let Some(limit) = &reader.PushedLimit {
            assert_eq!((limit.Offset, limit.Count), (0, 2));
        }
    }
}

#[test]
fn ordered_table_limit_uses_expected_scan_count() {
    let plan = crate::main_test::optimize_query_for_test("select * from t order by a limit 1")
        .expect("optimize primary-key LIMIT");
    let mut types = Vec::new();
    physical_plan_types(plan.as_ref(), &mut types);
    assert!(types.iter().any(|kind| kind == "Limit"), "{types:?}");
    assert!(!types.iter().any(|kind| kind == "TopN"), "{types:?}");
    fn check_scan(plan: &dyn base::PhysicalPlan) -> bool {
        if let Some(scan) = plan
            .as_any()
            .downcast_ref::<physicalop_dependency::PhysicalTableScan>()
        {
            assert!(scan.KeepOrder);
            // Go AdjustRowCountForTableScanByLimit adds the configured
            // ordering selectivity ratio after the uniform LIMIT estimate.
            let expected = 1.0 + 9_999.0 * scan.s_ctx().GetSessionVars().OptOrderingIdxSelRatio;
            assert!((scan.stats_count() - expected).abs() < 1e-9);
            return true;
        }
        plan.children().into_iter().any(check_scan)
    }
    assert!(check_scan(plan.as_ref()));
}

#[test]
fn index_range_rebuild_keeps_non_index_predicates_on_table_side() {
    for quota in [None, Some(1_000_000)] {
        let context = integration_plan_context_with_params(
            &[kv_dependency::StoreType::TiKV],
            "tikv",
            false,
            false,
            Vec::new(),
            quota,
        );
        let mut plan = optimize_integration_query_with_schema(
            "select * from t use index(idx_a) where a = 1 and b is not null",
            &context,
            integration_multi_info_schema(&["t"], false, Some(("idx_a", &["a"])), &["a", "b", "c"]),
        );
        plan.resolve_indices()
            .expect("a table-only filter must not reference a missing index column");
        let scan = find_physical_index_scan(plan.as_ref()).expect("forced index scan");
        assert!(scan.FilterCondition.iter().all(|condition| {
            expression::ExtractColumns(condition.as_ref())
                .iter()
                .all(|column| scan.schema().Contains(column))
        }));
    }
}

#[test]
pub fn test_plan_cache_for_index_range_fallback() {
    for sql in [
        "select * from t where a in ('aa', 'bb', 'cc', 'dd', 'ee')",
        "select * from t where a in ('aaaaaaaaaa', 'bbbbbbbbbb', 'cccccccccc', 'dddddddddd', 'eeeeeeeeee')",
        "select * from t where a in (?, ?, ?, ?, ?)",
        "select * from t where a in (?, ?, ?, ?, ?) and b in (?, ?, ?, ?, ?)",
    ] {
        assert_sql_parses(sql);
    }
    let context = integration_plan_context_with_params(
        &[kv_dependency::StoreType::TiKV],
        "tikv",
        false,
        false,
        ["aa", "bb", "cc", "dd", "ee"]
            .into_iter()
            .map(|value| expression::types::NewStringDatum(value.to_owned()))
            .collect(),
        None,
    );
    let plan = optimize_integration_query_with_schema(
        "select * from t where a in (?, ?, ?, ?, ?)",
        &context,
        integration_multi_info_schema(
            &["t"],
            false,
            Some(("idx_a_b", &["a", "b"])),
            &["a", "b", "c"],
        ),
    );
    let mut types = Vec::new();
    physical_plan_types(plan.as_ref(), &mut types);
    assert!(
        types.iter().any(|kind| kind == "IndexScan"),
        "plan-cache parameter range must retain IndexScan, got {types:?}"
    );
    let scan = find_physical_index_scan(plan.as_ref()).expect("short IN list must use IndexScan");
    assert_eq!(
        scan.PlanCacheRangeString()
            .expect("rebuild short cached index ranges"),
        "[\"aa\",\"aa\"], [\"bb\",\"bb\"], [\"cc\",\"cc\"], [\"dd\",\"dd\"], [\"ee\",\"ee\"]"
    );

    let long_values = [
        "aaaaaaaaaa",
        "bbbbbbbbbb",
        "cccccccccc",
        "dddddddddd",
        "eeeeeeeeee",
    ];
    let long_context = integration_plan_context_with_params(
        &[kv_dependency::StoreType::TiKV],
        "tikv",
        false,
        false,
        long_values
            .into_iter()
            .map(|value| expression::types::NewStringDatum(value.to_owned()))
            .collect(),
        Some(1_330),
    );
    let long_plan = plan
        .clone_physical(long_context.clone())
        .expect("clone the cached short-range physical plan into long parameter context");
    let long_scan =
        find_physical_index_scan(long_plan.as_ref()).expect("cached rebuild must keep IndexScan");
    assert_eq!(
        long_scan
            .PlanCacheRangeString()
            .expect("rebuild long cached index ranges without quota"),
        "[\"aaaaaaaaaa\",\"aaaaaaaaaa\"], [\"bbbbbbbbbb\",\"bbbbbbbbbb\"], [\"cccccccccc\",\"cccccccccc\"], [\"dddddddddd\",\"dddddddddd\"], [\"eeeeeeeeee\",\"eeeeeeeeee\"]"
    );
}

#[test]
pub fn test_cor_col_range_with_range_max_size() {
    let info_schema = integration_multi_info_schema(
        &["t1", "t2", "t3"],
        false,
        Some(("idx_a_b", &["a", "b"])),
        &[],
    );
    let context = integration_plan_context_with_params(
        &[kv_dependency::StoreType::TiKV],
        "tikv",
        false,
        false,
        Vec::new(),
        Some(1_000),
    );
    let sql = "select * from t1 where exists (select /*+ NO_DECORRELATE() */ * from t2 where t2.a in (1, 3, 5) and b >= 2 and t2.b = t1.a)";
    let plan = optimize_integration_query_with_schema(sql, &context, info_schema.clone());
    let mut rows = Vec::new();
    physical_plan_diagnostics(plan.as_ref(), &mut rows);
    assert!(
        rows.iter()
            .any(|row| row.starts_with("Selection ") && row.contains("ge(test.t2.b, 2)")),
        "range fallback must leave b>=2 as Selection: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| {
            row.starts_with("IndexRangeScan ")
                && row.contains("decided by")
                && row.contains("in(test.t2.a, 1, 3, 5)")
                && row.contains("eq(test.t2.b, test.t1.a)")
        }),
        "correlated equality must remain an index access condition: {rows:?}"
    );
    let warnings = context.GetSessionVars().StmtCtx.GetWarnings();
    assert_eq!(warnings.len(), 1, "unexpected range warnings: {warnings:?}");
    assert_eq!(
        warnings[0].Err.as_ref().map(ToString::to_string).as_deref(),
        Some(
            "Memory capacity of 1000 bytes for 'tidb_opt_range_max_size' exceeded when building ranges. Less accurate ranges such as full range are chosen"
        )
    );

    let table_context = integration_plan_context_with_params(
        &[kv_dependency::StoreType::TiKV],
        "tikv",
        false,
        false,
        Vec::new(),
        Some(1),
    );
    let table_sql = "select * from t1 where exists (select /*+ NO_DECORRELATE() */ * from t3 where t3.a = t1.a)";
    let table_plan =
        optimize_integration_query_with_schema(table_sql, &table_context, info_schema.clone());
    let mut table_rows = Vec::new();
    physical_plan_diagnostics(table_plan.as_ref(), &mut table_rows);
    assert!(
        table_rows.iter().any(|row| {
            row.starts_with("TableRangeScan ") && row.contains("eq(test.t3.a, test.t1.a)")
        }),
        "correlated primary-key equality must be table access: {table_rows:?}"
    );
    assert!(
        table_context
            .GetSessionVars()
            .StmtCtx
            .GetWarnings()
            .is_empty(),
        "correlated table range must ignore the one-byte range quota"
    );
}

#[test]
pub fn test_cor_col_range_predicate_access() {
    let context = integration_plan_context(&[kv_dependency::StoreType::TiKV], "tikv", false, false);
    let info_schema =
        integration_multi_info_schema(&["t1", "t2"], false, Some(("idx_a_b", &["a", "b"])), &[]);
    for (comparison, function) in [("<", "lt("), (">", "gt("), ("<=", "le("), (">=", "ge(")] {
        let sql = format!(
            "select * from t1 t1a where exists (select /*+ NO_DECORRELATE() */ 1 from t1 t1b where t1b.b = t1a.b and t1b.a {comparison} t1a.a) order by t1a.a"
        );
        let plan = optimize_integration_query_with_schema(&sql, &context, info_schema.clone());
        let mut rows = Vec::new();
        physical_plan_diagnostics(plan.as_ref(), &mut rows);
        assert!(
            rows.iter().any(|row| {
                row.starts_with("IndexRangeScan ")
                    && row.contains("decided by")
                    && row.contains(function)
            }),
            "{comparison} correlated predicate must be index access: {rows:?}"
        );
    }
    let reverse_sql = "select * from t1 t1a where exists (select /*+ NO_DECORRELATE() */ 1 from t1 t1b where t1b.b = t1a.b and t1a.a < t1b.a)";
    let reverse_plan =
        optimize_integration_query_with_schema(reverse_sql, &context, info_schema.clone());
    let mut reverse_rows = Vec::new();
    physical_plan_diagnostics(reverse_plan.as_ref(), &mut reverse_rows);
    assert!(
        reverse_rows.iter().any(|row| {
            row.starts_with("IndexRangeScan ") && row.contains("decided by") && row.contains("gt(")
        }),
        "reversed LT predicate must become inner-column GT access: {reverse_rows:?}"
    );
    for (comparison, function) in [("<", "lt("), (">", "gt("), ("<=", "le("), (">=", "ge(")] {
        let sql = format!(
            "select * from t2 t2a where exists (select /*+ NO_DECORRELATE() */ 1 from t2 t2b where t2b.b = t2a.b and t2b.a {comparison} t2a.a) order by t2a.a"
        );
        let plan = optimize_integration_query_with_schema(&sql, &context, info_schema.clone());
        let mut rows = Vec::new();
        physical_plan_diagnostics(plan.as_ref(), &mut rows);
        assert!(
            rows.iter().any(|row| {
                row.starts_with("IndexRangeScan ")
                    && row.contains("decided by")
                    && row.contains(function)
            }),
            "t2 {comparison} correlated predicate must be index access: {rows:?}"
        );
    }
    assert!(context.GetSessionVars().StmtCtx.GetWarnings().is_empty());
}

#[test]
pub fn test_explain_analyze_dml_commit() {
    assert_sql_parses("explain analyze delete from t");
    let table = typed_table("t", &["c1", "c2"]);
    let mut builder = crate::planbuilder::NewPlanBuilder(&[]);
    let delete = builder
        .buildDelete(&crate::logical_plan_builder::DeleteStmt {
            source: typed_table_source(table.clone()),
            tables: vec![table.id],
            where_clause: None,
            order_by: Vec::new(),
            limit: None,
            ignore: false,
        })
        .expect("build delete child before EXPLAIN ANALYZE commit execution");
    assert!(matches!(delete.kind, crate::task::PlanKind::Other(ref name) if name == "Delete"));
}

#[test]
pub fn test_plan_cache_for_index_join_range_fallback() {
    for sql in [
        "select /*+ inl_join(t1) */ * from t1 join t2 on t1.a = t2.d where t1.b in ('a', 'b', 'c')",
        "select /*+ inl_join(t1) */ * from t1 join t2 on t1.a = t2.d where t1.b in ('aaaaaa', 'bbbbbb', 'cccccc')",
        "select /*+ inl_join(t1) */ * from t1 join t2 on t1.a = t2.d where t1.b in (?, ?, ?)",
        "select /*+ inl_join(t1) */ * from t1 join t2 on t1.a = t2.d where t1.b in (?, ?, ?, ?, ?)",
    ] {
        assert_sql_parses(sql);
    }
    let context = integration_plan_context_with_params(
        &[kv_dependency::StoreType::TiKV],
        "tikv",
        false,
        false,
        ["a", "b", "c"]
            .into_iter()
            .map(|value| expression::types::NewStringDatum(value.to_owned()))
            .collect(),
        Some(1_260),
    );
    let sql = "select /*+ inl_join(t1) */ * from t1 join t2 on t1.a = t2.d where t1.b in (?, ?, ?)";
    let plan = optimize_integration_query_with_schema(
        sql,
        &context,
        integration_multi_info_schema(
            &["t1", "t2"],
            false,
            Some(("idx_a_b", &["a", "b"])),
            &["b", "c"],
        ),
    );
    assert!(physical_plan_contains::<
        physicalop_dependency::PhysicalIndexJoin,
    >(plan.as_ref()));
    let index_join = find_physical_index_join(plan.as_ref()).expect("hint must choose IndexJoin");
    let inner_scan = index_join
        .InnerPlan
        .as_deref()
        .and_then(find_physical_index_scan)
        .expect("INL_JOIN inner path must contain IndexRangeScan");
    let inner_info = inner_scan.ExplainInfo();
    assert!(
        inner_info.contains("decided by")
            && inner_info.contains("eq(test.t1.a, test.t2.d)")
            && inner_info.contains("in(test.t1.b, a, b, c)"),
        "short INL_JOIN ranges must match the Go access conditions: {inner_info}"
    );
    let warnings = context.GetSessionVars().StmtCtx.GetWarnings();
    assert!(warnings.is_empty(), "warnings: {warnings:?}");
}

#[test]
pub fn test_is_i_pv4_to_ti_flash() {
    assert_builtin_contract("is_ipv4", &[1], &[0, 2], &[("IsIPv4", "is_ipv4")]);
}
#[test]
pub fn test_is_i_pv6_to_ti_flash() {
    assert_builtin_contract("is_ipv6", &[1], &[0, 2], &[("IsIPv6", "is_ipv6")]);
}
#[test]
pub fn test_virtual_expr_push_down() {
    for sql in [
        "select * from t order by c2 limit 2",
        "select c1 + c2 from t",
        "select * from t where c2 > 1",
        "select g from t_force_idx force index (idx_exp_i) where (i + 1) >= 1 order by g limit 1",
        "select /* issue:67981 */ * from t_bug force index (idx_exp_lower) where lower(s) >= 'a' order by g limit 1",
        "select /* issue:67981 */ * from t_bug force index (g) where lower(s) >= 'a' order by g limit 1",
    ] {
        assert_sql_parses(sql);
    }
    assert_pushdown_contract(
        "abs",
        "AbsInt",
        1,
        expression::infer_pushdown::StoreType::TiKV,
    );
}

// —— 窗口函数、虚拟列与历史 issue 回归 ——
#[test]
pub fn test_window_range_frame_push_down_tiflash() {
    let sqls = [
        "select *, first_value(v) over (partition by p order by o range between 3 preceding and 0 following) as a from test.first_range",
        "select *, first_value(v) over (partition by p order by o range between 3 preceding and 2.9E0 following) as a from test.first_range",
        "select *, first_value(v) over (partition by p order by o range between 2.3 preceding and 0 following) as a from test.first_range_d64",
        "select *, first_value(v) over (partition by p order by o_datetime range between interval 1 day preceding and interval 1 day following) as a from test.first_range",
        "select *, first_value(v) over (partition by p order by o_time range between interval 1 day preceding and interval 1 day following) as a from test.first_range",
    ];
    for sql in sqls {
        assert_sql_parses(sql);
    }
    let context =
        integration_plan_context(&[kv_dependency::StoreType::TiFlash], "tiflash", false, true);
    let info_schema =
        integration_multi_info_schema(&["first_range", "first_range_d64"], true, None, &[]);
    for sql in sqls {
        let plan = optimize_integration_query_with_schema(sql, &context, info_schema.clone());
        assert!(
            physical_plan_contains::<physicalop_dependency::PhysicalWindow>(plan.as_ref()),
            "RANGE frame must retain Window for {sql:?}"
        );
        assert!(
            physical_plan_contains::<physicalop_dependency::PhysicalTableScan>(plan.as_ref()),
            "RANGE frame must reach TiFlash TableFullScan for {sql:?}"
        );
        assert_eq!(
            find_physical_window(plan.as_ref())
                .expect("RANGE frame physical window")
                .StoreTp,
            kv_dependency::StoreType::TiFlash,
            "RANGE frame window must execute in TiFlash MPP for {sql:?}"
        );
    }
}

#[test]
pub fn test_issue41458() {
    let sql = "select * from t t1 join t t2 on t1.b = t2.b join t t3 on t2.b=t3.b join t t4 on t3.b=t4.b where t3.a=1 and t2.a=2";
    assert_sql_parses(sql);
    let context = integration_plan_context(&[kv_dependency::StoreType::TiKV], "tikv", false, false);
    let plan = optimize_integration_query_with_schema(
        sql,
        &context,
        integration_multi_info_schema(&["t"], false, Some(("ia", &["a"])), &[]),
    );
    let mut exact_path = Vec::new();
    physical_plan_exact_types(plan.as_ref(), &mut exact_path);
    assert_eq!(
        exact_path,
        [
            "Projection",
            "HashJoin",
            "HashJoin",
            "HashJoin",
            "IndexLookUp",
            "IndexRangeScan",
            "Selection",
            "TableRowIDScan",
            "IndexLookUp",
            "IndexRangeScan",
            "Selection",
            "TableRowIDScan",
            "TableReader",
            "Selection",
            "TableFullScan",
            "TableReader",
            "Selection",
            "TableFullScan",
        ],
        "issue 41458 statement-summary plan must preserve all 18 Go operators"
    );
}

#[test]
pub fn test_issue48257() {
    let context = integration_plan_context(&[kv_dependency::StoreType::TiKV], "tikv", false, false);
    let cases = [
        ("t", 1.0, false),
        ("t", 2.0, false),
        ("t", 1.0, false),
        ("t1", 1.0, true),
        ("t1", 2.0, true),
        ("t1", 10_000.0, true),
        ("t1", 1.0, false),
    ];
    for (table, expected_rows, pseudo) in cases {
        let sql = format!("select * from {table}");
        let plan = optimize_integration_query_with_schema_and_stats(
            &sql,
            &context,
            integration_info_schema(false, false, table, false, false),
            expected_rows,
            pseudo,
        );
        let mut exact_path = Vec::new();
        physical_plan_exact_types(plan.as_ref(), &mut exact_path);
        assert_eq!(plan.stats_count(), expected_rows);
        let reader = plan
            .as_any()
            .downcast_ref::<physicalop_dependency::PhysicalTableReader>()
            .or_else(|| {
                plan.children().into_iter().find_map(|child| {
                    child
                        .as_any()
                        .downcast_ref::<physicalop_dependency::PhysicalTableReader>()
                })
            })
            .expect("plan must contain TableReader");
        let scan = reader
            .children()
            .first()
            .copied()
            .and_then(|child| {
                child
                    .as_any()
                    .downcast_ref::<physicalop_dependency::PhysicalTableScan>()
            })
            .expect("TableReader child must be TableFullScan");
        assert_eq!(scan.stats_count(), expected_rows);
        assert_eq!(
            base::Plan::stats_info(scan).StatsVersion == 0,
            pseudo,
            "stats:pseudo transition mismatch for {table} at {expected_rows} rows"
        );
        assert_eq!(exact_path, ["TableReader", "TableFullScan"]);
    }
}

#[test]
pub fn test_issue54213() {
    assert_sql_parses(
        "select count(1) from (select /*+ force_index(tb, ab) */ 1 from tb where a=1 and b=1 limit 100) a",
    );
    let int_type = crate::task::FieldType {
        code: crate::task::TypeCode::Int,
        flen: 20,
        decimal: 0,
        unsigned: false,
    };
    let mut table = typed_table("tb", &["object_id", "a", "b", "c"]);
    table.indices.push(crate::planbuilder::IndexMeta {
        id: 2,
        name: "ab".into(),
        columns: vec![1, 2],
        prefix_lengths: vec![None, None],
        unique: false,
        global: false,
        invisible: false,
        multi_valued: false,
        vector: false,
    });
    let inner = crate::logical_plan_builder::SelectStmt {
        from: Some(typed_table_source(table)),
        fields: vec![crate::logical_plan_builder::SelectField {
            expr: crate::task::Expression {
                name: "1".into(),
                return_type: Some(int_type.clone()),
                ..Default::default()
            },
            alias: None,
            wildcard: false,
            table_wildcard: None,
        }],
        where_clause: Some(crate::task::Expression {
            name: "a=1 and b=1".into(),
            ..Default::default()
        }),
        limit: Some(crate::logical_plan_builder::LimitClause {
            offset: None,
            count: Some(crate::planbuilder::Value::UInt(100)),
        }),
        ..Default::default()
    };
    let outer = crate::logical_plan_builder::SelectStmt {
        from: Some(crate::logical_plan_builder::ResultSet::Select(Box::new(
            inner,
        ))),
        fields: vec![crate::logical_plan_builder::SelectField {
            expr: crate::task::Expression {
                name: "agg:count".into(),
                return_type: Some(int_type),
                ..Default::default()
            },
            alias: None,
            wildcard: false,
            table_wildcard: None,
        }],
        ..Default::default()
    };
    let mut builder = crate::planbuilder::NewPlanBuilder(&[]);
    let plan = builder
        .buildSelect(&outer)
        .expect("build issue 54213 typed plan");
    let mut exact_path = Vec::new();
    typed_plan_kinds(&plan, &mut exact_path);
    assert_eq!(
        exact_path,
        [
            "Projection",
            "HashAgg",
            "Limit",
            "Projection",
            "Selection",
            "IndexScan",
        ]
    );

    let sql = "select count(1) from (select /*+ force_index(tb, ab) */ 1 from tb where a=1 and b=1 limit 100) a";
    let context = integration_plan_context(&[kv_dependency::StoreType::TiKV], "tikv", false, false);
    let physical = optimize_integration_query_with_schema(
        sql,
        &context,
        integration_multi_info_schema(&["tb"], false, Some(("ab", &["a", "b"])), &[]),
    );
    let mut physical_path = Vec::new();
    physical_plan_exact_types(physical.as_ref(), &mut physical_path);
    assert_eq!(
        physical_path,
        [
            "StreamAgg",
            "Limit",
            "IndexReader",
            "Limit",
            "IndexRangeScan",
        ],
        "issue 54213 must retain the exact Go force-index path"
    );
}

#[test]
pub fn test_issue54870() {
    assert_sql_parses(
        "create table t (id int, deleted_at datetime(3) not null default '1970-01-01 01:00:01.000', is_deleted tinyint(1) generated always as ((deleted_at > _utf8mb4'1970-01-01 01:00:01.000')) virtual not null, key k(id, is_deleted))",
    );
    assert_sql_parses("insert into t (id, deleted_at) values (1, now())");
    assert_sql_parses("select 1 from t where id=1 and is_deleted=true");
    let mut table = typed_table("t", &["id", "deleted_at", "is_deleted"]);
    table.columns[2].generated = true;
    table.columns[2].stored = false;
    table.indices.push(crate::planbuilder::IndexMeta {
        id: 8,
        name: "k".into(),
        columns: vec![0, 2],
        prefix_lengths: vec![None, None],
        unique: false,
        global: false,
        invisible: false,
        multi_valued: false,
        vector: false,
    });
    let select = crate::logical_plan_builder::SelectStmt {
        from: Some(typed_table_source(table)),
        fields: vec![crate::logical_plan_builder::SelectField {
            expr: crate::task::Expression {
                name: "1".into(),
                return_type: Some(crate::task::FieldType {
                    code: crate::task::TypeCode::Int,
                    flen: 1,
                    decimal: 0,
                    unsigned: false,
                }),
                ..Default::default()
            },
            alias: None,
            wildcard: false,
            table_wildcard: None,
        }],
        where_clause: Some(crate::task::Expression {
            name: "id=1 and is_deleted=true".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut builder = crate::planbuilder::NewPlanBuilder(&[]);
    let plan = builder
        .buildSelect(&select)
        .expect("build issue 54870 generated-index select");
    let mut exact_path = Vec::new();
    typed_plan_kinds(&plan, &mut exact_path);
    assert_eq!(
        exact_path,
        ["Projection", "Selection", "IndexScan"],
        "issue 54870 must use k(id,is_deleted), not a full scan"
    );
}

#[test]
pub fn test_issue52472() {
    let sqls = [
        "select c1 from t1 union all select c1 from t2",
        "select 0 union all select c1 from t3",
    ];
    for sql in sqls {
        assert_sql_parses(sql);
    }
    let table = |id: i64, name: &str, tp, unsigned| {
        let mut field_type = *expression::types::NewFieldType(tp);
        if unsigned {
            field_type.AddFlag(expression::mysql::UnsignedFlag);
        }
        let model = Arc::new(expression::model::TableInfo {
            ID: id,
            Name: crate::ast::NewCIStr(name),
            Columns: vec![expression::model::ColumnInfo {
                ID: id * 100 + 1,
                Name: crate::ast::NewCIStr("c1"),
                Offset: 0,
                State: expression::model::StatePublic,
                FieldType: field_type,
                ..Default::default()
            }],
            ..Default::default()
        });
        infoschema_dependency::infoschema::TableInfo {
            id,
            name: infoschema_dependency::infoschema::CiString::new(name),
            columns: vec![infoschema_dependency::infoschema::ColumnInfo {
                id: model.Columns[0].ID,
                name: infoschema_dependency::infoschema::CiString::new("c1"),
                ..Default::default()
            }],
            model_meta: Some(model),
            ..Default::default()
        }
    };
    let info_schema = infoschema_dependency::infoschema::MockInfoSchema(vec![
        table(524_721, "t1", expression::mysql::TypeLong, false),
        table(524_722, "t2", expression::mysql::TypeLong, true),
        table(524_723, "t3", expression::mysql::TypeLonglong, true),
    ]);
    let context = integration_plan_context(&[kv_dependency::StoreType::TiKV], "tikv", false, false);
    let int_union = optimize_integration_query_with_schema(sqls[0], &context, info_schema.clone());
    assert_eq!(
        int_union.schema().Columns[0]
            .RetType
            .as_ref()
            .expect("UNION output must have a field type")
            .GetType(),
        expression::mysql::TypeLonglong,
        "signed INT UNION unsigned INT must promote to BIGINT"
    );
    let decimal_union = optimize_integration_query_with_schema(sqls[1], &context, info_schema);
    assert_eq!(
        decimal_union.schema().Columns[0]
            .RetType
            .as_ref()
            .expect("UNION output must have a field type")
            .GetType(),
        expression::mysql::TypeNewDecimal,
        "integer literal UNION unsigned BIGINT must promote to DECIMAL"
    );

    let select = |name: &str, field_type: crate::task::FieldType| {
        (
            crate::logical_plan_builder::SetOprType::UnionAll,
            crate::logical_plan_builder::SelectStmt {
                fields: vec![crate::logical_plan_builder::SelectField {
                    expr: crate::task::Expression {
                        name: name.into(),
                        return_type: Some(field_type),
                        ..Default::default()
                    },
                    alias: None,
                    wildcard: false,
                    table_wildcard: None,
                }],
                ..Default::default()
            },
        )
    };
    let signed_int = crate::task::FieldType {
        code: crate::task::TypeCode::Int,
        flen: 11,
        decimal: 0,
        unsigned: false,
    };
    let unsigned_int = crate::task::FieldType {
        code: crate::task::TypeCode::UInt,
        flen: 11,
        decimal: 0,
        unsigned: true,
    };
    let unsigned_bigint = crate::task::FieldType {
        code: crate::task::TypeCode::UInt,
        flen: 20,
        decimal: 0,
        unsigned: true,
    };
    let mut builder = crate::planbuilder::NewPlanBuilder(&[]);
    let typed_int_union = builder
        .buildResultSetNode(
            &crate::logical_plan_builder::ResultSet::SetOperation(vec![
                select("t1.c1", signed_int.clone()),
                select("t2.c1", unsigned_int),
            ]),
            false,
        )
        .expect("build signed/unsigned integer UNION ALL");
    assert_eq!(typed_int_union.schema[0].code, crate::task::TypeCode::Int);
    assert_eq!(typed_int_union.schema[0].flen, 20);

    let typed_decimal_union = builder
        .buildResultSetNode(
            &crate::logical_plan_builder::ResultSet::SetOperation(vec![
                select("literal_0", signed_int),
                select("t3.c1", unsigned_bigint),
            ]),
            false,
        )
        .expect("build integer/unsigned-bigint UNION ALL");
    assert_eq!(
        typed_decimal_union.schema[0].code,
        crate::task::TypeCode::Decimal
    );
}

#[test]
pub fn test_ti_flash_hash_agg_pre_agg_mode() {
    let mut vars = variable_dependency::session::SessionVars::default();
    assert_eq!(
        vars.GetSystemVar(vardef_dependency::TiFlashHashAggPreAggMode),
        Some(vardef_dependency::DefTiFlashPreAggMode.to_owned()),
    );
    for value in ["auto", "force_streaming", "force_preagg"] {
        vars.SetSystemVar(vardef_dependency::TiFlashHashAggPreAggMode, value)
            .unwrap_or_else(|error| {
                panic!("set TiFlash hashagg preaggregation mode {value}: {error}")
            });
        assert_eq!(
            vars.GetSystemVar(vardef_dependency::TiFlashHashAggPreAggMode),
            Some(value.to_owned()),
        );
    }
    assert!(
        vars.SetSystemVar(vardef_dependency::TiFlashHashAggPreAggMode, "test")
            .is_err(),
        "invalid TiFlash hashagg preaggregation mode must be rejected"
    );
}

#[test]
pub fn test_nested_virtual_generated_column_update() {
    assert_sql_parses(
        r#"create table test1 (
            col1 bigint(20) not null, col2 varchar(36) not null, col3 int(11) default null,
            col4 varchar(36) not null, col5 varchar(255) default null,
            modify_time bigint(20) default null, create_time bigint(20) default null,
            col6 json default null, col7 json default null,
            col8 json generated always as (json_merge_patch(ifnull(col6, _utf8mb4"{}"), ifnull(col7, _utf8mb4"{}"))) stored,
            col9 varchar(36) generated always as (left(json_unquote(json_extract(col8, _utf8mb4"$.col9[0]")), 36)) virtual,
            col10 varchar(30) generated always as (left(json_unquote(json_extract(col8, _utf8mb4"$.col10")), 30)) virtual,
            key dev_idx1 (col10)) engine=InnoDB default charset=utf8 collate=utf8_bin"#,
    );
    assert_sql_parses(
        r#"insert into test1 values (-100000000, "123459789332", 1, "123459789332", "BBBBB", 1675871896, 1675871896, '{"col10": "CCCCC","col9": ["ABCDEFG"]}', null, default, default, default)"#,
    );
    assert_sql_parses(r#"update test1 set col7 = '{"col10":"DDDDD","col9":["abcdefg"]}'"#);
    assert_sql_parses("delete from test1 where col1 < 0");
    let mut table = typed_table(
        "test1",
        &[
            "col1",
            "col2",
            "col3",
            "col4",
            "col5",
            "modify_time",
            "create_time",
            "col6",
            "col7",
            "col8",
            "col9",
            "col10",
        ],
    );
    for offset in [1, 3, 4, 7, 8, 9, 10, 11] {
        let column = &mut table.columns[offset];
        column.field_type.code = crate::task::TypeCode::String;
        column.field_type.flen = 255;
    }
    for (offset, stored) in [(9, true), (10, false), (11, false)] {
        table.columns[offset].generated = true;
        table.columns[offset].stored = stored;
    }
    let mut builder = crate::planbuilder::NewPlanBuilder(&[]);
    let insert = builder
        .Build(&crate::planbuilder::Statement::Insert(
            crate::planbuilder::InsertStatement {
                table: table.clone(),
                columns: Vec::new(),
                values: vec![vec![
                    crate::planbuilder::Value::Int(-100_000_000),
                    crate::planbuilder::Value::String("123459789332".into()),
                    crate::planbuilder::Value::Int(1),
                    crate::planbuilder::Value::String("123459789332".into()),
                    crate::planbuilder::Value::String("BBBBB".into()),
                    crate::planbuilder::Value::Int(1_675_871_896),
                    crate::planbuilder::Value::Int(1_675_871_896),
                    crate::planbuilder::Value::String(
                        r#"{"col10": "CCCCC","col9": ["ABCDEFG"]}"#.into(),
                    ),
                    crate::planbuilder::Value::Null,
                    crate::planbuilder::Value::Default,
                    crate::planbuilder::Value::Default,
                    crate::planbuilder::Value::Default,
                ]],
                select: None,
                on_duplicate: Vec::new(),
                replace: false,
            },
        ))
        .expect("build nested generated-column insert");
    let crate::planbuilder::BuiltPlan::Insert { generated, .. } = insert else {
        panic!("generated-column insert must retain generated expressions")
    };
    assert_eq!(generated.len(), 3);
    assert!(generated[0].0.stored);
    assert!(generated[1..].iter().all(|(column, _)| !column.stored));

    let update = builder
        .buildUpdate(&crate::logical_plan_builder::UpdateStmt {
            source: typed_table_source(table.clone()),
            assignments: vec![crate::logical_plan_builder::Assignment {
                table_id: table.id,
                column: 8,
                expression: crate::task::Expression {
                    name: "json-col7".into(),
                    ..Default::default()
                },
            }],
            where_clause: None,
            order_by: Vec::new(),
            limit: None,
            ignore: false,
        })
        .expect("build nested generated-column update");
    assert!(matches!(update.kind, crate::task::PlanKind::Other(ref name) if name == "Update"));
    let delete = builder
        .buildDelete(&crate::logical_plan_builder::DeleteStmt {
            source: typed_table_source(table.clone()),
            tables: vec![table.id],
            where_clause: Some(crate::task::Expression {
                name: "col1 < 0".into(),
                ..Default::default()
            }),
            order_by: Vec::new(),
            limit: None,
            ignore: false,
        })
        .expect("build nested generated-column delete");
    assert!(matches!(delete.kind, crate::task::PlanKind::Other(ref name) if name == "Delete"));
}

#[test]
pub fn test_aggregation_in_window_function_push_down_to_ti_flash() {
    let sql = "select sum(v) over w as res1, count(v) over w as res2, avg(v) over w as res3, min(v) over w as res4, max(v) over w as res5 from t window w as (partition by p order by o)";
    let context =
        integration_plan_context(&[kv_dependency::StoreType::TiFlash], "tiflash", false, true);
    let plan = optimize_integration_query(sql, &context, true, false);
    let mut exact_path = Vec::new();
    physical_plan_exact_types(plan.as_ref(), &mut exact_path);
    assert_eq!(
        exact_path,
        [
            "TableReader",
            "ExchangeSender",
            "Projection",
            "Window",
            "Sort",
            "ExchangeReceiver",
            "ExchangeSender",
            "TableFullScan",
        ],
        "window aggregation pushdown must preserve the complete Go MPP plan tree"
    );
    for function in ["sum", "count", "avg", "min", "max"] {
        let query =
            format!("select {function}(v) over w from t window w as (partition by p order by o)");
        let plan = optimize_integration_query(&query, &context, true, false);
        assert!(physical_plan_contains::<
            physicalop_dependency::PhysicalWindow,
        >(plan.as_ref()));
        assert!(physical_plan_contains::<
            physicalop_dependency::PhysicalTableScan,
        >(plan.as_ref()));
    }
}

#[test]
pub fn test_correlated_scalar_subquery() {
    let sql = r#"select
            (select count(t3.user_id) from t2
              left join t1 as cm on t2.cid = cm.id
              left join t3 on t2.bid = t3.id
             where cm.hcode = t1.hcode) as tt
           from t1
           where t1.id in (select min(id) from t1)"#;
    assert_sql_parses(sql);
    let context = integration_plan_context(&[kv_dependency::StoreType::TiKV], "tikv", false, false);
    let mut plan = optimize_integration_query_with_schema(
        sql,
        &context,
        integration_multi_info_schema(&["t1", "t2", "t3"], false, None, &[]),
    );
    // Go checks that this query does not lose the COUNT input during column
    // pruning. A decorrelated join is valid; requiring Apply rejects it.
    plan.resolve_indices()
        .expect("correlated COUNT input must resolve after optimization");
    assert_eq!(
        plan.schema().Len(),
        1,
        "scalar COUNT produces one output column"
    );
}

#[test]
pub fn test_issue66619() {
    let sqls = [
        "select a, (select count(1) as cnt from t2 where t2.a = t1.a having cnt > 0) from t1 order by a",
        "select a, (select /*+ NO_DECORRELATE() */ count(1) as cnt from t2 where t2.a = t1.a having cnt > 0) from t1 order by a",
        "select a, (select count(1) as cnt from t2 where t2.a = t1.a having cnt < 1) from t1 order by a",
        "select a, (select count(1) from t2 where t2.a = t1.a) from t1 order by a",
        "select /* issue:66947 direct-having */ hex(t0.c0) from t0 group by t0.c0 having sum(t0.c0) > -1 and char_length(t0.c0)",
        "select /* issue:66947 derived-filter */ hex(ref0) from (select t0.c0 as ref0, (sum(t0.c0) > -1 and char_length(t0.c0)) as ref1 from t0 group by t0.c0) as s where ref1",
        "select /* issue:67237 not-null */ c1 from t1 where ifnull(c1, '') = c1",
        "select /* issue:67237 nullable */ c1 from t1 where ifnull(c1, '') = c1",
        r#"select /* issue:66922-direct */ t0.c1, t0.c0, t0.c2
           from t0 group by t0.c1, t0.c0, t0.c2
           having (count(t0.c1) != -1)
             and ((case t0.c1 when false then t0.c0 else true end) like t0.c0)"#,
        r#"select /* issue:66922-derived */ ref0, ref1, ref2
           from (
             select t0.c1 as ref0, t0.c0 as ref1, t0.c2 as ref2,
               ((count(t0.c1) != -1)
                 and ((case t0.c1 when false then t0.c0 else true end) like t0.c0)) as ref3
             from t0 group by t0.c1, t0.c0, t0.c2
           ) as s where ref3"#,
    ];
    for sql in sqls {
        assert_sql_parses(sql);
    }
    let context = integration_plan_context(&[kv_dependency::StoreType::TiKV], "tikv", false, false);
    let info_schema = integration_multi_info_schema(&["t0", "t1", "t2"], false, None, &[]);
    for sql in sqls {
        optimize_integration_query_with_schema(sql, &context, info_schema.clone());
    }
    let int_type = crate::task::FieldType {
        code: crate::task::TypeCode::Int,
        flen: 20,
        decimal: 0,
        unsigned: false,
    };
    let expr = |name: &str, column: Option<usize>| crate::task::Expression {
        name: name.into(),
        column,
        return_type: Some(int_type.clone()),
        ..Default::default()
    };
    let field = |expression| crate::logical_plan_builder::SelectField {
        expr: expression,
        alias: None,
        wildcard: false,
        table_wildcard: None,
    };
    let mut builder = crate::planbuilder::NewPlanBuilder(&[]);
    let correlated_having = crate::logical_plan_builder::SelectStmt {
        from: Some(typed_table_source(typed_table("t2", &["a"]))),
        fields: vec![field(expr("agg:count", None))],
        where_clause: Some(expr("t2.a=t1.a", None)),
        having: Some(expr("cnt>0", None)),
        ..Default::default()
    };
    let subquery_plan = builder
        .buildSelect(&correlated_having)
        .expect("build correlated scalar aggregate with HAVING");
    assert!(typed_plan_contains(&subquery_plan, &|kind| matches!(
        kind,
        crate::task::PlanKind::HashAgg
    )));
    assert!(typed_plan_contains(&subquery_plan, &|kind| matches!(
        kind,
        crate::task::PlanKind::Selection
    )));

    let direct_having = crate::logical_plan_builder::SelectStmt {
        from: Some(typed_table_source(typed_table("t0", &["c0"]))),
        fields: vec![field(expr("hex(c0)", Some(0)))],
        group_by: vec![expr("c0", Some(0))],
        having: Some(expr("sum(c0)>-1 and char_length(c0)", None)),
        ..Default::default()
    };
    let direct = builder
        .buildSelect(&direct_having)
        .expect("build issue 66947 direct HAVING");
    assert!(typed_plan_contains(&direct, &|kind| matches!(
        kind,
        crate::task::PlanKind::HashAgg
    )));
    assert!(typed_plan_contains(&direct, &|kind| matches!(
        kind,
        crate::task::PlanKind::Selection
    )));

    for nullable in [false, true] {
        let mut table = typed_table("t1", &["c1"]);
        table.columns[0].field_type.code = crate::task::TypeCode::Decimal;
        let ifnull = crate::logical_plan_builder::SelectStmt {
            from: Some(typed_table_source(table)),
            fields: vec![field(expr("c1", Some(0)))],
            where_clause: Some(expr(
                if nullable {
                    "ifnull_nullable(c1,'')=c1"
                } else {
                    "ifnull_not_null(c1,'')=c1"
                },
                None,
            )),
            ..Default::default()
        };
        let plan = builder
            .buildSelect(&ifnull)
            .expect("build issue 67237 IFNULL predicate");
        assert!(typed_plan_contains(&plan, &|kind| matches!(
            kind,
            crate::task::PlanKind::Selection
        )));
    }
}
