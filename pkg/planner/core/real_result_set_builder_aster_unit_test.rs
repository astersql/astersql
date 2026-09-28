// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 真实结果集构建器（result-set builder）单元测试。
//
// 验证默认 PlanBuilder 能将 parser AST 建成逻辑计划管道
//（TableDual/DataSource → Selection → Projection → Sort → Limit），
// 以及通配符展开与可注入 DataSourceProvider 的行为。

use base_dependency as base;
use expression_dependency as expression;
use logicalop_dependency as logicalop;
use logicalop_dependency::LogicalPlan as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

/// 测试用 PlanContext：分配计划 ID 并提供会话/表达式上下文。
struct TestPlanContext {
    /// 计划 ID 原子计数器。
    plan_id: AtomicI32,
    /// 会话变量。
    session_vars: variable_dependency::session::SessionVars,
    /// 表达式上下文。
    expr_ctx: exprstatic_dependency::ExprContext,
    /// 内建函数使用计数器。
    builtin_function_usage: base::BuiltinFunctionUsageCounter,
}

impl base::PlanContext for TestPlanContext {
    /// 分配递增的计划节点 ID。
    fn alloc_plan_id(&self) -> i32 {
        self.plan_id.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// EXPLAIN ID 是否忽略后缀。
    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    /// 返回会话变量。
    fn GetSessionVars(&self) -> &variable_dependency::session::SessionVars {
        &self.session_vars
    }

    /// 返回表达式求值上下文。
    fn GetExprCtx(&self) -> &dyn expression::exprctx::ExprContext {
        &self.expr_ctx
    }

    /// 返回 Ranger 上下文（本测试不构建 range）。
    fn GetRangerCtx(&self) -> &base::RangerContext<'_> {
        panic!("result-set builder test does not build ranges")
    }

    /// 返回空值拒绝检查用表达式上下文。
    fn GetNullRejectCheckExprCtx(&self) -> &dyn expression::exprctx::ExprContext {
        &self.expr_ctx
    }

    /// 返回构建 protobuf 执行器上下文（本测试不用）。
    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("result-set builder test does not build protobuf executors")
    }

    /// 累计内建函数使用计数。
    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.builtin_function_usage.Inc(scalar_func_sig_name)
    }
}

/// 创建默认测试 PlanContext（当前库 test）。
fn plan_context() -> base::ContextRef {
    let mut session_vars = variable_dependency::session::SessionVars::default();
    session_vars.SetCurrentDB("test");
    Arc::new(TestPlanContext {
        plan_id: AtomicI32::new(0),
        session_vars,
        expr_ctx: exprstatic_dependency::NewExprContext(Vec::new()),
        builtin_function_usage: base::BuiltinFunctionUsageCounter::default(),
    })
}

/// 构造含表 t(a,b) 及隐藏 handle/commitTS 列的 Mock InfoSchema。
fn table_info_schema() -> Arc<dyn infoschema_dependency::infoschema::InfoSchema> {
    let model = Arc::new(expression::model::TableInfo {
        ID: 41,
        Name: crate::ast::NewCIStr("t"),
        Columns: vec![
            expression::model::ColumnInfo {
                ID: 1,
                Name: crate::ast::NewCIStr("a"),
                Offset: 0,
                State: expression::model::StatePublic,
                FieldType: *expression::types::NewFieldType(expression::mysql::TypeLonglong),
                ..Default::default()
            },
            expression::model::ColumnInfo {
                ID: 2,
                Name: crate::ast::NewCIStr("b"),
                Offset: 1,
                State: expression::model::StatePublic,
                FieldType: *expression::types::NewFieldType(expression::mysql::TypeLonglong),
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    infoschema_dependency::infoschema::MockInfoSchema(vec![
        infoschema_dependency::infoschema::TableInfo {
            id: model.ID,
            name: infoschema_dependency::infoschema::CiString::new("t"),
            columns: vec![
                infoschema_dependency::infoschema::ColumnInfo {
                    id: 1,
                    name: infoschema_dependency::infoschema::CiString::new("a"),
                    ..Default::default()
                },
                infoschema_dependency::infoschema::ColumnInfo {
                    id: 2,
                    name: infoschema_dependency::infoschema::CiString::new("b"),
                    ..Default::default()
                },
            ],
            model_meta: Some(model),
            ..Default::default()
        },
    ])
}

/// 解析单条 SQL 为 AST NodeRef。
fn parse(sql: &str) -> crate::ast::NodeRef {
    crate::ast::NodeRef::new(
        parser_dependency::New()
            .ParseOneStmt(sql, "", "")
            .unwrap_or_else(|error| panic!("parse {sql:?}: {error}")),
    )
}

#[test]
/// 字面量 SELECT 应得到 Projection over TableDual。
fn real_result_set_builder_builds_select_literal_from_parser_ast() {
    let context = plan_context();
    let info_schema = infoschema_dependency::infoschema::MockInfoSchema(Vec::new());
    let (mut builder, _) = crate::NewPlanBuilder().Init(
        context,
        info_schema,
        hint_dependency::NewQBHintHandler(None),
    );

    let plan = builder
        .buildResultSetNode(crate::context::TODO(), &parse("select 1"), false)
        .expect("the default PlanBuilder must install the real AST result-set builder");
    let projection = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalProjection>()
        .expect("SELECT fields become a LogicalProjection");
    assert_eq!(projection.Exprs.len(), 1);
    assert!(
        projection.Children()[0]
            .as_any()
            .is::<logicalop::LogicalTableDual>()
    );
}

#[test]
/// 解析表并构建 WHERE/ORDER BY/LIMIT 的逻辑管道。
fn real_result_set_builder_resolves_table_and_builds_select_pipeline() {
    let context = plan_context();
    let (mut builder, _) = crate::NewPlanBuilder().Init(
        context,
        table_info_schema(),
        hint_dependency::NewQBHintHandler(None),
    );

    let plan = builder
        .buildResultSetNode(
            crate::context::TODO(),
            &parse("select a from t where b > 3 order by a desc limit 2 offset 1"),
            false,
        )
        .expect("the parser AST must become a logical SELECT pipeline");
    let limit = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalLimit>()
        .expect("LIMIT is the root");
    assert_eq!((limit.Offset, limit.Count), (1, 2));
    let sort = limit.Children()[0]
        .as_any()
        .downcast_ref::<logicalop::LogicalSort>()
        .expect("ORDER BY is below LIMIT");
    assert_eq!(sort.ByItems.len(), 1);
    assert!(sort.ByItems[0].Desc);
    let projection = sort.Children()[0]
        .as_any()
        .downcast_ref::<logicalop::LogicalProjection>()
        .expect("SELECT fields are below ORDER BY");
    assert_eq!(projection.Exprs.len(), 1);
    assert_eq!(
        projection.OutputNames().0[0].as_ref().unwrap().ColName.L,
        "a"
    );
    let selection = projection.Children()[0]
        .as_any()
        .downcast_ref::<logicalop::LogicalSelection>()
        .expect("WHERE is below projection");
    assert_eq!(selection.Conditions.len(), 1);
    let source = selection.Children()[0]
        .as_any()
        .downcast_ref::<logicalop::DataSource>()
        .expect("FROM resolves to DataSource");
    assert_eq!(source.TableInfo.ID, 41);
    assert_eq!(
        source
            .Schema()
            .Columns
            .iter()
            .filter(|column| !column.IsHidden)
            .count(),
        2
    );
    assert_eq!(source.Schema().Len(), 4);
    assert_eq!(source.TblColsByID.len(), 4);
    assert!(
        source
            .Schema()
            .Columns
            .iter()
            .any(|column| { column.ID == expression::model::ExtraHandleID && column.IsHidden })
    );
    assert!(
        source
            .Schema()
            .Columns
            .iter()
            .any(|column| { column.ID == expression::model::ExtraCommitTSID && column.IsHidden })
    );
    assert_eq!(source.AllPossibleAccessPaths.len(), 1);
    assert!(source.AllPossibleAccessPaths[0].IsIntHandlePath);
}

/// 测试 DataSourceProvider：写入固定行数并计数调用。
struct TestDataSourceProvider {
    /// 注入的行数估计。
    row_count: f64,
    /// Populate 调用次数。
    calls: AtomicI32,
}

impl crate::DataSourceProvider for TestDataSourceProvider {
    /// 填充 DataSource 统计与访问路径代价估计。
    fn Populate(
        &self,
        _ctx: &dyn crate::context::Context,
        _plan_ctx: &base::ContextRef,
        _info_schema: &dyn infoschema_dependency::infoschema::InfoSchema,
        table: &crate::ast::TableName,
        source: &mut logicalop::DataSource,
    ) -> Result<(), expression::Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(table.Name.L, "t");
        source.TableStats.RowCount = self.row_count;
        source.AllPossibleAccessPaths[0].CountAfterAccess = self.row_count;
        source.PossibleAccessPaths[0].CountAfterAccess = self.row_count;
        Ok(())
    }
}

#[test]
/// 通配符展开为显式投影，并调用 DataSourceProvider；ResetForReuse 保留 provider。
fn real_result_set_builder_expands_wildcard_and_calls_data_source_provider() {
    let context = plan_context();
    let provider = Arc::new(TestDataSourceProvider {
        row_count: 42.0,
        calls: AtomicI32::new(0),
    });
    let (mut builder, _) = crate::NewPlanBuilder()
        .withDataSourceProvider(provider.clone())
        .Init(
            context,
            table_info_schema(),
            hint_dependency::NewQBHintHandler(None),
        );

    let plan = builder
        .buildResultSetNode(crate::context::TODO(), &parse("select * from t"), false)
        .expect("wildcard expansion uses resolved table columns");
    let projection = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalProjection>()
        .expect("SELECT wildcard remains an explicit logical projection");
    assert_eq!(projection.Exprs.len(), 2);
    let source = projection.Children()[0]
        .as_any()
        .downcast_ref::<logicalop::DataSource>()
        .expect("projection reads the resolved table");
    assert_eq!(source.TableStats.RowCount, 42.0);
    assert_eq!(source.AllPossibleAccessPaths[0].CountAfterAccess, 42.0);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);

    let (mut reused, _) = builder.ResetForReuse().Init(
        plan_context(),
        table_info_schema(),
        hint_dependency::NewQBHintHandler(None),
    );
    reused
        .buildResultSetNode(crate::context::TODO(), &parse("select a from t"), false)
        .expect("ResetForReuse retains the stateful provider instance");
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
}
