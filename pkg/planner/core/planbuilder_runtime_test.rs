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

use base_dependency as base;
use expression_dependency as expression;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

struct TestPlanContext {
    plan_id: AtomicI32,
    session_vars: variable_dependency::session::SessionVars,
    expr_ctx: exprstatic_dependency::ExprContext,
    builtin_function_usage: base::BuiltinFunctionUsageCounter,
}

impl base::PlanContext for TestPlanContext {
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
        panic!("planbuilder runtime test does not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn expression::exprctx::ExprContext {
        &self.expr_ctx
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("planbuilder runtime test does not build protobuf executors")
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.builtin_function_usage.Inc(scalar_func_sig_name)
    }
}

#[test]
fn init_uses_the_session_no_decorrelate_setting_like_go() {
    let mut session_vars = variable_dependency::session::SessionVars::default();
    session_vars.EnableNoDecorrelateInSelect = true;
    let context: base::ContextRef = Arc::new(TestPlanContext {
        plan_id: AtomicI32::new(0),
        session_vars,
        expr_ctx: exprstatic_dependency::NewExprContext(Vec::new()),
        builtin_function_usage: base::BuiltinFunctionUsageCounter::default(),
    });
    let info_schema = infoschema_dependency::infoschema::MockInfoSchema(Vec::new());

    let (builder, _) = crate::NewPlanBuilder().Init(
        context,
        info_schema,
        hint_dependency::NewQBHintHandler(None),
    );

    assert!(builder.noDecorrelate);
}

#[test]
fn pg_dialect_15_planner_uses_exact_physical_identity() {
    use infoschema_dependency::{CiString, DBInfo, Table, infoschema::infoSchema};
    use logicalop_dependency::LogicalPlan;
    crate::InstallPlannerExpressionFactory().unwrap();
    let mut schema = infoSchema::new(1);
    let tables = [(11, "foo"), (12, "Foo")].map(|(id, name)| {
        Table::from_model(model_dependency::TableInfo {
            ID: id,
            DBID: 1,
            Name: crate::ast::NewCIStr(name),
            ExactName: true,
            State: model_dependency::StatePublic,
            Columns: vec![model_dependency::ColumnInfo {
                ID: 1,
                Name: crate::ast::NewCIStr("id"),
                State: model_dependency::StatePublic,
                FieldType: *expression::types::NewFieldType(expression::mysql::TypeLong),
                ..Default::default()
            }],
            ..Default::default()
        })
    });
    schema.add_schema(
        DBInfo {
            id: 1,
            name: CiString::new("names"),
            ..Default::default()
        },
        tables.to_vec(),
    );
    let schema: Arc<dyn infoschema_dependency::InfoSchema> = Arc::new(schema);
    fn source_ids(plan: &dyn LogicalPlan, result: &mut Vec<i64>) {
        if let Some(source) = plan
            .as_any()
            .downcast_ref::<logicalop_dependency::DataSource>()
        {
            result.push(source.PhysicalTableID);
        }
        for child in plan.Children() {
            source_ids(child.as_ref(), result);
        }
    }
    for (dialect, sql, expected) in [
        (
            crate::ast::SqlDialect::PostgreSQL,
            "SELECT id FROM names.foo",
            11,
        ),
        (
            crate::ast::SqlDialect::PostgreSQL,
            "SELECT id FROM names.Foo",
            11,
        ),
        (
            crate::ast::SqlDialect::PostgreSQL,
            "SELECT id FROM names.\"Foo\"",
            12,
        ),
        (
            crate::ast::SqlDialect::MySQL,
            "SELECT id FROM names.Foo",
            11,
        ),
    ] {
        let context: base::ContextRef = Arc::new(TestPlanContext {
            plan_id: AtomicI32::new(0),
            session_vars: Default::default(),
            expr_ctx: exprstatic_dependency::NewExprContext(Vec::new()),
            builtin_function_usage: Default::default(),
        });
        let node = crate::ast::NodeRef::new(
            parser_dependency::Parser::with_dialect(dialect)
                .ParseOneStmt(sql, "", "")
                .unwrap(),
        );
        let (mut builder, _) = crate::NewPlanBuilder().Init(
            context,
            schema.clone(),
            hint_dependency::NewQBHintHandler(None),
        );
        let crate::BuiltRuntimePlan::Logical(plan) =
            builder.BuildNodeRef(crate::context::TODO(), &node).unwrap()
        else {
            panic!("SELECT must build a real plan")
        };
        let mut ids = Vec::new();
        source_ids(plan.as_ref(), &mut ids);
        assert_eq!(ids, vec![expected], "{sql}");
    }
}
