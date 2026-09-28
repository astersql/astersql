// Copyright 2026 AsterSQL.

use crate::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

fn planner_column(id: i64, unique_id: i64) -> Column {
    Column::new(
        *expression::types::NewFieldType(mysql::r#type::TypeLonglong),
        id,
        unique_id,
        0,
    )
}

#[test]
fn non_pk_table_returns_extra_handle_column() {
    let extra_handle = planner_column(model::ExtraHandleID, 101);
    let mut source = DataSource::default();
    source.Columns = vec![model::NewExtraHandleColInfo()];
    source
        .LogicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![extra_handle.clone()]));

    let actual = source
        .GetPKIsHandleCol()
        .expect("non-PK tables expose the extra handle column");

    assert_eq!(actual.ID, extra_handle.ID);
    assert_eq!(actual.UniqueID, extra_handle.UniqueID);
}

#[test]
fn pruning_all_columns_keeps_non_pk_row_handle() {
    let id = planner_column(1, 100);
    let row_id = planner_column(model::ExtraHandleID, 101);
    let mut source = DataSource::default();
    source.Columns = vec![
        model::ColumnInfo::New(1, parser_ast::NewCIStr("id")),
        model::NewExtraHandleColInfo(),
    ];
    source.HandleCols = Some(planner_util::NewIntHandleCols(row_id.clone()));
    source
        .LogicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![id, row_id]));

    source.PruneColumns(&[]).unwrap();

    assert_eq!(source.Schema().Columns.len(), 1);
    assert_eq!(source.Schema().Columns[0].ID, model::ExtraHandleID);
    assert_eq!(source.Columns[0].ID, model::ExtraHandleID);
}

struct RangeTestContext {
    next_id: AtomicI32,
    vars: planctx::variable::SessionVars,
    expr: exprstatic::ExprContext,
    ranger: base::RangerContext<'static>,
    usage: base::BuiltinFunctionUsageCounter,
}

impl base::PlanContext for RangeTestContext {
    fn alloc_plan_id(&self) -> i32 {
        self.next_id.fetch_add(1, Ordering::SeqCst) + 1
    }
    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }
    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        &self.vars
    }
    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        &self.expr
    }
    fn GetRangerCtx(&self) -> &base::RangerContext<'_> {
        &self.ranger
    }
    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        &self.expr
    }
    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("unused")
    }
    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.usage.Inc(name)
    }
}

#[test]
fn index_range_fallback_preserves_non_index_residuals() {
    let expr = exprstatic::NewExprContext(Vec::new());
    let ranger = base::RangerContext {
        TypeCtx: expression::types::DefaultStmtNoWarningContext.clone(),
        ErrCtx: expression::errctx::StrictNoWarningContext.clone(),
        ExprCtx: Arc::new(exprstatic::NewExprContext(Vec::new())),
        RangeFallbackHandler: None,
        PlanCacheTracker: None,
        OptimizerFixControl: Default::default(),
        UseCache: false,
        RegardNULLAsPoint: true,
        OptPrefixIndexSingleScan: false,
    };
    let context: base::ContextRef = Arc::new(RangeTestContext {
        next_id: AtomicI32::new(0),
        vars: planctx::variable::SessionVars::default(),
        expr,
        ranger,
        usage: base::BuiltinFunctionUsageCounter::default(),
    });
    let mut a = planner_column(1, 101);
    a.OrigName = "a".to_owned();
    let mut b = planner_column(2, 102);
    b.OrigName = "b".to_owned();
    let mut c = planner_column(3, 103);
    c.OrigName = "c".to_owned();
    let mut source = DataSource::default().Init(context.clone(), 0);
    source.TableInfo.Columns = vec![
        model::ColumnInfo::New(1, parser_ast::NewCIStr("a")),
        model::ColumnInfo::New(2, parser_ast::NewCIStr("b")),
        model::ColumnInfo::New(3, parser_ast::NewCIStr("c")),
    ];
    for (offset, column) in source.TableInfo.Columns.iter_mut().enumerate() {
        column.Offset = offset as isize;
    }
    source.Columns = source.TableInfo.Columns.clone();
    source
        .LogicalSchemaProducer
        .SetSchema(expression::NewSchema(vec![a.clone(), b.clone(), c.clone()]));
    source.TableStats.RowCount = 1000.0;
    source.PossibleAccessPaths = vec![planner_util::AccessPath {
        Index: Some(model::IndexInfo {
            Columns: vec![
                model::IndexColumn {
                    Name: parser_ast::NewCIStr("a"),
                    Offset: 0,
                    Length: expression::types::UnspecifiedLength as isize,
                    UseChangingType: false,
                },
                model::IndexColumn {
                    Name: parser_ast::NewCIStr("b"),
                    Offset: 1,
                    Length: expression::types::UnspecifiedLength as isize,
                    UseChangingType: false,
                },
            ],
            ..Default::default()
        }),
        ..Default::default()
    }];
    let predicate = |column: &Column, value: i64| -> Expression {
        expression::NewFunctionInternal(
            context.GetExprCtx(),
            parser_ast::GT,
            *expression::types::NewFieldType(mysql::r#type::TypeTiny),
            vec![
                Box::new(column.clone()),
                Box::new(expression::Constant::with_type(
                    expression::types::NewIntDatum(value),
                    *expression::types::NewFieldType(mysql::r#type::TypeLonglong),
                )),
            ],
        )
        .expect("range predicate")
    };
    let cast_a = expression::NewFunctionInternal(
        context.GetExprCtx(),
        expression::ast::Cast,
        *expression::types::NewFieldType(mysql::r#type::TypeDouble),
        vec![Box::new(a.clone())],
    )
    .expect("numeric cast");
    let cast_range = expression::NewFunctionInternal(
        context.GetExprCtx(),
        parser_ast::GT,
        *expression::types::NewFieldType(mysql::r#type::TypeTiny),
        vec![
            cast_a,
            Box::new(expression::Constant::with_type(
                expression::types::NewIntDatum(1),
                *expression::types::NewFieldType(mysql::r#type::TypeLonglong),
            )),
        ],
    )
    .expect("cast range predicate");
    source.AllConds = vec![cast_range, predicate(&b, 0), predicate(&c, 0)];
    source
        .deriveAccessPathsFromPredicates()
        .expect("derive index range");
    let path = &source.PossibleAccessPaths[0];
    assert!(
        !path.AccessConds.is_empty(),
        "first index column should build an access range"
    );
    let index_column_ids = path
        .IndexFilters
        .iter()
        .flat_map(|filter| expression::ExtractColumns(filter.as_ref()))
        .map(|column| column.UniqueID)
        .collect::<Vec<_>>();
    assert!(
        index_column_ids.contains(&b.UniqueID),
        "b must remain an index filter: {index_column_ids:?}"
    );
    let table_column_ids = path
        .TableFilters
        .iter()
        .flat_map(|filter| expression::ExtractColumns(filter.as_ref()))
        .map(|column| column.UniqueID)
        .collect::<Vec<_>>();
    assert!(
        table_column_ids.contains(&c.UniqueID),
        "c must remain a table filter: {table_column_ids:?}"
    );
}
