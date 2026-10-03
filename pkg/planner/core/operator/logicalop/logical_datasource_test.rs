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

#[test]
fn common_handle_group_ndv_uses_only_declared_index_columns() {
    let mut source = DataSource::default();
    source.TableStats.RowCount = 100.0;
    source.AskedColumnGroup = vec![vec![planner_column(1, 101)]];
    let mut histogram = statistics::PseudoHistColl(1, true);
    let mut index = statistics::Index {
        CMSketch: None,
        TopN: None,
        FMSketch: None,
        Info: None,
        Histogram: statistics::NewHistogram(
            9,
            7,
            0,
            0,
            &expression::types::NewFieldType(mysql::r#type::TypeLonglong),
            0,
            0,
        ),
        StatsLoadedStatus: statistics::NewStatsFullLoadStatus(),
        PhysicalID: 1,
        StatsVer: 2,
    };
    index.Info = Some(statistics::IndexInfo {
        ID: 9,
        Columns: vec![statistics::IndexColumnInfo::default()],
        ..Default::default()
    });
    index.Histogram.NDV = 7;
    index.StatsLoadedStatus = statistics::NewStatsFullLoadStatus();
    histogram.Indices.insert(9, Box::new(index));
    for mapped in [vec![101, 102, 103], vec![101, 102], vec![101]] {
        histogram.Idx2ColUniqueIDs.insert(9, mapped);
        source.TableStats.HistColl = Some(Arc::new(histogram.clone()));
        let (stats, _) = source.DeriveStats(true).unwrap();
        assert_eq!(source.TableStats.GroupNDVs, stats.GroupNDVs);
        assert_eq!(
            stats.GroupNDVs,
            vec![property::GroupNDV {
                Cols: vec![101],
                NDV: 7.0
            }]
        );
    }
    for mapped in [vec![], vec![999, 102, 103]] {
        histogram.Idx2ColUniqueIDs.insert(9, mapped);
        source.TableStats.HistColl = Some(Arc::new(histogram.clone()));
        assert!(source.DeriveStats(true).unwrap().0.GroupNDVs.is_empty());
    }
    histogram.Idx2ColUniqueIDs.insert(9, vec![101, 102, 103]);
    histogram.Indices.get_mut(&9).unwrap().StatsLoadedStatus =
        statistics::NewStatsAllEvictedStatus();
    source.TableStats.HistColl = Some(Arc::new(histogram));
    assert!(source.DeriveStats(true).unwrap().0.GroupNDVs.is_empty());
}

#[test]
fn common_handle_suffix_updates_histogram_column_mapping_once() {
    let context: base::ContextRef = Arc::new(RangeTestContext {
        next_id: AtomicI32::new(0),
        vars: planctx::variable::SessionVars::default(),
        expr: exprstatic::NewExprContext(Vec::new()),
        ranger: base::RangerContext {
            TypeCtx: expression::types::DefaultStmtNoWarningContext.clone(),
            ErrCtx: expression::errctx::StrictNoWarningContext.clone(),
            ExprCtx: Arc::new(exprstatic::NewExprContext(Vec::new())),
            RangeFallbackHandler: None,
            PlanCacheTracker: None,
            OptimizerFixControl: Default::default(),
            UseCache: false,
            RegardNULLAsPoint: true,
            OptPrefixIndexSingleScan: false,
        },
        usage: base::BuiltinFunctionUsageCounter::default(),
    });
    let columns = vec![
        planner_column(1, 101),
        planner_column(2, 102),
        planner_column(3, 103),
    ];
    let mut source = DataSource::default().Init(context, 0);
    source.TableInfo.IsCommonHandle = true;
    source.TableInfo.CommonHandleVersion = 1;
    source.TableInfo.Columns = (1..=3)
        .map(|id| model::ColumnInfo::New(id, parser_ast::NewCIStr(format!("c{id}"))))
        .collect();
    source
        .LogicalSchemaProducer
        .SetSchema(expression::NewSchema(columns.clone()));
    source.TblCols = columns.clone();
    source.CommonHandleCols = columns[1..].to_vec();
    source.CommonHandleLens = vec![-1, -1];
    source.PossibleAccessPaths = vec![planner_util::AccessPath {
        Index: Some(model::IndexInfo {
            ID: 9,
            Columns: vec![model::IndexColumn {
                Offset: 0,
                Length: -1,
                ..Default::default()
            }],
            ..Default::default()
        }),
        ..Default::default()
    }];
    source.TableStats.RowCount = 5.0;
    for (mapped, expected) in [
        (vec![101], vec![101, 102, 103]),
        (vec![101, 102, 103], vec![101, 102, 103]),
        (vec![], vec![]),
    ] {
        let mut histogram = statistics::PseudoHistColl(1, true);
        histogram.Idx2ColUniqueIDs.insert(9, mapped);
        source.TableStats.HistColl = Some(Arc::new(histogram));
        source.deriveAccessPathsFromPredicates().unwrap();
        assert_eq!(source.PossibleAccessPaths[0].IdxCols.len(), 3);
        assert_eq!(
            source.PossibleAccessPaths[0].FullIdxColLens,
            vec![-1, -1, -1]
        );
        let histogram = source
            .TableStats
            .HistColl
            .as_ref()
            .unwrap()
            .downcast_ref::<statistics::HistColl>()
            .unwrap();
        assert_eq!(histogram.Idx2ColUniqueIDs[&9], expected);
    }
}

#[test]
fn common_handle_suffix_respects_physical_key_layout_guards() {
    let context: base::ContextRef = Arc::new(RangeTestContext {
        next_id: AtomicI32::new(0),
        vars: planctx::variable::SessionVars::default(),
        expr: exprstatic::NewExprContext(Vec::new()),
        ranger: base::RangerContext {
            TypeCtx: expression::types::DefaultStmtNoWarningContext.clone(),
            ErrCtx: expression::errctx::StrictNoWarningContext.clone(),
            ExprCtx: Arc::new(exprstatic::NewExprContext(Vec::new())),
            RangeFallbackHandler: None,
            PlanCacheTracker: None,
            OptimizerFixControl: Default::default(),
            UseCache: false,
            RegardNULLAsPoint: true,
            OptPrefixIndexSingleScan: false,
        },
        usage: base::BuiltinFunctionUsageCounter::default(),
    });
    for guard in [
        "plain",
        "non_clustered",
        "empty",
        "lens",
        "unique",
        "primary",
        "global",
        "mv",
        "columnar",
        "overlap",
        "unresolved",
        "v0_string",
        "v0_binary",
    ] {
        let columns = vec![
            planner_column(1, 101),
            planner_column(2, 102),
            planner_column(3, 103),
        ];
        let mut source = DataSource::default().Init(context.clone(), 0);
        source.TableInfo.IsCommonHandle = guard != "non_clustered";
        source.TableInfo.CommonHandleVersion = if guard.starts_with("v0") { 0 } else { 1 };
        source.TableInfo.Columns = (1..=3)
            .map(|id| model::ColumnInfo::New(id, parser_ast::NewCIStr(format!("c{id}"))))
            .collect();
        source
            .LogicalSchemaProducer
            .SetSchema(expression::NewSchema(columns.clone()));
        source.TblCols = columns.clone();
        source.CommonHandleCols = if guard == "empty" {
            vec![]
        } else {
            columns[1..].to_vec()
        };
        source.CommonHandleLens = if guard == "lens" {
            vec![-1]
        } else {
            vec![-1, -1]
        };
        if guard.starts_with("v0") {
            let mut field = expression::types::NewFieldType(mysql::r#type::TypeVarchar);
            if guard == "v0_binary" {
                field.AddFlag(mysql::r#type::BinaryFlag);
            }
            source.CommonHandleCols[0].RetType = Some(*field);
        }
        let mut index = model::IndexInfo {
            ID: 9,
            Columns: vec![model::IndexColumn {
                Offset: 0,
                Length: -1,
                ..Default::default()
            }],
            ..Default::default()
        };
        index.Unique = guard == "unique";
        index.Primary = guard == "primary";
        index.Global = guard == "global";
        index.MVIndex = guard == "mv";
        if guard == "columnar" {
            index.InvertedInfo = Some(model::InvertedIndexInfo::default());
        }
        if guard == "overlap" {
            index.Columns.push(model::IndexColumn {
                Offset: 1,
                Length: -1,
                ..Default::default()
            });
        }
        if guard == "unresolved" {
            index.Columns.push(model::IndexColumn {
                Offset: 99,
                Length: -1,
                ..Default::default()
            });
        }
        source.PossibleAccessPaths = vec![planner_util::AccessPath {
            Index: Some(index),
            ..Default::default()
        }];
        source.TableStats.RowCount = 5.0;
        source.deriveAccessPathsFromPredicates().unwrap();
        let expected = match guard {
            "plain" | "v0_binary" => 3,
            "v0_string" if !expression::collate::NewCollationEnabled() => 3,
            "overlap" => 2,
            _ => 1,
        };
        assert_eq!(
            source.PossibleAccessPaths[0].IdxCols.len(),
            expected,
            "{guard}"
        );
    }
}

#[test]
fn appended_handle_point_estimate_aligns_to_table_selectivity() {
    let mut source = DataSource::default();
    source.TableStats.RowCount = 2.0;
    source.PossibleAccessPaths = vec![planner_util::AccessPath {
        Index: Some(model::IndexInfo {
            Columns: vec![model::IndexColumn::default()],
            ..Default::default()
        }),
        IdxCols: vec![planner_column(1, 101), planner_column(2, 102)],
        Ranges: vec![ranger::Range {
            LowVal: vec![
                expression::types::NewIntDatum(5),
                expression::types::NewIntDatum(7),
            ],
            HighVal: vec![
                expression::types::NewIntDatum(5),
                expression::types::NewIntDatum(7),
            ],
            Collators: expression::collate::GetBinaryCollatorSlice(2),
            ..Default::default()
        }],
        CountAfterAccess: 1.0,
        MinCountAfterAccess: 0.5,
        MaxCountAfterAccess: 1.0,
        ..Default::default()
    }];
    source.DeriveStats(true).unwrap();
    let path = &source.PossibleAccessPaths[0];
    assert_eq!(path.CountAfterAccess, 2.0);
    assert_eq!(path.MinCountAfterAccess, 0.5);
    assert_eq!(path.MaxCountAfterAccess, 2.0);

    source.TableStats.RowCount = 2.0 + cost::factors_thresholds::ToleranceFactor / 2.0;
    source.DeriveStats(true).unwrap();
    assert_eq!(source.PossibleAccessPaths[0].CountAfterAccess, 2.0);

    source.TableStats.RowCount = 3.0;
    source.PossibleAccessPaths[0].MinCountAfterAccess = 0.0;
    source.PossibleAccessPaths[0].MaxCountAfterAccess = 5.0;
    source.DeriveStats(true).unwrap();
    let path = &source.PossibleAccessPaths[0];
    assert_eq!(path.CountAfterAccess, 3.0);
    assert_eq!(path.MinCountAfterAccess, 2.0);
    assert_eq!(path.MaxCountAfterAccess, 5.0);
}
