// Copyright 2026 AsterSQL.
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

use std::sync::Mutex;

use super::*;

#[derive(Default)]
struct ObserveMarkersRuntime {
    markers_seen_by_build: Mutex<Vec<PlanCacheParamMarker>>,
}

impl PlanCachePrepareRuntime for ObserveMarkersRuntime {
    fn Preprocess(&self, _: &PlanCachePrepareInput) -> Result<(), PlanCacheError> {
        Ok(())
    }

    fn Build(
        &self,
        input: &PlanCachePrepareInput,
    ) -> Result<Box<dyn base_dependency::Plan>, PlanCacheError> {
        *self.markers_seen_by_build.lock().unwrap() = input.markers.clone();
        Err(PlanCacheError::new("stop after observing markers"))
    }

    fn CheckPreparedPrivileges(&self, _: &PlanCacheStmt) -> Result<(), PlanCacheError> {
        Ok(())
    }

    fn AppendWarning(&self, _: &str) {}
}

#[test]
fn non_prepared_parameters_keep_values_when_sorted_before_build() {
    let runtime = ObserveMarkersRuntime::default();
    let input = PlanCachePrepareInput {
        is_prepared_statement: false,
        markers: vec![
            PlanCacheParamMarker {
                offset: 20,
                datum: Some(crate::Datum::Int(22)),
                in_execute: true,
                ..Default::default()
            },
            PlanCacheParamMarker {
                offset: 10,
                datum: Some(crate::Datum::Int(11)),
                in_execute: true,
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    assert!(GeneratePlanCacheStmtWithAST(&runtime, input).is_err());
    let markers = runtime.markers_seen_by_build.lock().unwrap();
    assert_eq!(markers[0].offset, 10);
    assert_eq!(markers[0].order, 0);
    assert_eq!(markers[0].datum, Some(crate::Datum::Int(11)));
    assert!(markers[0].in_execute);
    assert_eq!(markers[1].datum, Some(crate::Datum::Int(22)));
}

struct BindingPlanContext {
    vars: variable_dependency::session::SessionVars,
    expr: exprstatic_dependency::ExprContext,
}
impl Default for BindingPlanContext {
    fn default() -> Self {
        Self {
            vars: Default::default(),
            expr: exprstatic_dependency::NewExprContext(Vec::new()),
        }
    }
}
impl base_dependency::PlanContext for BindingPlanContext {
    fn BuiltinFunctionUsageInc(&self, _: &str) {
        panic!("metadata capture does not evaluate functions")
    }
    fn alloc_plan_id(&self) -> i32 {
        self.vars.AllocNewPlanID()
    }
    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }
    fn GetSessionVars(&self) -> &variable_dependency::session::SessionVars {
        &self.vars
    }
    fn GetExprCtx(&self) -> &dyn expression_dependency::exprctx::ExprContext {
        &self.expr
    }
    fn GetNullRejectCheckExprCtx(&self) -> &dyn expression_dependency::exprctx::ExprContext {
        &self.expr
    }
    fn GetRangerCtx(&self) -> &base_dependency::RangerContext<'_> {
        panic!("metadata capture does not build ranges")
    }
    fn GetBuildPBCtx(&self) -> &base_dependency::BuildPBContext {
        panic!("metadata capture does not build protobuf")
    }
}
struct PrepareBindingRuntime;

impl PlanCachePrepareRuntime for PrepareBindingRuntime {
    fn Preprocess(&self, _: &PlanCachePrepareInput) -> Result<(), PlanCacheError> {
        Ok(())
    }
    fn Build(
        &self,
        _: &PlanCachePrepareInput,
    ) -> Result<Box<dyn base_dependency::Plan>, PlanCacheError> {
        Ok(Box::new(physicalop_dependency::BasePhysicalPlan::New(
            std::sync::Arc::new(BindingPlanContext::default()),
            "TableDual",
            0,
        )))
    }
    fn CheckPreparedPrivileges(&self, _: &PlanCacheStmt) -> Result<(), PlanCacheError> {
        Ok(())
    }
    fn AppendWarning(&self, _: &str) {}
}

#[test]
fn prepared_binding_key_is_captured_before_build_and_without_plan_cache() {
    let query = "select hour(`d`) as `hour` from test.t_issue57992 group by `hour`";
    for enabled in [false, true] {
        let input = PlanCachePrepareInput {
            is_prepared_statement: true,
            prepared_ast: ast::misc::Prepared {
                stmt: query.into(),
                stmt_type: "Select".into(),
            },
            parameterized_sql: query.into(),
            prepared_cache_enabled: enabled,
            ast_cacheable: true,
            ..Default::default()
        };
        let (statement, _, count) =
            GeneratePlanCacheStmtWithAST(&PrepareBindingRuntime, input).unwrap();
        assert_eq!(count, 0);
        assert!(!statement.BindingInfo.normalized_sql.is_empty());
        assert!(statement.BindingInfo.digest.is_some());
        let original = astersql_bindinfo::Statement {
            SQL: query.into(),
            ..Default::default()
        };
        assert_eq!(
            statement.BindingInfo.match_info.NoDBDigest,
            astersql_bindinfo::NormalizeStmtForBinding(&original, "", true).1
        );
        assert_eq!(
            statement.BindingInfo.match_info.TableNames,
            vec![astersql_bindinfo::TableName {
                Schema: "test".into(),
                Name: "t_issue57992".into(),
                Alias: String::new()
            }]
        );
        assert!(!statement.BindingInfo.normalized_sql.contains("`test`."));
    }
    let (statement, _, _) = GeneratePlanCacheStmtWithAST(
        &PrepareBindingRuntime,
        PlanCachePrepareInput {
            prepared_ast: ast::misc::Prepared {
                stmt: query.into(),
                stmt_type: "Select".into(),
            },
            parameterized_sql: query.into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(statement.BindingInfo, PlanCacheBindingInfo::default());
}
