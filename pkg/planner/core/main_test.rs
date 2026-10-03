// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 规划器 core 测试运行时与 typed JSON fixture 基础设施。
//
// 对应 Go `TestMain`：构造隔离的 PlanContext、Mock InfoSchema、统计填充，
// 提供 parse/build/logical-optimize/physical-optimize 入口，并加载
// `testdata/plan_suite_unexported_*.json` 等套件。

// Go's TestMain loads these suites once. Rust tests preserve the typed fixture
// 契约，并为每个用例构造隔离的规划器上下文。
// contract while constructing an isolated planner context per case.

use base_dependency as base;
use expression_dependency as expression;
use logicalop_dependency as logicalop;
use logicalop_dependency::LogicalPlan as _;
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

/// 测试用统计加载等待句柄：可模拟成功完成或超时。
pub(crate) struct PlannerTestStatsHandle {
    calls: AtomicUsize,
    receiver: Mutex<mpsc::Receiver<Result<(), String>>>,
    _sender: Option<mpsc::Sender<Result<(), String>>>,
    delay_before_wait: Duration,
}

impl PlannerTestStatsHandle {
    /// 后台短暂延迟后发送成功结果。
    pub(crate) fn success() -> Arc<Self> {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1));
            sender.send(Ok(())).expect("complete stats-load success");
        });
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            receiver: Mutex::new(receiver),
            _sender: None,
            delay_before_wait: Duration::from_millis(1),
        })
    }

    /// 永不完成的等待，用于超时路径。
    pub(crate) fn timeout() -> Arc<Self> {
        let (sender, receiver) = mpsc::channel();
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            receiver: Mutex::new(receiver),
            _sender: Some(sender),
            delay_before_wait: Duration::ZERO,
        })
    }

    /// 已调用 `SyncWaitStatsLoad` 的次数。
    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::Acquire)
    }
}

impl base::StatsLoadWaiter for PlannerTestStatsHandle {
    /// 同步等待语句上下文中挂起的统计加载项。
    fn SyncWaitStatsLoad(
        &self,
        session_vars: &variable_dependency::session::SessionVars,
    ) -> Result<(), String> {
        let pending_items = session_vars.StmtCtx.PendingStatsLoadItems();
        assert!(pending_items > 0);
        self.calls.fetch_add(1, Ordering::AcqRel);
        std::thread::sleep(self.delay_before_wait);
        self.receiver
            .lock()
            .expect("stats-load receiver lock")
            .recv_timeout(Duration::from_millis(5))
            .map_err(|_| "synchronous statistics wait timed out".to_owned())?
    }
}

/// 测试用 PlanContext：会话变量、表达式上下文、Ranger 与可选统计等待器。
struct PlannerTestContext {
    session_vars: Arc<variable_dependency::session::SessionVars>,
    expr_ctx: exprstatic_dependency::ExprContext,
    ranger_ctx: base::RangerContext<'static>,
    builtin_usage: base::BuiltinFunctionUsageCounter,
    stats_load_waiter: Option<Arc<dyn base::StatsLoadWaiter>>,
}

impl base::PlanContext for PlannerTestContext {
    fn alloc_plan_id(&self) -> i32 {
        self.session_vars.AllocNewPlanID()
    }

    fn plan_id_checkpoint(&self) -> Option<i32> {
        Some(self.session_vars.PlanID.load(Ordering::SeqCst))
    }

    fn restore_plan_id_checkpoint(&self, checkpoint: i32) {
        self.session_vars.PlanID.store(checkpoint, Ordering::SeqCst);
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &variable_dependency::session::SessionVars {
        self.session_vars.as_ref()
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
        panic!("planner unit tests do not build protobuf executors")
    }

    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.builtin_usage.Inc(name)
    }

    fn GetStatsLoadWaiter(&self) -> Option<&dyn base::StatsLoadWaiter> {
        self.stats_load_waiter.as_deref()
    }
}

/// 默认测试上下文（无挂起统计项）。
fn planner_test_context() -> base::ContextRef {
    planner_test_context_with_stats_state(0, false, None, false, |_| {})
}

/// 构造带挂起统计项 / 同步失败标志 / 等待器的测试上下文。
fn planner_test_context_with_stats_state(
    pending_items: usize,
    sync_failed: bool,
    stats_load_waiter: Option<Arc<dyn base::StatsLoadWaiter>>,
    enable_agg_pushdown: bool,
    configure: impl FnOnce(&mut variable_dependency::session::SessionVars),
) -> base::ContextRef {
    logicalop::InstallPredicateSimplificationPassthrough();
    let mut session_vars = variable_dependency::session::SessionVars::default();
    session_vars.SetCurrentDB("test");
    *session_vars
        .StmtCtx
        .StatsLoad
        .NeededItems
        .lock()
        .expect("stats-load items lock") = (0..pending_items)
        .map(|_| Arc::new(()) as Arc<dyn std::any::Any + Send + Sync>)
        .collect();
    session_vars
        .StmtCtx
        .IsSyncStatsFailed
        .store(sync_failed, Ordering::Release);
    session_vars
        .SetSystemVar(vardef_dependency::TiDBIsolationReadEngines, "tikv")
        .expect("set planner-test isolation engine");
    if enable_agg_pushdown {
        session_vars
            .SetSystemVar(vardef_dependency::TiDBOptAggPushDown, "1")
            .expect("enable aggregate pushdown for Go eager-aggregation fixture");
    }
    session_vars.IsolationReadEngines = HashSet::from([kv_dependency::StoreType::TiKV]);
    configure(&mut session_vars);
    let session_vars = Arc::new(session_vars);
    let eval_ctx =
        exprstatic_dependency::NewEvalContext(vec![exprstatic_dependency::WithOptionalProperty(
            vec![Box::new(
                expression_expropt_dependency::SessionVarsPropProvider::new(Arc::clone(
                    &session_vars,
                )),
            )],
        )]);
    let expr_ctx = exprstatic_dependency::NewExprContext(vec![
        exprstatic_dependency::WithEvalCtx(Arc::new(eval_ctx)),
        exprstatic_dependency::WithColumnIDAllocator(Arc::new(
            expression::exprctx::NewSimplePlanColumnIDAllocator(0),
        )),
    ]);
    let ranger_expr: Arc<dyn expression::exprctx::BuildContext> =
        Arc::new(exprstatic_dependency::NewExprContext(Vec::new()));
    Arc::new(PlannerTestContext {
        session_vars,
        expr_ctx,
        ranger_ctx: base::RangerContext {
            TypeCtx: expression::types::DefaultStmtNoWarningContext.clone(),
            ErrCtx: expression::errctx::StrictNoWarningContext.clone(),
            ExprCtx: ranger_expr,
            RangeFallbackHandler: None,
            PlanCacheTracker: None,
            OptimizerFixControl: Default::default(),
            UseCache: false,
            RegardNULLAsPoint: true,
            OptPrefixIndexSingleScan: false,
        },
        builtin_usage: base::BuiltinFunctionUsageCounter::default(),
        stats_load_waiter,
    })
}

/// 默认 Mock InfoSchema（表 t 无分区）。
fn planner_test_schema() -> Arc<dyn infoschema_dependency::infoschema::InfoSchema> {
    planner_test_schema_with_partitioned_t(None)
}

/// 构建对齐 Go coretestsdk 的表元数据；可选为 t 增加 Range 分区。
fn planner_test_schema_with_partitioned_t(
    partition_count: Option<usize>,
) -> Arc<dyn infoschema_dependency::infoschema::InfoSchema> {
    let table_names = [
        "t",
        "t2",
        "t3",
        "pt2",
        "u1",
        "user1",
        "pt2_global_index",
        "T_StateNoneColumn",
        "v",
    ];
    let tables = table_names
        .iter()
        .enumerate()
        .map(|(table_offset, table_name)| {
            let table_id = match *table_name {
                "t" => 1,
                "t3" => 4,
                "pt2" => 5,
                _ => 10_000 + table_offset as i64,
            };
            let column_names: Vec<&str> = if matches!(*table_name, "t" | "pt2") {
                // 对齐 Go coretestsdk.MockSignedTable，
                // coretestsdk.MockSignedTable, which backs the Go planner
                // 其列宽决定 plan_suite 输出中稳定的 Column# ID。
                // fixture suite. Its width also determines stable Column# IDs
                // 记录于 plan_suite_unexported_out.json。
                // recorded in plan_suite_unexported_out.json.
                let mut names = vec![
                    "a", "b", "c", "d", "e", "c_str", "d_str", "e_str", "f", "g", "h", "i_date",
                ];
                if *table_name == "pt2" || (*table_name == "t" && partition_count.is_some()) {
                    names.push("ptn");
                }
                names
            } else if *table_name == "t2" {
                // 对齐 coretestsdk.MockUnsignedTable。
                // coretestsdk.MockUnsignedTable.
                vec!["a", "b", "c"]
            } else {
                vec![
                    "a", "b", "c", "d", "e", "p", "o", "v", "c1", "f", "g", "c_str", "d_str",
                    "e_str",
                ]
            };
            let columns = column_names
                .iter()
                .enumerate()
                .map(|(offset, name)| {
                    let signed_layout = matches!(*table_name, "t" | "pt2");
                    let integer_layout = signed_layout || *table_name == "t2";
                    let mut field_type = if signed_layout && (5..=7).contains(&offset) {
                        *expression::types::NewFieldType(expression::mysql::TypeVarchar)
                    } else if signed_layout && offset == 11 {
                        *expression::types::NewFieldType(expression::mysql::TypeDate)
                    } else if integer_layout {
                        *expression::types::NewFieldType(expression::mysql::TypeLong)
                    } else {
                        *expression::types::NewFieldType(expression::mysql::TypeLonglong)
                    };
                    if offset == 0 && *table_name != "t3" {
                        field_type.AddFlag(
                            expression::mysql::PriKeyFlag | expression::mysql::NotNullFlag,
                        );
                    } else if *table_name == "t3" && offset == 0 {
                        // MockNoPKTable：列 a 为 NOT NULL，
                        // coretestsdk.MockNoPKTable keeps column `a` NOT NULL
                        // DELETE 使用额外 row handle。
                        // while using the extra row handle for DELETE.
                        field_type.AddFlag(expression::mysql::NotNullFlag);
                    } else if *table_name == "t" && matches!(offset, 1 | 2 | 3 | 8 | 9) {
                        field_type.AddFlag(expression::mysql::NotNullFlag);
                    }
                    if *table_name == "t2" && matches!(offset, 0 | 2) {
                        field_type.AddFlag(expression::mysql::UnsignedFlag);
                    }
                    if *table_name == "t2" && offset == 1 {
                        field_type.AddFlag(expression::mysql::NotNullFlag);
                    }
                    expression::model::ColumnInfo {
                        ID: table_id * 100 + offset as i64 + 1,
                        Name: crate::ast::NewCIStr(name),
                        Offset: offset as isize,
                        State: expression::model::StatePublic,
                        FieldType: field_type,
                        ..Default::default()
                    }
                })
                .collect::<Vec<_>>();
            let index_columns = |names: &[&str]| {
                names
                    .iter()
                    .map(|name| expression::model::IndexColumn {
                        Name: crate::ast::NewCIStr(name),
                        Offset: columns
                            .iter()
                            .position(|column| column.Name.L == *name)
                            .expect("planner-test index column exists")
                            as isize,
                        Length: -1,
                        ..Default::default()
                    })
                    .collect()
            };
            let mut indices = if *table_name == "t3" {
                Vec::new()
            } else if *table_name == "t2" {
                vec![
                    expression::model::IndexInfo {
                        ID: table_id * 10 + 1,
                        Name: crate::ast::NewCIStr("b"),
                        Table: crate::ast::NewCIStr(table_name),
                        Columns: index_columns(&["b"]),
                        State: expression::model::StatePublic,
                        Unique: true,
                        ..Default::default()
                    },
                    expression::model::IndexInfo {
                        ID: table_id * 10 + 2,
                        Name: crate::ast::NewCIStr("b_c"),
                        Table: crate::ast::NewCIStr(table_name),
                        Columns: index_columns(&["b", "c"]),
                        State: expression::model::StatePublic,
                        ..Default::default()
                    },
                ]
            } else {
                vec![expression::model::IndexInfo {
                    ID: table_id * 10 + 1,
                    Name: crate::ast::NewCIStr("c_d_e"),
                    Table: crate::ast::NewCIStr(table_name),
                    Columns: index_columns(&["c", "d", "e"]),
                    State: expression::model::StatePublic,
                    Unique: *table_name == "t",
                    ..Default::default()
                }]
            };
            if *table_name == "t" {
                indices.push(expression::model::IndexInfo {
                    ID: table_id * 10 + 7,
                    Name: crate::ast::NewCIStr("x"),
                    Table: crate::ast::NewCIStr(table_name),
                    Columns: index_columns(&["e"]),
                    State: expression::model::StateWriteOnly,
                    Unique: false,
                    ..Default::default()
                });
                indices.push(expression::model::IndexInfo {
                    ID: table_id * 10 + 2,
                    Name: crate::ast::NewCIStr("f"),
                    Table: crate::ast::NewCIStr(table_name),
                    Columns: index_columns(&["f"]),
                    State: expression::model::StatePublic,
                    Unique: true,
                    ..Default::default()
                });
                indices.push(expression::model::IndexInfo {
                    ID: table_id * 10 + 3,
                    Name: crate::ast::NewCIStr("g"),
                    Table: crate::ast::NewCIStr(table_name),
                    Columns: index_columns(&["g"]),
                    State: expression::model::StatePublic,
                    Unique: false,
                    ..Default::default()
                });
                indices.push(expression::model::IndexInfo {
                    ID: table_id * 10 + 4,
                    Name: crate::ast::NewCIStr("f_g"),
                    Table: crate::ast::NewCIStr(table_name),
                    Columns: index_columns(&["f", "g"]),
                    State: expression::model::StatePublic,
                    Unique: true,
                    ..Default::default()
                });
                indices.push(expression::model::IndexInfo {
                    ID: table_id * 10 + 5,
                    Name: crate::ast::NewCIStr("c_d_e_str"),
                    Table: crate::ast::NewCIStr(table_name),
                    Columns: index_columns(&["c_str", "d_str", "e_str"]),
                    State: expression::model::StatePublic,
                    Unique: false,
                    ..Default::default()
                });
                let mut prefix = index_columns(&["e_str", "d_str", "c_str"]);
                prefix[2].Length = 10;
                indices.push(expression::model::IndexInfo {
                    ID: table_id * 10 + 6,
                    Name: crate::ast::NewCIStr("e_d_c_str_prefix"),
                    Table: crate::ast::NewCIStr(table_name),
                    Columns: prefix,
                    State: expression::model::StatePublic,
                    Unique: false,
                    ..Default::default()
                });
            }
            let partition = if *table_name == "t" && partition_count.is_some() {
                let less_than = ["16", "32", "64", "128", "maxvalue"];
                Some(expression::model::PartitionInfo {
                    Enable: true,
                    Num: partition_count.unwrap_or_default() as u64,
                    Definitions: less_than
                        .iter()
                        .take(partition_count.unwrap_or_default())
                        .enumerate()
                        .map(|(index, boundary)| expression::model::PartitionDefinition {
                            ID: 41 + index as i64,
                            Name: crate::ast::NewCIStr(&format!("p{}", index + 1)),
                            LessThan: vec![(*boundary).to_owned()],
                            ..Default::default()
                        })
                        .collect(),
                    Type: expression::model::ast::PartitionType::Range,
                    Expr: "ptn".to_owned(),
                    ..Default::default()
                })
            } else if *table_name == "pt2" {
                Some(expression::model::PartitionInfo {
                    Enable: true,
                    Num: 2,
                    Definitions: vec![
                        expression::model::PartitionDefinition {
                            ID: 51,
                            Name: crate::ast::NewCIStr("p1"),
                            ..Default::default()
                        },
                        expression::model::PartitionDefinition {
                            ID: 52,
                            Name: crate::ast::NewCIStr("p2"),
                            ..Default::default()
                        },
                    ],
                    Type: expression::model::ast::PartitionType::Hash,
                    Expr: "ptn".to_owned(),
                    ..Default::default()
                })
            } else {
                None
            };
            let model = Arc::new(expression::model::TableInfo {
                ID: table_id,
                Name: crate::ast::NewCIStr(table_name),
                Columns: columns,
                Indices: indices,
                PKIsHandle: *table_name != "t3",
                Partition: partition,
                ..Default::default()
            });
            infoschema_dependency::infoschema::TableInfo {
                id: table_id,
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

/// 为 DataSource 填入固定行数/NDV，使代价估计路径可运行。
struct PlannerTestStatsProvider;

impl crate::DataSourceProvider for PlannerTestStatsProvider {
    /// 填充表级与访问路径上的伪统计计数。
    fn Populate(
        &self,
        _ctx: &dyn crate::context::Context,
        _plan_ctx: &base::ContextRef,
        _info_schema: &dyn infoschema_dependency::infoschema::InfoSchema,
        _table: &crate::ast::TableName,
        source: &mut logicalop::DataSource,
    ) -> Result<(), expression::Error> {
        source.TableStats.RowCount = statistics_dependency::PseudoRowCount as f64;
        source.TableStats.StatsVersion = statistics_dependency::PseudoVersion;
        source.TableStats.ColNDVs = source
            .Schema()
            .Columns
            .iter()
            .map(|column| (column.UniqueID, source.TableStats.RowCount * 0.8))
            .collect();
        for path in source
            .AllPossibleAccessPaths
            .iter_mut()
            .chain(source.PossibleAccessPaths.iter_mut())
        {
            path.CountAfterAccess = source.TableStats.RowCount;
            path.MinCountAfterAccess = source.TableStats.RowCount;
            path.MaxCountAfterAccess = source.TableStats.RowCount;
            path.CountAfterIndex = source.TableStats.RowCount;
        }
        Ok(())
    }
}

/// 解析并构建 SQL 的运行时计划（默认上下文与 schema）。
pub(crate) fn build_runtime_for_test(
    sql: &str,
) -> Result<(base::ContextRef, u64, crate::BuiltRuntimePlan), String> {
    build_runtime_for_test_with_context(sql, planner_test_context())
}

/// Go TestPlanBuilder explicitly uses cost model v1 and one hash-join worker.
pub(crate) fn build_plan_builder_fixture_for_test(
    sql: &str,
) -> Result<(base::ContextRef, u64, crate::BuiltRuntimePlan), String> {
    let context = planner_test_context_with_stats_state(0, false, None, false, |vars| {
        vars.SetSystemVar(vardef_dependency::TiDBCostModelVersion, "1")
            .unwrap();
        vars.SetSystemVar(vardef_dependency::TiDBHashJoinConcurrency, "1")
            .unwrap();
    });
    build_runtime_for_test_with_context(sql, context)
}

/// 使用指定上下文构建运行时计划。
fn build_runtime_for_test_with_context(
    sql: &str,
    context: base::ContextRef,
) -> Result<(base::ContextRef, u64, crate::BuiltRuntimePlan), String> {
    build_runtime_for_test_with_context_and_schema(sql, context, planner_test_schema())
}

/// 完整 build：安装表达式工厂、ParseOneStmt、PlanBuilder.Build。
fn build_runtime_for_test_with_context_and_schema(
    sql: &str,
    context: base::ContextRef,
    info_schema: Arc<dyn infoschema_dependency::infoschema::InfoSchema>,
) -> Result<(base::ContextRef, u64, crate::BuiltRuntimePlan), String> {
    crate::InstallPlannerExpressionFactory().map_err(|error| error.to_string())?;
    let statement = crate::ast::NodeRef::new(
        parser_dependency::New()
            .ParseOneStmt(sql, "", "")
            .map_err(|error| format!("parse {sql:?}: {error}"))?,
    );
    let (mut builder, _) = crate::NewPlanBuilder()
        .withDataSourceProvider(Arc::new(PlannerTestStatsProvider))
        .Init(
            context.clone(),
            info_schema,
            hint_dependency::NewQBHintHandler(None),
        );
    let built = builder
        .BuildNodeRef(crate::context::TODO(), &statement)
        .map_err(|error| format!("build {sql:?}: {error}"))?;
    let opt_flag = builder.GetOptFlag();
    Ok((context, opt_flag, built))
}

/// 分区 fixture：按 IsIdx 选择分区数量后做逻辑优化。
pub(crate) fn logical_optimize_partition_fixture_for_test(
    sql: &str,
    info_schema_index: usize,
    flags: u64,
) -> Result<(base::ContextRef, logicalop::LogicalPlanRef), String> {
    let context = planner_test_context();
    let partition_count = match info_schema_index {
        0 => 5,
        1 => 4,
        index => return Err(format!("unknown partition fixture schema index {index}")),
    };
    let (context, _, built) = build_runtime_for_test_with_context_and_schema(
        sql,
        context,
        planner_test_schema_with_partitioned_t(Some(partition_count)),
    )?;
    let crate::BuiltRuntimePlan::Logical(mut logical) = built else {
        return Err(format!("build {sql:?}: expected a logical query plan"));
    };
    crate::LogicalOptimizeForTest(flags, &mut logical)
        .map_err(|error| format!("logical optimize {sql:?}: {error}"))?;
    Ok((context, logical))
}

/// 带挂起统计项的逻辑优化入口。
pub(crate) fn logical_optimize_with_pending_stats_for_test(
    sql: &str,
    flags: u64,
    pending_items: usize,
    sync_failed: bool,
) -> Result<(base::ContextRef, logicalop::LogicalPlanRef), String> {
    let context =
        planner_test_context_with_stats_state(pending_items, sync_failed, None, false, |_| {});
    let (context, _, built) = build_runtime_for_test_with_context(sql, context)?;
    let crate::BuiltRuntimePlan::Logical(mut logical) = built else {
        return Err(format!("build {sql:?}: expected a logical query plan"));
    };
    crate::LogicalOptimizeForTest(flags, &mut logical)
        .map_err(|error| format!("logical optimize {sql:?}: {error}"))?;
    Ok((context, logical))
}

/// 注入 StatsLoadWaiter 的逻辑优化；优化错误作为内层 Result 返回。
pub(crate) fn logical_optimize_with_stats_handle_for_test(
    sql: &str,
    flags: u64,
    pending_items: usize,
    sync_failed: bool,
    stats_handle: Arc<PlannerTestStatsHandle>,
) -> Result<(base::ContextRef, Result<logicalop::LogicalPlanRef, String>), String> {
    let waiter: Arc<dyn base::StatsLoadWaiter> = stats_handle;
    let context = planner_test_context_with_stats_state(
        pending_items,
        sync_failed,
        Some(waiter),
        false,
        |_| {},
    );
    let (context, _, built) = build_runtime_for_test_with_context(sql, context)?;
    let crate::BuiltRuntimePlan::Logical(mut logical) = built else {
        return Err(format!("build {sql:?}: expected a logical query plan"));
    };
    let optimized = crate::LogicalOptimizeForTest(flags, &mut logical)
        .map(|_| logical)
        .map_err(|error| format!("logical optimize {sql:?}: {error}"));
    Ok((context, optimized))
}

/// 仅构建逻辑计划，不做优化。
pub(crate) fn build_logical_for_test(
    sql: &str,
) -> Result<(base::ContextRef, logicalop::LogicalPlanRef), String> {
    let (context, _, built) = build_runtime_for_test(sql)?;
    match built {
        crate::BuiltRuntimePlan::Logical(logical) => Ok((context, logical)),
        crate::BuiltRuntimePlan::NonLogical(_) => {
            Err(format!("build {sql:?}: expected a logical query plan"))
        }
    }
}

/// 使用显式规则标志做逻辑优化。
pub(crate) fn logical_optimize_for_test(
    sql: &str,
    flags: u64,
) -> Result<(base::ContextRef, logicalop::LogicalPlanRef), String> {
    let (context, _, built) = build_runtime_for_test(sql)?;
    let crate::BuiltRuntimePlan::Logical(mut logical) = built else {
        return Err(format!("build {sql:?}: expected a logical query plan"));
    };
    crate::LogicalOptimizeForTest(flags, &mut logical)
        .map_err(|error| format!("logical optimize {sql:?}: {error}"))?;
    Ok((context, logical))
}

pub(crate) fn logical_optimize_with_agg_pushdown_for_test(
    sql: &str,
    flags: u64,
) -> Result<(base::ContextRef, logicalop::LogicalPlanRef), String> {
    let context = planner_test_context_with_stats_state(0, false, None, true, |_| {});
    let (context, _, built) = build_runtime_for_test_with_context(sql, context)?;
    let crate::BuiltRuntimePlan::Logical(mut logical) = built else {
        return Err(format!("build {sql:?}: expected a logical query plan"));
    };
    crate::LogicalOptimizeForTest(flags, &mut logical)
        .map_err(|error| format!("logical optimize {sql:?}: {error}"))?;
    Ok((context, logical))
}

/// 使用 PlanBuilder 产出的默认 OptFlag 做逻辑优化。
pub(crate) fn logical_optimize_default_for_test(
    sql: &str,
) -> Result<(base::ContextRef, logicalop::LogicalPlanRef), String> {
    let (context, flags, built) = build_runtime_for_test(sql)?;
    let crate::BuiltRuntimePlan::Logical(mut logical) = built else {
        return Err(format!("build {sql:?}: expected a logical query plan"));
    };
    crate::LogicalOptimizeForTest(flags, &mut logical)
        .map_err(|error| format!("logical optimize {sql:?}: {error}"))?;
    Ok((context, logical))
}

/// 完整 DoOptimize，得到物理计划并校验代价与 Schema。
pub(crate) fn optimize_query_for_test(sql: &str) -> Result<Box<dyn base::PhysicalPlan>, String> {
    let (context, opt_flag, built) = build_runtime_for_test(sql)?;
    let crate::BuiltRuntimePlan::Logical(mut logical) = built else {
        return Err(format!("build {sql:?}: expected a logical query plan"));
    };
    let (physical, cost) =
        crate::DoOptimize(crate::context::TODO(), &context, opt_flag, &mut logical)
            .map_err(|error| format!("optimize {sql:?}: {error}"))?;
    if !cost.is_finite() || cost <= 0.0 {
        return Err(format!("optimize {sql:?}: invalid cost {cost}"));
    }
    if physical.schema().Len() == 0 {
        return Err(format!("optimize {sql:?}: unexpectedly empty schema"));
    }
    Ok(physical)
}

/// 物理优化但跳过 post-optimize（窗口并发默认 1）。
pub(crate) fn optimize_query_without_post_for_test(
    sql: &str,
) -> Result<Box<dyn base::PhysicalPlan>, String> {
    optimize_query_without_post_with_window_concurrency_for_test(sql, 1)
}

/// 可配置窗口并发的物理优化（无 post 阶段）。
pub(crate) fn optimize_query_without_post_with_window_concurrency_for_test(
    sql: &str,
    window_concurrency: usize,
) -> Result<Box<dyn base::PhysicalPlan>, String> {
    let context = planner_test_context_with_stats_state(0, false, None, false, |vars| {
        vars.SetSystemVar(vardef_dependency::TiDBCostModelVersion, "1")
            .unwrap();
        // Go's MockContext leaves EnablePipelinedWindowExec at its zero value;
        // it does not load the server's ON default for this planner fixture.
        vars.SetSystemVar(vardef_dependency::TiDBEnablePipelinedWindowFunction, "0")
            .unwrap();
    });
    let (_, opt_flag, built) =
        build_runtime_for_test_with_context(sql, context).map_err(|error| {
            error
                .strip_prefix(&format!("build {sql:?}: "))
                .unwrap_or(&error)
                .to_owned()
        })?;
    let crate::BuiltRuntimePlan::Logical(mut logical) = built else {
        return Err(format!("build {sql:?}: expected a logical query plan"));
    };
    crate::LogicalOptimizeForTest(opt_flag, &mut logical).map_err(|error| error.to_string())?;
    let (physical, cost) = crate::optimizer_runtime::PhysicalOptimizeForTestWithWindowConcurrency(
        &mut logical,
        window_concurrency,
    )
    .map_err(|error| error.to_string())?;
    if !cost.is_finite() || cost <= 0.0 {
        return Err(format!("optimize {sql:?}: invalid cost {cost}"));
    }
    if physical.schema().Len() == 0 {
        return Err(format!("optimize {sql:?}: unexpectedly empty schema"));
    }
    Ok(physical)
}

/// 冒烟：能 build 的逻辑语句走完优化；非逻辑语句直接 Ok。
pub(crate) fn exercise_statement_for_test(sql: &str) -> Result<(), String> {
    let (context, opt_flag, built) = build_runtime_for_test(sql)?;
    let crate::BuiltRuntimePlan::Logical(mut logical) = built else {
        return Ok(());
    };
    if logical.Schema().Len() == 0 {
        return Err(format!("build {sql:?}: unexpectedly empty schema"));
    }
    let (physical, cost) =
        crate::DoOptimize(crate::context::TODO(), &context, opt_flag, &mut logical)
            .map_err(|error| format!("optimize {sql:?}: {error}"))?;
    if !cost.is_finite() || cost <= 0.0 || physical.schema().Len() == 0 {
        return Err(format!("optimize {sql:?}: invalid plan/cost {cost}"));
    }
    Ok(())
}

#[test]
fn go_merge_46_full_join_defaults_to_disabled() {
    let sql = "select * from t t1 full outer join t t2 on t1.a = t2.a";
    let error = build_logical_for_test(sql)
        .err()
        .expect("FULL OUTER JOIN is opt-in");
    assert!(
        error.contains("FULL OUTER JOIN"),
        "unexpected error: {error}"
    );
}

#[test]
fn go_merge_46_full_join_enabled_builds_logical_and_hash_plan() {
    let sql = "select * from t t1 full outer join t t2 on t1.a = t2.a";
    let context = planner_test_context_with_stats_state(0, false, None, false, |vars| {
        vars.SetSystemVar("tidb_enable_full_outer_join", "ON")
            .expect("enable FULL OUTER JOIN");
    });
    let (context, flags, built) =
        build_runtime_for_test_with_context(sql, context).expect("build FULL OUTER JOIN");
    let crate::BuiltRuntimePlan::Logical(mut logical) = built else {
        panic!("query must build a logical plan");
    };
    fn find_logical_join(plan: &dyn logicalop::LogicalPlan) -> Option<&logicalop::LogicalJoin> {
        plan.as_any()
            .downcast_ref::<logicalop::LogicalJoin>()
            .or_else(|| {
                plan.Children()
                    .iter()
                    .find_map(|child| find_logical_join(child.as_ref()))
            })
    }
    let join = find_logical_join(logical.as_ref()).expect("logical join");
    assert_eq!(join.JoinType, base::JoinType::FullOuterJoin);
    assert!(join.Schema().Columns.iter().all(|column| {
        column
            .RetType
            .as_ref()
            .is_some_and(|field| !expression::mysql::HasNotNullFlag(field.GetFlag()))
    }));
    let (physical, _) = crate::DoOptimize(crate::context::TODO(), &context, flags, &mut logical)
        .expect("optimize FULL OUTER JOIN");
    fn find_hash_join(
        plan: &dyn base::PhysicalPlan,
    ) -> Option<&physicalop_dependency::PhysicalHashJoin> {
        plan.as_any()
            .downcast_ref::<physicalop_dependency::PhysicalHashJoin>()
            .or_else(|| {
                plan.children()
                    .iter()
                    .find_map(|child| find_hash_join(*child))
            })
    }
    let join = find_hash_join(physical.as_ref()).expect("physical HashJoin");
    assert_eq!(
        join.BasePhysicalJoin.JoinType,
        base::JoinType::FullOuterJoin
    );
    assert!(!join.UseOuterToBuild);
}

#[test]
fn go_merge_46_full_join_rejects_unsupported_forms_and_cascades() {
    for sql in [
        "select * from t t1 full outer join t t2 using (a)",
        "select * from t t1 natural full outer join t t2",
        "select * from t t1 full outer join lateral (select 1 as a) as t2 on false",
    ] {
        let context = planner_test_context_with_stats_state(0, false, None, false, |vars| {
            vars.SetSystemVar("tidb_enable_full_outer_join", "ON")
                .expect("enable FULL OUTER JOIN");
        });
        let error = build_runtime_for_test_with_context(sql, context)
            .err()
            .unwrap_or_else(|| panic!("{sql} must be rejected"));
        assert!(error.contains("FULL OUTER JOIN"), "{sql}: {error}");
    }
    let sql = "select * from t t1 full outer join t t2 on t1.a = t2.a";
    let context = planner_test_context_with_stats_state(0, false, None, false, |vars| {
        vars.SetSystemVar("tidb_enable_full_outer_join", "ON")
            .expect("enable FULL OUTER JOIN");
        vars.SetSystemVar("tidb_enable_cascades_planner", "ON")
            .expect("enable Cascades");
    });
    let error = build_runtime_for_test_with_context(sql, context)
        .err()
        .expect("FULL OUTER JOIN with Cascades must be rejected");
    assert!(
        error.contains("FULL OUTER JOIN"),
        "unexpected error: {error}"
    );
}

#[test]
fn go_merge_46_full_join_null_rejected_predicates_simplify_join_type() {
    for (predicate, expected) in [
        ("t1.b > 1", base::JoinType::LeftOuterJoin),
        ("t2.b > 1", base::JoinType::RightOuterJoin),
        ("t1.b > 1 and t2.b > 1", base::JoinType::InnerJoin),
        ("t1.b > 1 or t2.b > 1", base::JoinType::FullOuterJoin),
    ] {
        let sql =
            format!("select * from t t1 full outer join t t2 on t1.a = t2.a where {predicate}");
        let context = planner_test_context_with_stats_state(0, false, None, false, |vars| {
            vars.SetSystemVar("tidb_enable_full_outer_join", "ON")
                .expect("enable FULL OUTER JOIN");
        });
        let (_, _, built) =
            build_runtime_for_test_with_context(&sql, context).expect("build FULL OUTER JOIN");
        let crate::BuiltRuntimePlan::Logical(mut logical) = built else {
            panic!("query must build a logical plan");
        };
        crate::LogicalOptimizeForTest(rule_dependency::FLAG_PREDICATE_PUSH_DOWN, &mut logical)
            .expect("simplify FULL OUTER JOIN");
        fn find_join(plan: &dyn logicalop::LogicalPlan) -> Option<base::JoinType> {
            plan.as_any()
                .downcast_ref::<logicalop::LogicalJoin>()
                .map(|join| join.JoinType)
                .or_else(|| {
                    plan.Children()
                        .iter()
                        .find_map(|child| find_join(child.as_ref()))
                })
        }
        assert_eq!(find_join(logical.as_ref()), Some(expected), "{sql}");
    }
}

#[test]
fn go_merge_46_non_unique_index_range_includes_signed_handle() {
    let sql = "select * from t use index(g) where g = 5 and a = 7";
    let (_, mut logical) =
        build_logical_for_test(sql).expect("build index and handle access range");
    crate::LogicalOptimizeForTest(rule_dependency::FLAG_PREDICATE_PUSH_DOWN, &mut logical)
        .expect("derive index access ranges");
    fn find_source(plan: &dyn logicalop::LogicalPlan) -> Option<&logicalop::DataSource> {
        plan.as_any()
            .downcast_ref::<logicalop::DataSource>()
            .or_else(|| {
                plan.Children()
                    .iter()
                    .find_map(|child| find_source(child.as_ref()))
            })
    }
    let source = find_source(logical.as_ref()).expect("table source");
    let path = source
        .PossibleAccessPaths
        .iter()
        .find(|path| path.Index.as_ref().is_some_and(|index| index.Name.L == "g"))
        .expect("non-unique g index path");
    assert_eq!(
        path.IdxCols.len(),
        2,
        "index key includes signed primary handle"
    );
    assert!(path.Ranges.iter().any(|range| range.LowVal.len() == 2));
}

#[test]
fn go_merge_46_unsupported_join_hints_warn() {
    for (hint, name) in [
        ("MERGE_JOIN(t1, t2)", "MERGE_JOIN"),
        ("INL_JOIN(t2)", "INL_JOIN"),
    ] {
        let sql = format!("select /*+ {hint} */ * from t t1 full outer join t t2 on t1.a = t2.a");
        let context = planner_test_context_with_stats_state(0, false, None, false, |vars| {
            vars.SetSystemVar("tidb_enable_full_outer_join", "ON")
                .expect("enable FULL OUTER JOIN");
        });
        let (context, flags, built) =
            build_runtime_for_test_with_context(&sql, context).expect("build hinted join");
        let crate::BuiltRuntimePlan::Logical(mut logical) = built else {
            panic!("query must build a logical plan");
        };
        let (physical, _) =
            crate::DoOptimize(crate::context::TODO(), &context, flags, &mut logical)
                .expect("optimize hinted join");
        assert!(physical.schema().Len() > 0);
        let warnings = context.GetSessionVars().StmtCtx.GetWarnings();
        assert!(
            warnings.iter().any(|warning| warning
                .Err
                .as_ref()
                .is_some_and(|error| error.to_string().contains(name)
                    && error.to_string().contains("inapplicable"))),
            "{sql}: {warnings:?}"
        );
    }
}

#[test]
fn go_merge_46_join_reorder_preserves_two_full_joins() {
    let sql = "select * from t t1 full outer join t t2 on t1.a = t2.a full outer join t t3 on t2.a = t3.a";
    let context = planner_test_context_with_stats_state(0, false, None, false, |vars| {
        vars.SetSystemVar("tidb_enable_full_outer_join", "ON")
            .expect("enable FULL OUTER JOIN");
    });
    let (_, _, built) = build_runtime_for_test_with_context(sql, context).expect("build joins");
    let crate::BuiltRuntimePlan::Logical(mut logical) = built else {
        panic!("query must build a logical plan");
    };
    crate::LogicalOptimizeForTest(
        rule_dependency::FLAG_PREDICATE_PUSH_DOWN | rule_dependency::FLAG_JOIN_REORDER,
        &mut logical,
    )
    .expect("reorder joins");
    fn count_full_joins(plan: &dyn logicalop::LogicalPlan) -> usize {
        usize::from(
            plan.as_any()
                .downcast_ref::<logicalop::LogicalJoin>()
                .is_some_and(|join| join.JoinType == base::JoinType::FullOuterJoin),
        ) + plan
            .Children()
            .iter()
            .map(|child| count_full_joins(child.as_ref()))
            .sum::<usize>()
    }
    assert_eq!(count_full_joins(logical.as_ref()), 2);
}

#[test]
fn go_merge_46_full_join_tail_scan_increases_both_cost_models() {
    fn optimize_join(full: bool) -> Box<dyn base::PhysicalPlan> {
        let join = if full { "full outer join" } else { "join" };
        let sql = format!("select /*+ HASH_JOIN(t1, t2) */ * from t t1 {join} t t2 on t1.a = t2.a");
        let context = planner_test_context_with_stats_state(0, false, None, false, |vars| {
            vars.SetSystemVar("tidb_enable_full_outer_join", "ON")
                .expect("enable FULL OUTER JOIN");
        });
        let (context, flags, built) =
            build_runtime_for_test_with_context(&sql, context).expect("build cost query");
        let crate::BuiltRuntimePlan::Logical(mut logical) = built else {
            panic!("query must build a logical plan");
        };
        crate::DoOptimize(crate::context::TODO(), &context, flags, &mut logical)
            .expect("optimize cost query")
            .0
    }
    fn find_join(plan: &dyn base::PhysicalPlan) -> &physicalop_dependency::PhysicalHashJoin {
        plan.as_any()
            .downcast_ref::<physicalop_dependency::PhysicalHashJoin>()
            .or_else(|| {
                plan.children().iter().find_map(|child| {
                    child
                        .as_any()
                        .downcast_ref::<physicalop_dependency::PhysicalHashJoin>()
                })
            })
            .expect("physical HashJoin")
    }
    let inner = optimize_join(false);
    let full = optimize_join(true);
    let inner = find_join(inner.as_ref());
    let full = find_join(full.as_ref());
    let option = costusage_dependency::new_default_plan_cost_option();
    let inner_v1 = crate::plan_cost_ver1::GetCanonicalPlanCostVer1(
        inner,
        property_dependency::RootTaskType,
        &option,
    )
    .expect("inner v1 cost");
    let full_v1 = crate::plan_cost_ver1::GetCanonicalPlanCostVer1(
        full,
        property_dependency::RootTaskType,
        &option,
    )
    .expect("full v1 cost");
    assert!(full_v1 > inner_v1, "v1: full={full_v1}, inner={inner_v1}");
    let inner_v2 = crate::plan_cost_ver2::GetCanonicalPlanCostVer2(
        inner,
        property_dependency::RootTaskType,
        &option,
        &[],
    )
    .expect("inner v2 cost")
    .get_cost();
    let full_v2 = crate::plan_cost_ver2::GetCanonicalPlanCostVer2(
        full,
        property_dependency::RootTaskType,
        &option,
        &[],
    )
    .expect("full v2 cost")
    .get_cost();
    assert!(full_v2 > inner_v2, "v2: full={full_v2}, inner={inner_v2}");
}

/// 过滤出 SELECT/WITH 查询字面量。
pub(crate) fn sql_literals<'a>(literals: &'a [&'a str]) -> impl Iterator<Item = &'a str> {
    literals.iter().copied().filter(|literal| {
        let lower = literal.trim_start().to_ascii_lowercase();
        lower.starts_with("select ") || lower.starts_with("with ")
    })
}

#[derive(Clone, Debug, PartialEq)]
/// 分区用例输入：SQL 与 InfoSchema 索引。
pub(crate) struct TablePartitionInput {
    pub sql: String,
    pub info_schema_index: usize,
}

#[derive(Clone, Debug, PartialEq)]
/// fixture 单条输入：普通 SQL 或分区对象。
pub(crate) enum FixtureCase {
    Sql(String),
    TablePartition(TablePartitionInput),
}

impl FixtureCase {
    /// 取出用例 SQL 文本。
    pub fn sql(&self) -> &str {
        match self {
            Self::Sql(sql) => sql,
            Self::TablePartition(case) => &case.sql,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
/// UNION 等：最优计划字符串 + 是否期望错误。
pub(crate) struct BestErrorExpected {
    pub best: String,
    pub error: bool,
}

#[derive(Clone, Debug, PartialEq)]
/// 按节点 ID 映射的唯一键列名集合。
pub(crate) struct UniqueKeyExpected {
    pub nodes: std::collections::BTreeMap<usize, Vec<Vec<String>>>,
}

#[derive(Clone, Debug, PartialEq)]
/// 列裁剪后各 DataSource 保留列名。
pub(crate) struct ColumnPruningExpected {
    pub data_sources: std::collections::BTreeMap<usize, Vec<String>>,
}

#[derive(Clone, Debug, PartialEq)]
/// 导出 NOT NULL 条件：计划与左右下推谓词。
pub(crate) struct DeriveNotNullExpected {
    pub plan: String,
    pub left: String,
    pub right: String,
}

#[derive(Clone, Debug, PartialEq)]
/// Join 两侧下推谓词字符串。
pub(crate) struct JoinPredicatesExpected {
    pub left: String,
    pub right: String,
}

#[derive(Clone, Debug, PartialEq)]
/// 外连接 WHERE Selection 与两侧下推谓词。
pub(crate) struct OuterPredicatesExpected {
    pub selection: String,
    pub left: String,
    pub right: String,
}

#[derive(Clone, Debug, PartialEq)]
/// 外连接简化后的最优计划与 JoinType。
pub(crate) struct SimplifyOuterExpected {
    pub best: String,
    pub join_type: String,
}

#[derive(Clone, Debug, PartialEq)]
/// DELETE 列裁剪：输出布局、表列位置信息与内部计划。
pub(crate) struct PruneDeleteExpected {
    pub sql: String,
    pub pruned_output: String,
    pub full_layout_info: Vec<Vec<String>>,
    pub inside_plan: String,
}

#[derive(Clone, Debug, PartialEq)]
/// fixture 期望值的类型化枚举。
pub(crate) enum FixtureExpected {
    Plan(String),
    BestError(BestErrorExpected),
    UniqueKey(UniqueKeyExpected),
    ColumnPruning(ColumnPruningExpected),
    SortColumns(Vec<String>),
    DeriveNotNull(DeriveNotNullExpected),
    OuterPredicates(OuterPredicatesExpected),
    JoinPredicates(JoinPredicatesExpected),
    SimplifyOuter(SimplifyOuterExpected),
    PruneDelete(PruneDeleteExpected),
}

#[derive(Clone, Debug, PartialEq)]
/// 一组命名用例：输入与期望一一对应。
pub(crate) struct PlanFixture {
    pub name: String,
    pub cases: Vec<FixtureCase>,
    pub expected: Vec<FixtureExpected>,
}

#[derive(Clone, Debug, PartialEq)]
/// 严格 JSON 子集 AST（拒绝重复键）。
enum JsonValue {
    Bool(bool),
    Number(i64),
    String(String),
    Array(Vec<JsonValue>),
    Object(std::collections::BTreeMap<String, JsonValue>),
    Null,
}

/// 手写 JSON 解析器，保证未知/重复字段可被检出。
struct JsonParser<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> JsonParser<'a> {
    /// 从字符串构造解析器。
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            offset: 0,
        }
    }

    /// 解析完整 JSON 值，不允许尾随内容。
    fn parse(mut self) -> Result<JsonValue, String> {
        let value = self.value()?;
        self.space();
        if self.offset != self.input.len() {
            return Err(format!("trailing JSON at byte {}", self.offset));
        }
        Ok(value)
    }

    /// 跳过空白。
    fn space(&mut self) {
        while self
            .input
            .get(self.offset)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.offset += 1;
        }
    }

    /// 期望并消费指定字节。
    fn take(&mut self, byte: u8) -> Result<(), String> {
        self.space();
        if self.input.get(self.offset) != Some(&byte) {
            return Err(format!(
                "expected {:?} at byte {}",
                byte as char, self.offset
            ));
        }
        self.offset += 1;
        Ok(())
    }

    /// 解析一个 JSON 值。
    fn value(&mut self) -> Result<JsonValue, String> {
        self.space();
        match self.input.get(self.offset).copied() {
            Some(b'"') => self.string().map(JsonValue::String),
            Some(b'[') => self.array(),
            Some(b'{') => self.object(),
            Some(b't') if self.keyword(b"true") => Ok(JsonValue::Bool(true)),
            Some(b'f') if self.keyword(b"false") => Ok(JsonValue::Bool(false)),
            Some(b'n') if self.keyword(b"null") => Ok(JsonValue::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            other => Err(format!("unexpected JSON byte {other:?} at {}", self.offset)),
        }
    }

    /// 尝试匹配关键字字面量。
    fn keyword(&mut self, keyword: &[u8]) -> bool {
        if self.input.get(self.offset..self.offset + keyword.len()) == Some(keyword) {
            self.offset += keyword.len();
            true
        } else {
            false
        }
    }

    /// 解析 JSON 字符串（含转义）。
    fn string(&mut self) -> Result<String, String> {
        self.take(b'"')?;
        let mut result = String::new();
        while let Some(byte) = self.input.get(self.offset).copied() {
            self.offset += 1;
            match byte {
                b'"' => return Ok(result),
                b'\\' => {
                    let escaped = self
                        .input
                        .get(self.offset)
                        .copied()
                        .ok_or_else(|| "unterminated JSON escape".to_owned())?;
                    self.offset += 1;
                    match escaped {
                        b'"' => result.push('"'),
                        b'\\' => result.push('\\'),
                        b'/' => result.push('/'),
                        b'b' => result.push('\u{0008}'),
                        b'f' => result.push('\u{000c}'),
                        b'n' => result.push('\n'),
                        b'r' => result.push('\r'),
                        b't' => result.push('\t'),
                        b'u' => {
                            let hex = std::str::from_utf8(
                                self.input
                                    .get(self.offset..self.offset + 4)
                                    .ok_or_else(|| "short JSON unicode escape".to_owned())?,
                            )
                            .map_err(|error| error.to_string())?;
                            self.offset += 4;
                            let value = u32::from_str_radix(hex, 16)
                                .map_err(|error| format!("invalid unicode escape: {error}"))?;
                            result.push(
                                char::from_u32(value)
                                    .ok_or_else(|| format!("invalid unicode scalar {value:#x}"))?,
                            );
                        }
                        _ => return Err(format!("invalid JSON escape {escaped:?}")),
                    }
                }
                byte if byte.is_ascii() => result.push(byte as char),
                _ => {
                    let remaining = std::str::from_utf8(&self.input[self.offset - 1..])
                        .map_err(|error| error.to_string())?;
                    let character = remaining
                        .chars()
                        .next()
                        .ok_or_else(|| "invalid UTF-8 string".to_owned())?;
                    self.offset += character.len_utf8() - 1;
                    result.push(character);
                }
            }
        }
        Err("unterminated JSON string".to_owned())
    }

    /// 解析 JSON 数组。
    fn array(&mut self) -> Result<JsonValue, String> {
        self.take(b'[')?;
        let mut values = Vec::new();
        self.space();
        if self.input.get(self.offset) == Some(&b']') {
            self.offset += 1;
            return Ok(JsonValue::Array(values));
        }
        loop {
            values.push(self.value()?);
            self.space();
            match self.input.get(self.offset) {
                Some(b',') => self.offset += 1,
                Some(b']') => {
                    self.offset += 1;
                    return Ok(JsonValue::Array(values));
                }
                _ => return Err(format!("expected array separator at {}", self.offset)),
            }
        }
    }

    /// 解析 JSON 对象；重复键报错。
    fn object(&mut self) -> Result<JsonValue, String> {
        self.take(b'{')?;
        let mut values = std::collections::BTreeMap::new();
        self.space();
        if self.input.get(self.offset) == Some(&b'}') {
            self.offset += 1;
            return Ok(JsonValue::Object(values));
        }
        loop {
            let key = self.string()?;
            self.take(b':')?;
            let value = self.value()?;
            if values.insert(key.clone(), value).is_some() {
                return Err(format!("duplicate JSON object key {key:?}"));
            }
            self.space();
            match self.input.get(self.offset) {
                Some(b',') => self.offset += 1,
                Some(b'}') => {
                    self.offset += 1;
                    return Ok(JsonValue::Object(values));
                }
                _ => return Err(format!("expected object separator at {}", self.offset)),
            }
        }
    }

    /// 解析整数 JSON 数字。
    fn number(&mut self) -> Result<JsonValue, String> {
        let start = self.offset;
        if self.input.get(self.offset) == Some(&b'-') {
            self.offset += 1;
        }
        while self.input.get(self.offset).is_some_and(u8::is_ascii_digit) {
            self.offset += 1;
        }
        let number = std::str::from_utf8(&self.input[start..self.offset])
            .map_err(|error| error.to_string())?
            .parse::<i64>()
            .map_err(|error| error.to_string())?;
        Ok(JsonValue::Number(number))
    }
}

/// 读取 fixture 文件，剥离 `//` 行注释后解析。
fn read_json(path: &std::path::Path) -> JsonValue {
    let data = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let json = data
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    JsonParser::new(&json)
        .parse()
        .unwrap_or_else(|error| panic!("decode typed fixture {}: {error}", path.display()))
}

/// 断言为对象并返回映射。
fn object(value: &JsonValue) -> &std::collections::BTreeMap<String, JsonValue> {
    let JsonValue::Object(value) = value else {
        panic!("expected JSON object")
    };
    value
}

/// 校验对象键集合与期望完全一致。
fn check_exact_keys(
    value: &std::collections::BTreeMap<String, JsonValue>,
    expected: &[&str],
) -> Result<(), String> {
    let actual = value.keys().map(String::as_str).collect::<Vec<_>>();
    let mut expected = expected.to_vec();
    expected.sort_unstable();
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "JSON object keys {actual:?}, expected {expected:?}"
        ))
    }
}

/// `check_exact_keys` 的 panic 包装。
fn exact_keys(value: &std::collections::BTreeMap<String, JsonValue>, expected: &[&str]) {
    check_exact_keys(value, expected).unwrap_or_else(|error| panic!("{error}"));
}

/// 断言为数组。
fn array(value: &JsonValue) -> &[JsonValue] {
    let JsonValue::Array(value) = value else {
        panic!("expected JSON array")
    };
    value
}

/// 断言为字符串并克隆。
fn string(value: &JsonValue) -> String {
    let JsonValue::String(value) = value else {
        panic!("expected JSON string")
    };
    value.clone()
}

/// 字符串数组。
fn strings(value: &JsonValue) -> Vec<String> {
    array(value).iter().map(string).collect()
}

/// 二维字符串数组。
fn nested_strings(value: &JsonValue) -> Vec<Vec<String>> {
    array(value).iter().map(strings).collect()
}

/// 在根数组中按 name 字段查找分组。
fn group<'a>(root: &'a JsonValue, name_key: &str, name: &str) -> &'a JsonValue {
    array(root)
        .iter()
        .find(|value| {
            object(value)
                .get(name_key)
                .is_some_and(|value| string(value) == name)
        })
        .unwrap_or_else(|| panic!("missing fixture group {name}"))
}

/// 加载 plan_suite_unexported 中指定分组的输入与期望。
pub(crate) fn plan_fixture(name: &str) -> PlanFixture {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    let inputs = read_json(&directory.join("plan_suite_unexported_in.json"));
    let outputs = read_json(&directory.join("plan_suite_unexported_out.json"));
    let input = object(group(&inputs, "name", name));
    let output = object(group(&outputs, "Name", name));
    exact_keys(input, &["cases", "name"]);
    exact_keys(output, &["Cases", "Name"]);
    let cases = array(input.get("cases").expect("fixture input cases"))
        .iter()
        .map(|case| match case {
            JsonValue::String(sql) => FixtureCase::Sql(sql.clone()),
            JsonValue::Object(case) => FixtureCase::TablePartition(TablePartitionInput {
                sql: {
                    exact_keys(case, &["IsIdx", "SQL"]);
                    string(case.get("SQL").expect("partition SQL"))
                },
                info_schema_index: match case.get("IsIdx").expect("partition IsIdx") {
                    JsonValue::Number(index) => *index as usize,
                    _ => panic!("partition IsIdx must be an integer"),
                },
            }),
            _ => panic!("unsupported fixture input case for {name}"),
        })
        .collect::<Vec<_>>();
    let expected = array(output.get("Cases").expect("fixture output cases"))
        .iter()
        .map(|value| decode_expected(name, value))
        .collect::<Vec<_>>();
    assert_eq!(
        cases.len(),
        expected.len(),
        "fixture input/output length mismatch for {name}"
    );
    PlanFixture {
        name: name.to_owned(),
        cases,
        expected,
    }
}

/// 按用例名将 JSON 期望解码为类型化 `FixtureExpected`。
fn decode_expected(name: &str, value: &JsonValue) -> FixtureExpected {
    match name {
        "TestUnion" => {
            let value = object(value);
            exact_keys(value, &["Best", "Err"]);
            FixtureExpected::BestError(BestErrorExpected {
                best: string(value.get("Best").expect("union Best")),
                error: match value.get("Err").expect("union Err") {
                    JsonValue::Bool(error) => *error,
                    _ => panic!("union Err must be bool"),
                },
            })
        }
        "TestUniqueKeyInfo" => {
            let value = object(value);
            assert!(value.keys().all(|key| key.parse::<usize>().is_ok()));
            FixtureExpected::UniqueKey(UniqueKeyExpected {
                nodes: value
                    .iter()
                    .map(|(key, value)| {
                        (
                            key.parse::<usize>().expect("unique-key node id"),
                            nested_strings(value),
                        )
                    })
                    .collect(),
            })
        }
        "TestColumnPruning" => {
            let value = object(value);
            assert!(value.keys().all(|key| key.parse::<usize>().is_ok()));
            FixtureExpected::ColumnPruning(ColumnPruningExpected {
                data_sources: value
                    .iter()
                    .map(|(key, value)| {
                        (
                            key.parse::<usize>().expect("column-pruning datasource id"),
                            strings(value),
                        )
                    })
                    .collect(),
            })
        }
        "TestSortByItemsPruning" => FixtureExpected::SortColumns(strings(value)),
        "TestDeriveNotNullConds" => {
            let value = object(value);
            exact_keys(value, &["Left", "Plan", "Right"]);
            FixtureExpected::DeriveNotNull(DeriveNotNullExpected {
                plan: string(value.get("Plan").expect("derive Plan")),
                left: string(value.get("Left").expect("derive Left")),
                right: string(value.get("Right").expect("derive Right")),
            })
        }
        "TestJoinPredicatePushDown" => {
            let value = object(value);
            exact_keys(value, &["Left", "Right"]);
            FixtureExpected::JoinPredicates(JoinPredicatesExpected {
                left: string(value.get("Left").expect("join Left")),
                right: string(value.get("Right").expect("join Right")),
            })
        }
        "TestOuterWherePredicatePushDown" => {
            let value = object(value);
            exact_keys(value, &["Left", "Right", "Sel"]);
            FixtureExpected::OuterPredicates(OuterPredicatesExpected {
                selection: string(value.get("Sel").expect("outer Sel")),
                left: string(value.get("Left").expect("outer Left")),
                right: string(value.get("Right").expect("outer Right")),
            })
        }
        "TestSimplifyOuterJoin" => {
            let value = object(value);
            exact_keys(value, &["Best", "JoinType"]);
            FixtureExpected::SimplifyOuter(SimplifyOuterExpected {
                best: string(value.get("Best").expect("simplify Best")),
                join_type: string(value.get("JoinType").expect("simplify JoinType")),
            })
        }
        "TestPruneColumnsForDelete" => {
            let value = object(value);
            exact_keys(
                value,
                &["FullLayoutInfo", "InsidePlan", "PrunedOutput", "SQL"],
            );
            FixtureExpected::PruneDelete(PruneDeleteExpected {
                sql: string(value.get("SQL").expect("delete SQL")),
                pruned_output: string(value.get("PrunedOutput").expect("delete PrunedOutput")),
                full_layout_info: nested_strings(
                    value.get("FullLayoutInfo").expect("delete FullLayoutInfo"),
                ),
                inside_plan: string(value.get("InsidePlan").expect("delete InsidePlan")),
            })
        }
        _ => FixtureExpected::Plan(string(value)),
    }
}

/// 逻辑计划 ToString，对齐 Go 测试输出格式。
pub(crate) fn logical_plan_string(plan: &dyn logicalop::LogicalPlan) -> String {
    logical_plan_string_inner(plan, false)
}

/// 递归格式化；Apply 右孩子可隐藏 join key 相关 firstrow。
fn logical_plan_string_inner(
    plan: &dyn logicalop::LogicalPlan,
    hide_apply_join_keys: bool,
) -> String {
    let is_apply = plan.as_any().is::<logicalop::LogicalApply>();
    let child_strings = plan
        .Children()
        .iter()
        .enumerate()
        .map(|(index, child)| {
            logical_plan_string_inner(
                child.as_ref(),
                hide_apply_join_keys || (is_apply && index == 1),
            )
        })
        .collect::<Vec<_>>();
    let current = if let Some(source) = plan.as_any().downcast_ref::<logicalop::DataSource>() {
        if source.PartitionDefIdx.is_some() {
            return format!("Partition({})", source.PhysicalTableID);
        }
        let name = source
            .TableAsName
            .as_ref()
            .filter(|name| !name.O.is_empty())
            .unwrap_or(&source.TableInfo.Name);
        format!("DataScan({})", name.O)
    } else if let Some(join) = plan.as_any().downcast_ref::<logicalop::LogicalJoin>() {
        let keys = join
            .EqualConditions
            .iter()
            .filter_map(|condition| condition.as_scalar_function())
            .filter(|condition| condition.GetArgs().len() == 2)
            .map(|condition| {
                let parameters = plan.SCtx().map(|ctx| ctx.GetExprCtx().GetEvalCtx());
                format!(
                    "({},{})",
                    condition.GetArgs()[0].StringWithCtx(parameters.map(|ctx| ctx as _), "OFF"),
                    condition.GetArgs()[1].StringWithCtx(parameters.map(|ctx| ctx as _), "OFF")
                )
            })
            .collect::<String>();
        format!("Join{{{}}}{keys}", child_strings.join("->"))
    } else if plan.as_any().is::<logicalop::LogicalApply>() {
        format!("Apply{{{}}}", child_strings.join("->"))
    } else if let Some(selection) = plan.as_any().downcast_ref::<logicalop::LogicalSelection>() {
        let parameters = plan.SCtx().map(|ctx| ctx.GetExprCtx().GetEvalCtx());
        format!(
            "Sel([{}])",
            selection
                .Conditions
                .iter()
                .map(|condition| condition.StringWithCtx(parameters.map(|ctx| ctx as _), "OFF"))
                .collect::<Vec<_>>()
                .join(" ")
        )
    } else if let Some(aggregation) = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalAggregation>()
    {
        let parameters = plan.SCtx().map(|ctx| ctx.GetExprCtx().GetEvalCtx());
        format!(
            "Aggr({})",
            aggregation
                .AggFuncs
                .iter()
                .filter(|function| {
                    !(hide_apply_join_keys
                        && function.Name.eq_ignore_ascii_case("firstrow")
                        && function.Args.len() == 1
                        && function.Args[0].as_column().is_some_and(|column| {
                            aggregation
                                .GetGroupByCols()
                                .iter()
                                .any(|grouped| grouped.UniqueID == column.UniqueID)
                        }))
                })
                .map(|function| function.StringWithCtx(parameters.map(|ctx| ctx as _), "OFF"))
                .collect::<Vec<_>>()
                .join(",")
        )
    } else if plan.as_any().is::<logicalop::LogicalProjection>() {
        "Projection".to_owned()
    } else if plan.as_any().is::<logicalop::LogicalUnionAll>() {
        format!("UnionAll{{{}}}", child_strings.join("->"))
    } else if plan.as_any().is::<logicalop::LogicalPartitionUnionAll>() {
        format!("PartitionUnionAll{{{}}}", child_strings.join("->"))
    } else if plan.as_any().is::<logicalop::LogicalTableDual>() {
        "Dual".to_owned()
    } else if plan.as_any().is::<logicalop::LogicalSort>() {
        "Sort".to_owned()
    } else if plan.as_any().is::<logicalop::LogicalLimit>() {
        "Limit".to_owned()
    } else if let Some(top_n) = plan.as_any().downcast_ref::<logicalop::LogicalTopN>() {
        let parameters = plan.SCtx().map(|ctx| ctx.GetExprCtx().GetEvalCtx());
        format!(
            "TopN([{}],{},{})",
            top_n
                .ByItems
                .iter()
                .map(|item| {
                    let mut value = item
                        .Expr
                        .StringWithCtx(parameters.map(|ctx| ctx as _), "OFF");
                    if item.Desc {
                        value.push_str(" true");
                    }
                    value
                })
                .collect::<Vec<_>>()
                .join(" "),
            top_n.Offset,
            top_n.Count
        )
    } else if plan.as_any().is::<logicalop::LogicalMaxOneRow>() {
        "MaxOneRow".to_owned()
    } else if let Some(window) = plan.as_any().downcast_ref::<logicalop::LogicalWindow>() {
        let parameters = plan.SCtx().map(|ctx| ctx.GetExprCtx().GetEvalCtx());
        let first_result = plan
            .Schema()
            .Len()
            .saturating_sub(window.WindowFuncDescs.len());
        let functions = window
            .WindowFuncDescs
            .iter()
            .enumerate()
            .map(|(index, function)| {
                let arguments = function
                    .Args
                    .iter()
                    .map(|argument| argument.StringWithCtx(parameters.map(|ctx| ctx as _), "OFF"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let output = plan.Schema().Columns.get(first_result + index).map_or_else(
                    || "Column#0".to_owned(),
                    |column| format!("Column#{}", column.UniqueID),
                );
                format!("{}({arguments})->{output}", function.Name,)
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("Window({functions})")
    } else {
        plan.TP().to_owned()
    };
    if child_strings.is_empty()
        || plan.as_any().is::<logicalop::LogicalJoin>()
        || plan.as_any().is::<logicalop::LogicalApply>()
        || plan.as_any().is::<logicalop::LogicalUnionAll>()
        || plan.as_any().is::<logicalop::LogicalPartitionUnionAll>()
    {
        current
    } else {
        format!("{}->{current}", child_strings.join("->"))
    }
}

#[derive(Clone, Debug, PartialEq)]
/// 辅助套件单条：SQL、是否脱敏日志、期望 Plan 行。
struct AuxiliarySuiteCase {
    sql: String,
    redact_log: bool,
    plan: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
/// 辅助套件（runtime filter / FTS 等）。
struct AuxiliarySuite {
    name: String,
    cases: Vec<AuxiliarySuiteCase>,
}

/// 加载 `{name}_in.json` / `{name}_out.json` 辅助套件。
fn load_auxiliary_suite(file_name: &str) -> AuxiliarySuite {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    let input = read_json(&directory.join(format!("{file_name}_in.json")));
    let output = read_json(&directory.join(format!("{file_name}_out.json")));
    if file_name == "fts_resolve_index_suite" {
        let generated = read_json(&directory.join(format!("{file_name}_xut.json")));
        assert_eq!(output, generated, "{file_name} out/xut mismatch");
    }
    assert_eq!(array(&input).len(), 1, "{file_name} input group count");
    assert_eq!(array(&output).len(), 1, "{file_name} output group count");
    let input = object(&array(&input)[0]);
    let output = object(&array(&output)[0]);
    exact_keys(input, &["Cases", "Name"]);
    exact_keys(output, &["Cases", "Name"]);
    let input_name = string(input.get("Name").expect("auxiliary input Name"));
    let output_name = string(output.get("Name").expect("auxiliary output Name"));
    assert_eq!(input_name, output_name, "{file_name} group name mismatch");
    let input_cases = array(input.get("Cases").expect("auxiliary input Cases"));
    let output_cases = array(output.get("Cases").expect("auxiliary output Cases"));
    assert_eq!(
        input_cases.len(),
        output_cases.len(),
        "{file_name} case count"
    );
    let cases = input_cases
        .iter()
        .zip(output_cases)
        .enumerate()
        .map(|(index, (input, output))| {
            let input = match input {
                JsonValue::String(sql) => AuxiliarySuiteCase {
                    sql: sql.clone(),
                    redact_log: false,
                    plan: Vec::new(),
                },
                JsonValue::Object(input) => {
                    if input.contains_key("RedactLog") {
                        exact_keys(input, &["RedactLog", "SQL"]);
                    } else {
                        exact_keys(input, &["SQL"]);
                    }
                    AuxiliarySuiteCase {
                        sql: string(input.get("SQL").expect("auxiliary input SQL")),
                        redact_log: match input.get("RedactLog") {
                            Some(JsonValue::Bool(value)) => *value,
                            None => false,
                            _ => panic!("auxiliary input RedactLog must be bool"),
                        },
                        plan: Vec::new(),
                    }
                }
                _ => panic!("auxiliary input case {index} must be string/object"),
            };
            let output = object(output);
            if file_name == "fts_resolve_index_suite" {
                exact_keys(output, &["Plan", "RedactLog", "SQL"]);
            } else {
                exact_keys(output, &["Plan", "SQL"]);
            }
            let output_sql = string(output.get("SQL").expect("auxiliary output SQL"));
            let output_redact = match output.get("RedactLog") {
                Some(JsonValue::Bool(value)) => *value,
                None => false,
                _ => panic!("auxiliary output RedactLog must be bool"),
            };
            assert_eq!(input.sql, output_sql, "{file_name} case {index} SQL");
            assert_eq!(
                input.redact_log, output_redact,
                "{file_name} case {index} RedactLog"
            );
            AuxiliarySuiteCase {
                sql: input.sql,
                redact_log: input.redact_log,
                plan: strings(output.get("Plan").expect("auxiliary output Plan")),
            }
        })
        .collect();
    AuxiliarySuite {
        name: input_name,
        cases,
    }
}

/// runtime_filter_generator_suite 数据。
fn get_runtime_filter_generator_data() -> AuxiliarySuite {
    load_auxiliary_suite("runtime_filter_generator_suite")
}

/// fts_resolve_index_suite 数据。
fn get_fts_resolve_index_suite_data() -> AuxiliarySuite {
    load_auxiliary_suite("fts_resolve_index_suite")
}

#[test]
/// 校验主套件分组齐全且辅助套件条数正确。
fn test_main_loads_all_typed_fixture_suites() {
    let expected_groups = [
        "TestEagerAggregation",
        "TestPlanBuilder",
        "TestPredicatePushDown",
        "TestSubquery",
        "TestTopNPushDown",
        "TestUnion",
        "TestWindowFunction",
        "TestWindowParallelFunction",
        "TestUniqueKeyInfo",
        "TestAggPrune",
        "TestColumnPruning",
        "TestSortByItemsPruning",
        "TestDeriveNotNullConds",
        "TestTablePartition",
        "TestJoinPredicatePushDown",
        "TestJoinReOrder",
        "TestOuterJoinEliminator",
        "TestSimplifyOuterJoin",
        "TestOuterWherePredicatePushDown",
        "TestPruneColumnsForDelete",
    ];
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    for (file, key) in [
        ("plan_suite_unexported_in.json", "name"),
        ("plan_suite_unexported_out.json", "Name"),
    ] {
        let root = read_json(&directory.join(file));
        let names = array(&root)
            .iter()
            .map(|group| string(object(group).get(key).expect("fixture group name")))
            .collect::<std::collections::BTreeSet<_>>();
        let expected = expected_groups
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(names, expected, "{file} group names");
    }
    for name in expected_groups {
        let fixture = plan_fixture(name);
        assert_eq!(fixture.name, name);
        assert!(!fixture.cases.is_empty(), "{name} has no SQL cases");
        assert_eq!(fixture.cases.len(), fixture.expected.len());
    }

    for (name, expected_count, suite) in [
        (
            "runtime_filter_generator_suite",
            12,
            get_runtime_filter_generator_data(),
        ),
        (
            "fts_resolve_index_suite",
            8,
            get_fts_resolve_index_suite_data(),
        ),
    ] {
        assert_eq!(suite.cases.len(), expected_count, "{name} case count");
        for (index, case) in suite.cases.iter().enumerate() {
            assert!(!case.sql.is_empty(), "{name} case {index} SQL");
            assert!(!case.plan.is_empty(), "{name} case {index} Plan");
        }
    }
}

#[test]
/// 重复对象键必须解析失败。
fn typed_json_parser_rejects_duplicate_object_keys() {
    let error = JsonParser::new(r#"{"Name":"first","Name":"second"}"#)
        .parse()
        .expect_err("duplicate keys must not be silently overwritten");
    assert!(error.contains("duplicate JSON object key"), "{error}");
}

#[test]
/// 未知字段必须被 exact_keys 拒绝。
fn typed_object_decoder_rejects_unknown_fields() {
    let value = JsonParser::new(r#"{"SQL":"select 1","Unexpected":true}"#)
        .parse()
        .expect("syntactically valid object");
    let error = check_exact_keys(object(&value), &["SQL"])
        .expect_err("unknown fields must not be silently ignored");
    assert!(error.contains("Unexpected"), "{error}");
}

#[test]
fn full_join_rejects_lateral_before_correlated_resolution() {
    let sqls = [
        "select * from t t1 full outer join t t2 on t1.a = t2.a",
        "select * from t t1 full outer join lateral (select 1 as a) as t2 on false",
        "select * from t t1 full outer join lateral (select t1.a) as t2 on true",
        "select * from t t1 full outer join (t t2 join lateral (select t2.a) as t3 on true) on false",
    ];
    for enabled in [false, true] {
        for (index, sql) in sqls.iter().enumerate() {
            // Later full-join planning allows the ordinary ON join when enabled.
            if enabled && index == 0 {
                continue;
            }
            let context = planner_test_context_with_stats_state(0, false, None, false, |vars| {
                vars.SetSystemVar(
                    "tidb_enable_full_outer_join",
                    if enabled { "ON" } else { "OFF" },
                )
                .unwrap();
            });
            crate::InstallPlannerExpressionFactory().unwrap();
            let statement = crate::ast::NodeRef::new(
                parser_dependency::New().ParseOneStmt(sql, "", "").unwrap(),
            );
            let (mut builder, _) = crate::NewPlanBuilder()
                .withDataSourceProvider(Arc::new(PlannerTestStatsProvider))
                .Init(
                    context,
                    planner_test_schema(),
                    hint_dependency::NewQBHintHandler(None),
                );
            let error = builder
                .BuildNodeRef(crate::context::TODO(), &statement)
                .err()
                .expect("FULL OUTER JOIN must be rejected before lateral resolution");
            let expected = plannererrors_dependency::ErrNotSupportedYet
                .GenWithStackByArgs(&["FULL OUTER JOIN".into()]);
            assert!(
                error.Equal(&plannererrors_dependency::ErrNotSupportedYet),
                "enabled={enabled}, {sql}: {error}"
            );
            assert_eq!(error.to_string(), expected.to_string());
        }
    }
}
