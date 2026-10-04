// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::{BatchPointGetPlan, PointGetPlan};

struct TestPlanContext(AtomicI32, base::BuiltinFunctionUsageCounter);

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }
    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }
    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        std::process::abort()
    }
    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        std::process::abort()
    }
    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        std::process::abort()
    }
    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        std::process::abort()
    }
    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        std::process::abort()
    }
    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.1.Inc(name)
    }
}

fn context() -> base::ContextRef {
    Arc::new(TestPlanContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
    ))
}

#[test]
fn point_get_fixed_cardinality_and_empty_correlations_match_go() {
    let plan = PointGetPlan::New(context());

    assert_eq!(plan.StatsCount(), 1.0);
    assert!(plan.ExtractCorrelatedCols().is_empty());
}

#[test]
fn point_get_operator_info_only_reports_handle_or_lock() {
    let mut plan = PointGetPlan::New(context());

    assert_eq!(plan.OperatorInfo(false), "");
    assert_eq!(plan.OperatorInfo(true), "");
    assert_eq!(plan.ExplainInfo(), "table:unknown");
    plan.Handle = Some(42);
    assert_eq!(plan.OperatorInfo(false), "handle:42");
    assert_eq!(plan.OperatorInfo(true), "handle:?");
    assert_eq!(plan.ExplainNormalizedInfo(), "table:unknown, handle:?");
    plan.Lock = true;
    assert_eq!(plan.OperatorInfo(false), "handle:42, lock");
}

#[test]
fn batch_point_get_operator_info_reports_order_and_lock_flags() {
    let mut plan = BatchPointGetPlan::New(context());

    assert_eq!(
        plan.OperatorInfo(true),
        "handle:?, keep order:false, desc:false"
    );
    assert_eq!(
        plan.ExplainNormalizedInfo(),
        "table:unknown, handle:?, keep order:false, desc:false"
    );
    plan.KeepOrder = true;
    plan.Desc = true;
    plan.Lock = true;
    assert_eq!(
        plan.OperatorInfo(true),
        "handle:?, keep order:true, desc:true, lock"
    );
}

fn common_primary_plan(input: &[Option<&str>]) -> BatchPointGetPlan {
    let column = model::ColumnInfo {
        ID: 1,
        Offset: 0,
        FieldType: *expression::types::NewFieldType(mysql::r#type::TypeVarchar),
        ..Default::default()
    };
    let index = model::IndexInfo {
        ID: 1,
        Primary: true,
        Unique: true,
        Columns: vec![model::IndexColumn {
            Offset: 0,
            Length: -1,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut plan = BatchPointGetPlan::New(context());
    plan.PointGetPlan.TblInfo = Some(model::TableInfo {
        ID: 1,
        Columns: vec![column],
        Indices: vec![index.clone()],
        IsCommonHandle: true,
        ..Default::default()
    });
    plan.PointGetPlan.IndexInfo = Some(index);
    plan.IndexValueRows = input
        .iter()
        .map(|value| {
            vec![value.map_or_else(types::datum::Datum::default, |value| {
                types::datum::NewStringDatum(value.to_owned())
            })]
        })
        .collect();
    plan
}

#[test]
fn common_primary_duplicate_values_keep_first_occurrence() {
    use expression::exprctx::ExprContext;
    let expression_context = exprstatic::NewExprContext(Vec::new());
    let statement_context = stmtctx::NewStmtCtx();
    let pruning = crate::BatchPointGetPruningContext {
        StatementContext: &statement_context,
        EvalContext: expression_context.GetEvalCtx(),
        PartitionedTable: None,
        SinglePartition: None,
        PartitionNames: &[],
        HandleColOffset: 0,
    };
    let cases: &[(&[Option<&str>], &[&str])] = &[
        (&[], &[]),
        (&[Some("a"), Some("b"), Some("c")], &["a", "b", "c"]),
        (
            &[Some("b"), Some("a"), Some("b"), None, Some("c"), Some("a")],
            &["b", "a", "c"],
        ),
        (&[Some("a"), Some("a"), Some("a")], &["a"]),
        (&[None, None], &[]),
    ];
    for (input, expected) in cases {
        let mut plan = common_primary_plan(input);
        let (handles, dual) = plan.PrunePartitionsAndValues(&pruning).unwrap();
        assert!(!dual);
        assert_eq!(handles.len(), expected.len());
        assert_eq!(plan.IndexValueRows.len(), expected.len());
        assert!(
            plan.Handles.is_empty(),
            "Go returns common handles without storing integer handles"
        );
        for ((row, handle), expected) in plan.IndexValueRows.iter().zip(&handles).zip(*expected) {
            assert_eq!(row[0].ToString().unwrap(), *expected);
            assert!(!handle.IsInt());
            let encoded =
                kv::codec::EncodeKey(statement_context.TimeZone(), vec![], row.clone()).unwrap();
            let expected_handle = kv::NewCommonHandle(encoded).unwrap();
            assert_eq!(handle.Encoded(), kv::Handle::Encoded(&expected_handle));
        }
    }
}

#[test]
fn common_handle_partition_compaction_keeps_handles_and_values_aligned() {
    let mut plan = common_primary_plan(&[Some("b"), Some("a"), Some("c")]);
    let statement_context = stmtctx::NewStmtCtx();
    let mut handles: Vec<Box<dyn kv::Handle>> = plan
        .IndexValueRows
        .iter()
        .map(|row| {
            Box::new(
                kv::NewCommonHandle(
                    kv::codec::EncodeKey(statement_context.TimeZone(), vec![], row.clone())
                        .unwrap(),
                )
                .unwrap(),
            ) as Box<dyn kv::Handle>
        })
        .collect();
    let retained_key = handles[2].Encoded();
    let info = model::PartitionInfo {
        Definitions: vec![
            model::PartitionDefinition {
                Name: parser_ast::NewCIStr("p0"),
                ..Default::default()
            },
            model::PartitionDefinition {
                Name: parser_ast::NewCIStr("p1"),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let found = crate::physical_batch_point_get::compact_batch_partition_values(
        &mut plan.IndexValueRows,
        &mut handles,
        &[-1, 0, 1],
        &mut plan.PartitionIdxs,
        None,
        &info,
        &[parser_ast::NewCIStr("P1")],
    );
    assert_eq!(found, 1);
    assert_eq!(plan.PartitionIdxs, vec![1]);
    assert_eq!(plan.IndexValueRows[0][0].ToString().unwrap(), "c");
    assert_eq!(handles[0].Encoded(), retained_key);
}

#[test]
fn integer_dedup_and_secondary_null_filter_preserve_order() {
    use expression::exprctx::ExprContext;
    let expression_context = exprstatic::NewExprContext(Vec::new());
    let statement_context = stmtctx::NewStmtCtx();
    let pruning = crate::BatchPointGetPruningContext {
        StatementContext: &statement_context,
        EvalContext: expression_context.GetEvalCtx(),
        PartitionedTable: None,
        SinglePartition: None,
        PartitionNames: &[],
        HandleColOffset: 0,
    };
    let mut plan = common_primary_plan(&[]);
    plan.PointGetPlan.IndexInfo = None;
    plan.Handles = vec![3, 1, 3, 2, 1];
    let (handles, dual) = plan.PrunePartitionsAndValues(&pruning).unwrap();
    assert!(!dual);
    assert_eq!(plan.Handles, vec![3, 1, 2]);
    assert_eq!(
        handles
            .iter()
            .map(|handle| handle.IntValue())
            .collect::<Vec<_>>(),
        vec![3, 1, 2]
    );
    let mut plan = common_primary_plan(&[Some("b"), None, Some("b"), Some("a")]);
    plan.PointGetPlan.IndexInfo.as_mut().unwrap().Primary = false;
    let (handles, dual) = plan.PrunePartitionsAndValues(&pruning).unwrap();
    assert!(!dual);
    assert!(
        handles.is_empty(),
        "secondary indexes retain duplicate values as in Go"
    );
    assert_eq!(
        plan.IndexValueRows
            .iter()
            .map(|row| row[0].ToString().unwrap())
            .collect::<Vec<_>>(),
        vec!["b", "b", "a"]
    );
}

#[test]
fn common_primary_enum_missing_values_and_conversion_errors() {
    use expression::exprctx::ExprContext;
    let expression_context = exprstatic::NewExprContext(Vec::new());
    let statement_context = stmtctx::NewStmtCtx();
    let pruning = crate::BatchPointGetPruningContext {
        StatementContext: &statement_context,
        EvalContext: expression_context.GetEvalCtx(),
        PartitionedTable: None,
        SinglePartition: None,
        PartitionNames: &[],
        HandleColOffset: 0,
    };
    let mut plan = common_primary_plan(&[Some("a"), Some("missing"), Some("a"), Some("b")]);
    let column = &mut plan.PointGetPlan.TblInfo.as_mut().unwrap().Columns[0];
    column.FieldType = *expression::types::NewFieldType(mysql::r#type::TypeEnum);
    column
        .FieldType
        .SetElems(vec!["a".to_owned(), "b".to_owned()]);
    let (handles, dual) = plan.PrunePartitionsAndValues(&pruning).unwrap();
    assert!(!dual);
    assert_eq!(handles.len(), 2);
    assert_eq!(
        plan.IndexValueRows[0][0].Kind(),
        types::datum::KindMysqlEnum
    );
    assert_eq!(plan.IndexValueRows[1][0].GetMysqlEnum().Name, "b");
    let mut plan = common_primary_plan(&[Some("a")]);
    plan.PointGetPlan
        .IndexInfo
        .as_mut()
        .unwrap()
        .Columns
        .push(model::IndexColumn::default());
    assert!(plan.PrunePartitionsAndValues(&pruning).is_err());
}

#[test]
fn single_partition_and_empty_compaction_release_rejected_rows() {
    let info = model::PartitionInfo::default();
    let mut plan = common_primary_plan(&[Some("a"), Some("b"), Some("c")]);
    let mut handles = Vec::new();
    plan.PartitionIdxs = vec![1];
    let found = crate::physical_batch_point_get::compact_batch_partition_values(
        &mut plan.IndexValueRows,
        &mut handles,
        &[0, 1, -1],
        &mut plan.PartitionIdxs,
        Some(1),
        &info,
        &[],
    );
    assert_eq!(found, 1);
    assert_eq!(plan.PartitionIdxs, vec![1]);
    assert_eq!(plan.IndexValueRows[0][0].ToString().unwrap(), "b");
    let found = crate::physical_batch_point_get::compact_batch_partition_values(
        &mut plan.IndexValueRows,
        &mut handles,
        &[-1],
        &mut plan.PartitionIdxs,
        Some(1),
        &info,
        &[],
    );
    assert_eq!(found, 0);
    assert!(plan.IndexValueRows.is_empty());
}
