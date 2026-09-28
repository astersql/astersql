// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::PhysicalIndexScan;

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
        panic!("physical index scan parity test does not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        std::process::abort()
    }

    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        std::process::abort()
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.1.Inc(scalar_func_sig_name)
    }
}

fn context() -> base::ContextRef {
    Arc::new(TestPlanContext(
        AtomicI32::new(0),
        base::BuiltinFunctionUsageCounter::default(),
    ))
}

#[test]
fn operator_info_omits_empty_range_and_false_desc_like_go() {
    let mut scan = PhysicalIndexScan::New(context());
    assert_eq!(scan.OperatorInfo(false), "keep order:false");
    assert_eq!(scan.OperatorInfo(true), "keep order:false");

    scan.Desc = true;
    assert_eq!(scan.OperatorInfo(false), "keep order:false, desc");
}

#[test]
fn correlated_columns_are_extracted_only_from_access_conditions_like_go() {
    let mut scan = PhysicalIndexScan::New(context());
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
fn access_object_includes_alias_partition_and_index_columns_like_go() {
    let visible = model::ColumnInfo {
        Name: parser_ast::NewCIStr("a"),
        ..Default::default()
    };
    let hidden = model::ColumnInfo {
        Name: parser_ast::NewCIStr("hidden"),
        Hidden: true,
        GeneratedExprString: "lower(a)".to_owned(),
        ..Default::default()
    };
    let table = model::TableInfo {
        Name: parser_ast::NewCIStr("t"),
        Columns: vec![visible, hidden],
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
    let index = model::IndexInfo {
        Name: parser_ast::NewCIStr("idx"),
        Columns: vec![
            model::IndexColumn {
                Name: parser_ast::NewCIStr("a"),
                Offset: 0,
                ..Default::default()
            },
            model::IndexColumn {
                Name: parser_ast::NewCIStr("hidden"),
                Offset: 1,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let mut scan = PhysicalIndexScan::New(context());
    scan.Table = Some(table);
    scan.Index = Some(index);
    scan.TableAsName = "alias".to_owned();
    scan.IsPartition = true;
    scan.PhysicalTableID = 11;

    assert_eq!(
        scan.AccessObject(),
        "table:alias, partition:p0, index:idx(a, lower(a))"
    );
    assert!(
        scan.ExplainNormalizedInfo()
            .starts_with("table:alias, partition:?, index:idx(a, lower(a)), ")
    );
}

#[test]
fn index_scan_pb_contains_schema_column_metadata_like_go() {
    let mut field_type = expression::types::NewFieldType(mysql::r#type::TypeLonglong);
    field_type.AddFlag(mysql::r#type::UnsignedFlag);
    let column_info = model::ColumnInfo {
        ID: 7,
        Name: parser_ast::NewCIStr("a"),
        FieldType: (*field_type).clone(),
        ..Default::default()
    };
    let table = model::TableInfo {
        ID: 42,
        Columns: vec![column_info],
        ..Default::default()
    };
    let index = model::IndexInfo {
        ID: 9,
        Columns: vec![model::IndexColumn {
            Name: parser_ast::NewCIStr("a"),
            Offset: 0,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut scan = PhysicalIndexScan::New(context());
    scan.Table = Some(table);
    scan.Index = Some(index);
    scan.PhysicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![expression::Column::new(
            (*field_type).clone(),
            7,
            70,
            0,
        )]));
    let expression: Arc<dyn planctx::exprctx::BuildContext> =
        Arc::new(exprstatic::NewExprContext(Vec::new()));
    let mut build_context = base::BuildPBContext {
        ExprCtx: expression,
        Client: None,
        TiFlashFastScan: false,
        TiFlashFineGrainedShuffleBatchSize: 0,
        GroupConcatMaxLen: 0,
        InExplainStmt: false,
        WarnHandler: None,
        ExtraWarnghandler: None,
    };

    let executor = scan
        .ToPB(&mut build_context, kv::StoreType::TiKV)
        .expect("encode index scan");
    let encoded = executor.get_idx_scan();
    assert_eq!(encoded.get_table_id(), 42);
    assert_eq!(encoded.get_index_id(), 9);
    assert_eq!(encoded.get_columns().len(), 1);
    assert_eq!(encoded.get_columns()[0].get_column_id(), 7);
    assert_ne!(
        encoded.get_columns()[0].get_flag() & mysql::r#type::UnsignedFlag as i32,
        0
    );
}

#[test]
fn init_schema_preserves_prebuilt_index_columns_like_go() {
    let table = model::TableInfo {
        Columns: vec![
            model::ColumnInfo {
                ID: 1,
                ..Default::default()
            },
            model::ColumnInfo {
                ID: 2,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let index = model::IndexInfo {
        Columns: vec![
            model::IndexColumn {
                Offset: 0,
                ..Default::default()
            },
            model::IndexColumn {
                Offset: 1,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let mut scan = PhysicalIndexScan::New(context());
    scan.Table = Some(table);
    scan.Index = Some(index);
    scan.IdxCols = vec![expression::Column::new(
        expression::types::FieldType::default(),
        1,
        10,
        0,
    )];
    scan.InitSchema(
        &[
            Some(expression::Column::new(
                expression::types::FieldType::default(),
                1,
                999,
                0,
            )),
            Some(expression::Column::new(
                expression::types::FieldType::default(),
                2,
                20,
                1,
            )),
        ],
        false,
    );

    let schema = scan.PhysicalSchemaProducer.SchemaRef().unwrap();
    assert_eq!(schema.Columns.len(), 2);
    assert_eq!(schema.Columns[0].UniqueID, 10);
    assert_eq!(schema.Columns[1].UniqueID, 20);
}
