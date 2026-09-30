// Copyright 2026 AsterSQL.

#[test]
fn go_merge_33_explain_ru_format() {
    let statement = crate::New()
        .ParseOneStmt("EXPLAIN FORMAT = RU SELECT 1", "", "")
        .expect("RU is an accepted EXPLAIN format");
    let explain = statement
        .as_any()
        .downcast_ref::<crate::ast::ExplainStmt>()
        .expect("EXPLAIN statement");
    assert_eq!(explain.Format, "RU");
}

#[test]
fn go_merge_33_interval_function_arity() {
    for (sql, arity) in [
        ("SELECT INTERVAL(1, 2)", 2),
        ("SELECT INTERVAL(1, 2, 3)", 3),
    ] {
        let statement = crate::New()
            .ParseOneStmt(sql, "", "")
            .unwrap_or_else(|error| panic!("{sql}: {error}"));
        let select = statement
            .as_any()
            .downcast_ref::<crate::ast::SelectStmt>()
            .unwrap();
        let Some(crate::ast::ExprKind::Function { FnName, Args, .. }) = select.Fields.Fields[0]
            .Expr
            .as_ref()
            .map(|expression| &expression.Kind)
        else {
            panic!("{sql}: expected function expression");
        };
        assert_eq!(FnName.O.to_ascii_uppercase(), "INTERVAL");
        assert_eq!(Args.len(), arity, "{sql}");
    }
    for sql in ["SELECT INTERVAL()", "SELECT INTERVAL(1)"] {
        assert!(crate::New().ParseOneStmt(sql, "", "").is_err(), "{sql}");
    }
}

#[test]
fn go_merge_33_index_auto_presplit() {
    let statement = crate::New()
        .ParseOneStmt("CREATE INDEX i ON t (a) PRE_SPLIT_REGIONS AUTO", "", "")
        .expect("AUTO pre-split is accepted");
    let index = statement
        .as_any()
        .downcast_ref::<crate::ast::CreateIndexStmt>()
        .expect("CREATE INDEX statement");
    assert!(
        index
            .Option
            .as_ref()
            .is_some_and(|option| option.AutoPreSplit)
    );
    for sql in [
        "CREATE INDEX i ON t (a) PRE_SPLIT_REGIONS AUTO PRE_SPLIT_REGIONS 2",
        "CREATE INDEX i ON t (a) PRE_SPLIT_REGIONS 2 PRE_SPLIT_REGIONS AUTO",
    ] {
        let statement = crate::New().ParseOneStmt(sql, "", "").unwrap();
        let index = statement
            .as_any()
            .downcast_ref::<crate::ast::CreateIndexStmt>()
            .unwrap();
        let option = index.Option.as_ref().unwrap();
        assert!(!option.AutoPreSplit, "{sql}");
        assert_eq!(
            option.SplitOpt.as_ref().map(|split| split.Num),
            Some(2),
            "{sql}"
        );
    }
}

#[test]
fn go_merge_33_materialized_view_statements() {
    let statement = crate::New()
        .ParseOneStmt("DROP MATERIALIZED VIEW IF EXISTS v", "", "")
        .expect("DROP MATERIALIZED VIEW is accepted");
    let drop_view = statement
        .as_any()
        .downcast_ref::<crate::ast::DropMaterializedViewStmt>()
        .expect("DROP MATERIALIZED VIEW AST");
    assert!(drop_view.IfExists);
    assert_eq!(drop_view.ViewName.as_ref().unwrap().Name.O, "v");

    let statement = crate::New()
        .ParseOneStmt("DROP MATERIALIZED VIEW LOG IF EXISTS ON t", "", "")
        .unwrap();
    let drop_log = statement
        .as_any()
        .downcast_ref::<crate::ast::DropMaterializedViewLogStmt>()
        .unwrap();
    assert!(drop_log.IfExists);
    assert_eq!(drop_log.Table.as_ref().unwrap().Name.O, "t");

    let statement = crate::New()
        .ParseOneStmt("REFRESH MATERIALIZED VIEW v FAST DRY RUN", "", "")
        .unwrap();
    let refresh = statement
        .as_any()
        .downcast_ref::<crate::ast::RefreshMaterializedViewStmt>()
        .unwrap();
    assert_eq!(refresh.Type, crate::ast::RefreshMaterializedViewType::Fast);
    assert_eq!(
        refresh.ObserveType,
        crate::ast::RefreshMaterializedViewObserveType::DryRun
    );
}

#[test]
fn go_merge_33_materialized_view_create_alter_and_jobs() {
    let statement = crate::New()
        .ParseOneStmt("CREATE MATERIALIZED VIEW v (a) AS SELECT 1", "", "")
        .unwrap();
    let create = statement
        .as_any()
        .downcast_ref::<crate::ast::CreateMaterializedViewStmt>()
        .unwrap();
    assert_eq!(create.ViewName.as_ref().unwrap().Name.O, "v");
    assert_eq!(create.Cols[0].O, "a");

    let statement = crate::New()
        .ParseOneStmt("ALTER MATERIALIZED VIEW v COMMENT = 'hello'", "", "")
        .unwrap();
    let alter = statement
        .as_any()
        .downcast_ref::<crate::ast::AlterMaterializedViewStmt>()
        .unwrap();
    assert_eq!(alter.Actions[0].Comment, "hello");

    let statement = crate::New()
        .ParseOneStmt("PURGE MATERIALIZED VIEW LOG ON t", "", "")
        .unwrap();
    assert!(
        statement
            .as_any()
            .downcast_ref::<crate::ast::PurgeMaterializedViewLogStmt>()
            .is_some()
    );

    let statement = crate::New()
        .ParseOneStmt("CANCEL MATERIALIZED VIEW REFRESH JOB 42", "", "")
        .unwrap();
    let cancel = statement
        .as_any()
        .downcast_ref::<crate::ast::CancelMaterializedViewJobStmt>()
        .unwrap();
    assert_eq!(cancel.JobID, 42);

    let statement = crate::New()
        .ParseOneStmt("REFRESH MATERIALIZED VIEW v COMPLETE IN PLACE", "", "")
        .unwrap();
    let refresh = statement
        .as_any()
        .downcast_ref::<crate::ast::RefreshMaterializedViewStmt>()
        .unwrap();
    assert_eq!(
        refresh.Type,
        crate::ast::RefreshMaterializedViewType::Complete
    );
}

#[test]
fn go_merge_33_materialized_view_options_and_log() {
    let statement = crate::New()
        .ParseOneStmt(
            "CREATE MATERIALIZED VIEW v (a) COMMENT='x' SHARD_ROW_ID_BITS=2 PRE_SPLIT_REGIONS=1 REFRESH FAST NEXT 2 ATTRIBUTES='{}' AS SELECT 1",
            "",
            "",
        )
        .unwrap();
    let create = statement
        .as_any()
        .downcast_ref::<crate::ast::CreateMaterializedViewStmt>()
        .unwrap();
    assert_eq!(create.Comment, "x");
    assert_eq!(create.Options.len(), 2);
    assert_eq!(create.Options[0].UintValue, 2);
    assert_eq!(create.Options[1].UintValue, 1);
    assert_eq!(create.Attributes, "{}");
    assert!(create.Refresh.as_ref().unwrap().Next.is_some());

    let statement = crate::New()
        .ParseOneStmt(
            "CREATE MATERIALIZED VIEW LOG ON t (a) SHARD_ROW_ID_BITS=2 PURGE IMMEDIATE ALERT ROWS 10",
            "",
            "",
        )
        .unwrap();
    let log = statement
        .as_any()
        .downcast_ref::<crate::ast::CreateMaterializedViewLogStmt>()
        .unwrap();
    assert_eq!(log.Cols[0].O, "a");
    assert_eq!(log.Options.len(), 1);
    assert_eq!(log.Options[0].UintValue, 2);
    assert!(log.Purge.as_ref().unwrap().Immediate);
    assert_eq!(log.AccumulationAlert.as_ref().unwrap().Rows, 10);

    let statement = crate::New()
        .ParseOneStmt("ALTER MATERIALIZED VIEW LOG ON t PURGE", "", "")
        .unwrap();
    let alter = statement
        .as_any()
        .downcast_ref::<crate::ast::AlterMaterializedViewLogStmt>()
        .unwrap();
    assert_eq!(alter.Actions.len(), 1);

    assert!(
        crate::New()
            .ParseOneStmt(
                "CREATE MATERIALIZED VIEW v (a) COMMENT='x' COMMENT='y' AS SELECT 1",
                "",
                "",
            )
            .is_err()
    );
}

#[test]
fn go_merge_33_storage_class_and_operate_view() {
    let statement = crate::New()
        .ParseOneStmt("SHOW STORAGE_CLASS TRANSITIONS", "", "")
        .unwrap();
    let show = statement
        .as_any()
        .downcast_ref::<crate::ast::ShowStmt>()
        .unwrap();
    assert_eq!(show.Tp, crate::ast::ShowStmtType::StorageClassTransitions);

    let statement = crate::New()
        .ParseOneStmt("CREATE TABLE t (a INT) STORAGE_CLASS='hot'", "", "")
        .unwrap();
    let table = statement
        .as_any()
        .downcast_ref::<crate::ast::CreateTableStmt>()
        .unwrap();
    assert!(table.Options.iter().any(|option| {
        option.Tp == crate::ast::TableOptionType::StorageClass && option.StrValue == "HOT"
    }));

    let statement = crate::New()
        .ParseOneStmt("GRANT OPERATE VIEW ON *.* TO 'u'@'%'", "", "")
        .unwrap();
    let grant = statement
        .as_any()
        .downcast_ref::<crate::ast::GrantStmt>()
        .unwrap();
    assert_eq!(grant.Privs[0].Priv, parser_mysql::privs::OperateViewPriv);
}

#[test]
fn go_merge_33_materialized_view_variant_fields() {
    let statement = crate::New()
        .ParseOneStmt(
            "REFRESH MATERIALIZED VIEW v WITH ASYNC MODE COMPLETE OUT OF PLACE WITH PROFILE",
            "",
            "",
        )
        .unwrap();
    let refresh = statement
        .as_any()
        .downcast_ref::<crate::ast::RefreshMaterializedViewStmt>()
        .unwrap();
    assert!(refresh.WithAsyncMode);
    assert_eq!(
        refresh.CompleteType,
        crate::ast::RefreshMaterializedViewCompleteType::OutOfPlace
    );
    assert_eq!(
        refresh.ObserveType,
        crate::ast::RefreshMaterializedViewObserveType::Profile
    );

    let statement = crate::New()
        .ParseOneStmt("CANCEL MATERIALIZED VIEW LOG PURGE JOB 7", "", "")
        .unwrap();
    let cancel = statement
        .as_any()
        .downcast_ref::<crate::ast::CancelMaterializedViewJobStmt>()
        .unwrap();
    assert_eq!(cancel.JobID, 7);
    assert_eq!(
        cancel.Tp,
        crate::ast::CancelMaterializedViewJobType::LogPurge
    );

    let statement = crate::New()
        .ParseOneStmt("ALTER MATERIALIZED VIEW LOG ON t ADD COLUMN (a)", "", "")
        .unwrap();
    let alter = statement
        .as_any()
        .downcast_ref::<crate::ast::AlterMaterializedViewLogStmt>()
        .unwrap();
    assert_eq!(alter.Actions[0].Cols[0].O, "a");

    let statement = crate::New()
        .ParseOneStmt(
            "CREATE MATERIALIZED VIEW v (a) REFRESH FAST START WITH 1 NEXT 2 AS SELECT 1",
            "",
            "",
        )
        .unwrap();
    let create = statement
        .as_any()
        .downcast_ref::<crate::ast::CreateMaterializedViewStmt>()
        .unwrap();
    let schedule = create.Refresh.as_ref().unwrap();
    assert!(schedule.StartWith.is_some());
    assert!(schedule.Next.is_some());

    let statement = crate::New()
        .ParseOneStmt(
            "CREATE MATERIALIZED VIEW LOG ON t (a) PURGE START WITH 1 NEXT 2",
            "",
            "",
        )
        .unwrap();
    let log = statement
        .as_any()
        .downcast_ref::<crate::ast::CreateMaterializedViewLogStmt>()
        .unwrap();
    let purge = log.Purge.as_ref().unwrap();
    assert!(!purge.Immediate);
    assert!(purge.StartWith.is_some());
    assert!(purge.Next.is_some());

    let statement = crate::New()
        .ParseOneStmt(
            "ALTER MATERIALIZED VIEW v REFRESH NEXT 2, ATTRIBUTES='{}'",
            "",
            "",
        )
        .unwrap();
    let alter = statement
        .as_any()
        .downcast_ref::<crate::ast::AlterMaterializedViewStmt>()
        .unwrap();
    assert_eq!(alter.Actions.len(), 2);
    assert!(alter.Actions[0].Refresh.as_ref().unwrap().Next.is_some());
    assert_eq!(alter.Actions[1].Attributes, "{}");
}

#[test]
fn go_merge_33_analyze_default_options() {
    let statement = crate::New()
        .ParseOneStmt(
            "ANALYZE TABLE t WITH DEFAULT BUCKETS DEFAULT TOPN DEFAULT SAMPLES DEFAULT SAMPLERATE",
            "",
            "",
        )
        .unwrap();
    let analyze = statement
        .as_any()
        .downcast_ref::<crate::ast::AnalyzeTableStmt>()
        .unwrap();
    assert_eq!(analyze.AnalyzeOpts.len(), 4);
    assert_eq!(
        analyze
            .AnalyzeOpts
            .iter()
            .map(|option| option.Type)
            .collect::<Vec<_>>(),
        vec![
            crate::ast::AnalyzeOptionType::NumBuckets,
            crate::ast::AnalyzeOptionType::NumTopN,
            crate::ast::AnalyzeOptionType::NumSamples,
            crate::ast::AnalyzeOptionType::SampleRate,
        ]
    );
    assert!(
        analyze
            .AnalyzeOpts
            .iter()
            .all(|option| option.Value.is_none())
    );
}

#[test]
fn go_merge_33_unsupported_create_table_option() {
    let sql = "CREATE TABLE t (a INT) START TRANSACTION";
    let mut parser = crate::New();
    assert!(parser.ParseOneStmt(sql, "", "").is_err());
    parser.SetParserConfig(crate::ParserConfig {
        EnableWindowFunction: true,
        EnableStrictDoubleTypeCheck: true,
        SkipPositionRecording: false,
        EnableUnsupportedMySQLSyntax: true,
    });
    let statement = parser.ParseOneStmt(sql, "", "").unwrap();
    let create = statement
        .as_any()
        .downcast_ref::<crate::ast::CreateTableStmt>()
        .unwrap();
    assert!(
        create
            .Options
            .iter()
            .any(|option| option.Tp == crate::ast::TableOptionType::StartTransaction)
    );
    let statement = parser
        .ParseOneStmt(
            "CREATE TABLE t2 (a INT) ENGINE=InnoDB START TRANSACTION",
            "",
            "",
        )
        .unwrap();
    let create = statement
        .as_any()
        .downcast_ref::<crate::ast::CreateTableStmt>()
        .unwrap();
    assert_eq!(create.Options.len(), 2);
    assert_eq!(
        create.Options[1].Tp,
        crate::ast::TableOptionType::StartTransaction
    );
    assert!(
        parser
            .ParseOneStmt("CREATE SEQUENCE s START TRANSACTION", "", "")
            .is_err()
    );
    parser.Reset();
    assert!(parser.ParseOneStmt(sql, "", "").is_err());
}
