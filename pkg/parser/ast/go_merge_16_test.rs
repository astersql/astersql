// Copyright 2026 AsterSQL.

use crate::misc::{VariableAssignment, redact_url};
use crate::sem::{RefreshMaterializedViewCommand, SEMCommand};
use crate::stats::{AnalyzeOpt, AnalyzeOptNumTopN, AnalyzeTableStmt};
use crate::*;

#[test]
fn go_merge_16_redacts_embedding_keys_and_azure_endpoints() {
    for name in [
        "jina_ai",
        "openai",
        "cohere",
        "huggingface",
        "nvidia_nim",
        "gemini",
    ] {
        let name = format!("TIDB_EXP_EMBED_{}_API_KEY", name.to_uppercase());
        let assignment = VariableAssignment {
            name: name.clone(),
            value: "'secret'".into(),
            is_system: true,
            ..Default::default()
        };
        assert_eq!(assignment.restore(), format!("@@SESSION.`{name}`='******'"));
    }
    for (name, is_system) in [
        ("TIDB_EXP_EMBED_OPENAI_API_KEY", false),
        ("tidb_exp_embed_future_api_key", true),
    ] {
        let assignment = VariableAssignment {
            name: name.into(),
            value: "'ordinary'".into(),
            is_system,
            ..Default::default()
        };
        assert!(assignment.restore().contains("'ordinary'"));
    }
    assert_eq!(
        redact_url("azure://container/file?endpoint=one&endpoint=two"),
        "azure://container/file?endpoint=xxxxxx"
    );
    assert_eq!(
        redact_url("azblob://container/file?EndPoint=secret&access-tier=Hot"),
        "azblob://container/file?EndPoint=xxxxxx&access-tier=Hot"
    );
    assert_eq!(
        redact_url("s3://bucket/file?endpoint=visible"),
        "s3://bucket/file?endpoint=visible"
    );
}

#[test]
fn go_merge_16_analyze_default_is_distinct_from_zero() {
    let stmt = AnalyzeTableStmt {
        table_names: vec!["t".into()],
        analyze_opts: vec![AnalyzeOpt {
            option_type: AnalyzeOptNumTopN,
            value: None,
        }],
        ..Default::default()
    };
    assert_eq!(
        stmt.restore().unwrap(),
        "ANALYZE TABLE `t` WITH DEFAULT TOPN"
    );
    let stmt = AnalyzeTableStmt {
        table_names: vec!["t".into()],
        analyze_opts: vec![AnalyzeOpt {
            option_type: AnalyzeOptNumTopN,
            value: Some("0".into()),
        }],
        ..Default::default()
    };
    assert_eq!(stmt.restore().unwrap(), "ANALYZE TABLE `t` WITH 0 TOPN");
    let table = TableName {
        Name: CIStr {
            O: "t".into(),
            L: "t".into(),
        },
        ..Default::default()
    };
    let canonical = crate::AnalyzeTableStmt {
        TableNames: vec![table],
        AnalyzeOpts: vec![crate::AnalyzeOpt {
            Type: crate::AnalyzeOptionType::NumTopN,
            Value: None,
        }],
        ..Default::default()
    };
    assert_eq!(
        canonical.restore().unwrap(),
        "ANALYZE TABLE `t` WITH DEFAULT TOPN"
    );
    assert_eq!(
        crate::sql_restore::restore_node(&canonical).unwrap(),
        canonical.restore().unwrap()
    );
    assert_eq!(
        crate::AnalyzeTableStmt {
            AnalyzeOpts: vec![crate::AnalyzeOpt {
                Type: crate::AnalyzeOptionType::NumTopN,
                Value: Some(ExprNode::Value("0".into()))
            }],
            ..canonical
        }
        .restore()
        .unwrap(),
        "ANALYZE TABLE `t` WITH 0 TOPN"
    );
}

#[test]
fn go_merge_16_refresh_modes_and_sem_commands() {
    let stmt = RefreshMaterializedViewStmt {
        ViewName: Some(TableName {
            Name: CIStr {
                O: "mv".into(),
                L: "mv".into(),
            },
            ..Default::default()
        }),
        Type: RefreshMaterializedViewType::Complete,
        CompleteType: RefreshMaterializedViewCompleteType::DeltaApply,
        ..Default::default()
    };
    assert_eq!(
        stmt.mode().unwrap(),
        RefreshMaterializedViewMode::CompleteDeltaApply
    );
    assert_eq!(
        stmt.restore().unwrap(),
        "REFRESH MATERIALIZED VIEW `mv` COMPLETE DELTA APPLY"
    );
    assert_eq!(stmt.sem_command(), RefreshMaterializedViewCommand);
    for (complete_type, mode, suffix) in [
        (
            RefreshMaterializedViewCompleteType::InPlace,
            RefreshMaterializedViewMode::CompleteInPlace,
            "IN PLACE",
        ),
        (
            RefreshMaterializedViewCompleteType::OutOfPlace,
            RefreshMaterializedViewMode::CompleteOutOfPlace,
            "OUT OF PLACE",
        ),
        (
            RefreshMaterializedViewCompleteType::DeltaApply,
            RefreshMaterializedViewMode::CompleteDeltaApply,
            "DELTA APPLY",
        ),
    ] {
        let variant = RefreshMaterializedViewStmt {
            CompleteType: complete_type,
            ..stmt.clone()
        };
        assert_eq!(variant.mode().unwrap(), mode);
        assert_eq!(
            variant.restore().unwrap(),
            format!("REFRESH MATERIALIZED VIEW `mv` COMPLETE {suffix}")
        );
        assert_eq!(mode.to_string(), format!("COMPLETE {suffix}"));
    }
    assert_eq!(
        RefreshMaterializedViewType::Unknown(9).to_string(),
        "UNKNOWN"
    );
    assert_eq!(
        RefreshMaterializedViewCompleteType::Unknown(9).to_string(),
        "UNKNOWN"
    );
    assert!(
        RefreshMaterializedViewStmt {
            Type: RefreshMaterializedViewType::Unknown(9),
            ..stmt.clone()
        }
        .mode()
        .is_err()
    );
    assert!(
        RefreshMaterializedViewStmt {
            CompleteType: RefreshMaterializedViewCompleteType::Unknown(0),
            ..stmt
        }
        .mode()
        .is_err()
    );
}

#[test]
fn go_merge_16_materialized_restore_and_traversal() {
    let table = TableName {
        Name: CIStr {
            O: "mv".into(),
            L: "mv".into(),
        },
        ..Default::default()
    };
    let purge = PurgeMaterializedViewLogStmt {
        Table: Some(table.clone()),
        ..Default::default()
    };
    assert_eq!(
        purge.restore().unwrap(),
        "PURGE MATERIALIZED VIEW LOG ON `mv`"
    );
    assert_eq!(
        crate::sql_restore::restore_node(&purge).unwrap(),
        purge.restore().unwrap()
    );
    assert_eq!(
        purge.sem_command(),
        crate::sem::PurgeMaterializedViewLogCommand
    );
    assert!(PurgeMaterializedViewLogStmt::default().restore().is_err());
    for (kind, text, command) in [
        (
            CancelMaterializedViewJobType::LogPurge,
            "CANCEL MATERIALIZED VIEW LOG PURGE JOB 42",
            crate::sem::CancelMaterializedViewLogPurgeJobCommand,
        ),
        (
            CancelMaterializedViewJobType::Refresh,
            "CANCEL MATERIALIZED VIEW REFRESH JOB 42",
            crate::sem::CancelMaterializedViewRefreshJobCommand,
        ),
    ] {
        let cancel = CancelMaterializedViewJobStmt {
            Tp: kind,
            JobID: 42,
            ..Default::default()
        };
        assert_eq!(cancel.restore().unwrap(), text);
        assert_eq!(cancel.sem_command(), command);
    }
    assert!(CancelMaterializedViewJobStmt::default().restore().is_err());
    assert_eq!(
        CancelMaterializedViewJobStmt::default().sem_command(),
        crate::sem::UnknownCommand
    );
    assert_eq!(
        CancelMaterializedViewJobStmt {
            Tp: CancelMaterializedViewJobType::Unknown(9),
            ..Default::default()
        }
        .restore()
        .unwrap_err(),
        "invalid materialized view job cancel type: 9"
    );
    let stmt = RefreshMaterializedViewStmt {
        ViewName: Some(table),
        WithAsyncMode: true,
        Type: RefreshMaterializedViewType::Fast,
        ObserveType: RefreshMaterializedViewObserveType::DryRun,
        AsOf: Some(AsOfClause {
            TsExpr: ExprNode::Value("1".into()),
        }),
        ..Default::default()
    };
    assert_eq!(
        stmt.restore().unwrap(),
        "REFRESH MATERIALIZED VIEW `mv` WITH ASYNC MODE FAST AS OF TIMESTAMP 1 DRY RUN"
    );
    assert_eq!(
        crate::sql_restore::restore_node(&stmt).unwrap(),
        stmt.restore().unwrap()
    );
    let impl_stmt = RefreshMaterializedViewImplementStmt {
        RefreshStmt: Some(stmt),
        LastSuccessfulRefreshReadTSO: 1,
        TargetRefreshReadTSO: 2,
        MLogRetainedLowerTSO: 3,
        ..Default::default()
    };
    assert_eq!(
        impl_stmt.restore().unwrap(),
        "IMPLEMENT FOR REFRESH MATERIALIZED VIEW `mv` WITH ASYNC MODE FAST AS OF TIMESTAMP 1 DRY RUN USING TIMESTAMP 1 UP TO TIMESTAMP 2 MLOG RETAINED LOWER TIMESTAMP 3"
    );
    assert_eq!(impl_stmt.sem_command(), RefreshMaterializedViewCommand);
    assert!(
        RefreshMaterializedViewImplementStmt::default()
            .restore()
            .is_err()
    );
    struct Rename;
    impl InPlaceVisitor for Rename {
        fn enter(&mut self, _: &mut dyn Node) -> bool {
            false
        }
        fn leave(&mut self, _: &mut dyn Node) -> bool {
            true
        }
        fn enter_table_name(&mut self, table: &mut TableName) -> bool {
            table.Name.O = "renamed".into();
            true
        }
    }
    let mut impl_stmt = impl_stmt;
    assert!(Walk(&mut impl_stmt, &mut Rename));
    assert_eq!(
        impl_stmt.RefreshStmt.unwrap().ViewName.unwrap().Name.O,
        "renamed"
    );
}
