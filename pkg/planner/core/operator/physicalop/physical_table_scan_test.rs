// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::PhysicalTableScan;

struct TestPlanContext(
    AtomicI32,
    base::BuiltinFunctionUsageCounter,
    planctx::variable::SessionVars,
);

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        &self.2
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
        planctx::variable::SessionVars::default(),
    ))
}

#[test]
fn operator_info_omits_empty_range_and_false_desc_like_go() {
    let mut scan = PhysicalTableScan::New(context());
    assert_eq!(scan.OperatorInfo(false), "keep order:false");
    assert_eq!(scan.OperatorInfo(true), "keep order:false");

    scan.Desc = true;
    assert_eq!(scan.OperatorInfo(false), "keep order:false, desc");

    scan.RangeInfo = "outer.id".to_owned();
    assert_eq!(scan.OperatorInfo(true), "keep order:false, desc");
}

#[test]
fn correlated_columns_are_extracted_only_from_access_conditions_like_go() {
    let mut scan = PhysicalTableScan::New(context());
    scan.FilterCondition
        .push(Box::new(expression::CorrelatedColumn {
            column: expression::Column::default(),
            data: None,
        }));

    assert!(scan.ExtractCorrelatedCols().is_empty());

    scan.AccessCondition
        .push(Box::new(expression::CorrelatedColumn {
            column: expression::Column::default(),
            data: None,
        }));
    assert_eq!(scan.ExtractCorrelatedCols().len(), 1);
}

#[test]
fn access_object_uses_partition_name_and_normalizes_it_like_go() {
    let table = model::TableInfo {
        Name: parser_ast::NewCIStr("t"),
        Partition: Some(model::PartitionInfo {
            Definitions: vec![model::PartitionDefinition {
                ID: 11,
                Name: parser_ast::NewCIStr("p0"),
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut scan = PhysicalTableScan::New(context());
    scan.Table = Some(table);
    scan.TableAsName = "alias".to_owned();
    scan.IsPartition = true;
    scan.PhysicalTableID = 11;

    assert_eq!(scan.AccessObject(), "table:alias, partition:p0");
    assert!(
        scan.ExplainNormalizedInfo()
            .starts_with("table:alias, partition:?, ")
    );
}

#[test]
fn signed_integer_handle_does_not_treat_unsigned_domain_as_full_scan() {
    let mut field_type = expression::types::NewFieldType(mysql::r#type::TypeLonglong);
    field_type.AddFlag(mysql::r#type::PriKeyFlag);
    let primary = model::ColumnInfo {
        ID: 1,
        FieldType: *field_type,
        ..Default::default()
    };
    let table = model::TableInfo {
        PKIsHandle: true,
        Columns: vec![primary],
        ..Default::default()
    };
    let unsigned_domain = ranger::Range {
        LowVal: vec![ranger::types::NewUintDatum(0)],
        HighVal: vec![ranger::types::NewUintDatum(u64::MAX)],
        Collators: ranger::collate::GetBinaryCollatorSlice(1),
        ..Default::default()
    };

    let mut scan = PhysicalTableScan::New(context());
    scan.Table = Some(table);
    scan.Ranges = ranger::Ranges(vec![unsigned_domain]);

    assert!(!scan.IsFullScan());
}
