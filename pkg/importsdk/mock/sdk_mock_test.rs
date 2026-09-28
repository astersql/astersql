// Copyright 2026 AsterSQL.

use super::*;
use astersql_importsdk::{FileScanner, JobManager};
use astersql_importsdk::{ImportOptions, SQLGenerator, TableMeta};

#[test]
fn independent_expectations_match_by_method_and_arguments() {
    let mock = NewMockJobManager();
    mock.EXPECT().SubmitJob("first", Ok(11));
    mock.EXPECT().SubmitJob("second", Ok(22));

    assert_eq!(mock.SubmitJob(&(), "second").unwrap(), 22);
    assert_eq!(mock.SubmitJob(&(), "first").unwrap(), 11);
    assert!(mock.verify().is_ok());
    assert_eq!(
        mock.calls(),
        vec![
            MockCall::SubmitJob {
                query: "second".into()
            },
            MockCall::SubmitJob {
                query: "first".into()
            },
        ]
    );
}

#[test]
fn mismatch_keeps_expectation_and_matching_uses_first_registered_response() {
    let mock = NewMockJobManager();
    mock.EXPECT().SubmitJob("same", Ok(1));
    mock.EXPECT().SubmitJob("same", Ok(2));

    assert!(mock.SubmitJob(&(), "other").is_err());
    assert_eq!(mock.pending_expectations().len(), 2);
    assert_eq!(mock.SubmitJob(&(), "same").unwrap(), 1);
    assert_eq!(mock.SubmitJob(&(), "same").unwrap(), 2);
    assert!(mock.verify().is_ok());
}

#[test]
fn go_return_slots_preserve_value_even_when_error_is_present() {
    let mock = NewMockJobManager();
    mock.EXPECT()
        .SubmitJobParts("IMPORT INTO t", 42, Some(astersql_errors::New("network")));

    let (job_id, error) = mock.SubmitJobParts(&(), "IMPORT INTO t");
    assert_eq!(job_id, 42);
    assert_eq!(error.unwrap().to_string(), "network");
    assert!(mock.verify().is_ok());
}

#[test]
fn call_counts_and_dependencies_match_gomock_rules() {
    let mock = NewMockJobManager();
    let first = mock.EXPECT().SubmitJob("first", Ok(7)).Times(2);
    mock.EXPECT().SubmitJob("second", Ok(8)).After(&first);

    assert!(mock.SubmitJob(&(), "second").is_err());
    assert_eq!(mock.SubmitJob(&(), "first").unwrap(), 7);
    assert!(mock.SubmitJob(&(), "second").is_err());
    assert_eq!(mock.SubmitJob(&(), "first").unwrap(), 7);
    assert_eq!(mock.SubmitJob(&(), "second").unwrap(), 8);
    assert!(mock.verify().is_ok());
}

#[test]
fn optional_repeated_expectation_does_not_fail_verification() {
    let mock = NewMockJobManager();
    mock.EXPECT().SubmitJob("optional", Ok(1)).AnyTimes();
    assert!(mock.verify().is_ok());
    assert_eq!(mock.SubmitJob(&(), "optional").unwrap(), 1);
    assert_eq!(mock.SubmitJob(&(), "optional").unwrap(), 1);
    assert!(mock.verify().is_ok());
}

#[test]
fn sql_callback_observes_typed_arguments_with_any_matchers() {
    let mock = NewMockSQLGenerator();
    mock.EXPECT().GenerateImportSQLAny(|meta, options| {
        assert_eq!(meta.Database, "db");
        assert_eq!(options.ResourceParameters, "region=us-east-1");
        Ok("IMPORT INTO ...".into())
    });
    let meta = TableMeta {
        Database: "db".into(),
        ..Default::default()
    };
    let options = ImportOptions {
        ResourceParameters: "region=us-east-1".into(),
        ..Default::default()
    };
    assert_eq!(
        mock.GenerateImportSQL(&meta, &options).unwrap(),
        "IMPORT INTO ..."
    );
    assert!(mock.verify().is_ok());
}

#[test]
fn custom_matcher_can_select_business_arguments() {
    let mock = NewMockJobManager();
    mock.EXPECT().SubmitJob("placeholder", Ok(4)).Matching(|actual| {
        matches!(actual, MockCall::SubmitJob { query } if query.starts_with("IMPORT INTO"))
    });
    assert_eq!(mock.SubmitJob(&(), "IMPORT INTO db.t").unwrap(), 4);
    assert!(mock.verify().is_ok());
}

#[test]
fn context_matcher_rejects_wrong_context_without_consuming_call() {
    let mock = NewMockJobManager();
    mock.EXPECT()
        .SubmitJob("sql", Ok(9))
        .ContextMatching(|context| {
            context
                .downcast_ref::<String>()
                .is_some_and(|value| value == "allowed")
        });
    assert!(mock.SubmitJob(&String::from("denied"), "sql").is_err());
    assert_eq!(mock.SubmitJob(&String::from("allowed"), "sql").unwrap(), 9);
    assert!(mock.verify().is_ok());
}

#[test]
fn table_metas_can_return_value_and_error_together() {
    let mock = NewMockFileScanner();
    let meta = TableMeta {
        Database: "db".into(),
        ..Default::default()
    };
    mock.EXPECT()
        .GetTableMetas(Ok(Vec::new()))
        .ReturnParts(Some(vec![meta]), Some(astersql_errors::New("partial")));
    let (metas, error) = mock.GetTableMetasParts(&());
    assert_eq!(metas.unwrap()[0].Database, "db");
    assert_eq!(error.unwrap().to_string(), "partial");
    assert!(mock.verify().is_ok());
}

#[test]
fn trait_objects_forward_go_return_parts() {
    let scanner = NewMockFileScanner();
    scanner.EXPECT().GetTableMetas(Ok(Vec::new())).ReturnParts(
        Some(vec![TableMeta {
            Database: "db".into(),
            ..Default::default()
        }]),
        Some(astersql_errors::New("partial")),
    );
    let mut scanner: Box<dyn FileScanner> = Box::new(scanner);
    let (metas, error) = scanner.GetTableMetasParts(&());
    assert_eq!(metas.unwrap()[0].Database, "db");
    assert_eq!(error.unwrap().to_string(), "partial");

    let sql = NewMockSQLGenerator();
    let meta = TableMeta::default();
    let options = ImportOptions::default();
    sql.EXPECT()
        .GenerateImportSQL(&meta, &options, Ok(String::new()))
        .ReturnParts(
            String::from("IMPORT INTO ..."),
            Some(astersql_errors::New("warning")),
        );
    let sql: Box<dyn SQLGenerator> = Box::new(sql);
    let (statement, error) = sql.GenerateImportSQLParts(&meta, &options);
    assert_eq!(statement, "IMPORT INTO ...");
    assert_eq!(error.unwrap().to_string(), "warning");
}

#[test]
fn job_callback_observes_context_and_query() {
    let mock = NewMockJobManager();
    mock.EXPECT()
        .SubmitJob("placeholder", Ok(0))
        .Matching(|call| matches!(call, MockCall::SubmitJob { .. }))
        .DoAndReturn(|call, context| {
            let MockCall::SubmitJob { query } = call else {
                panic!("wrong call")
            };
            assert_eq!(query, "IMPORT INTO db.t");
            assert_eq!(context.unwrap().downcast_ref::<i32>(), Some(&12));
            (77_i64, None)
        });
    assert_eq!(mock.SubmitJob(&12_i32, "IMPORT INTO db.t").unwrap(), 77);
    assert!(mock.verify().is_ok());
}

#[test]
fn callback_can_return_nonzero_id_with_error_through_trait() {
    let mock = NewMockJobManager();
    mock.EXPECT()
        .SubmitJob("sql", Ok(0))
        .DoAndReturn(|_, _| (77_i64, Some(astersql_errors::New("submit failed"))));
    let mock: Box<dyn JobManager> = Box::new(mock);
    let (id, error) = mock.SubmitJobParts(&(), "sql");
    assert_eq!(id, 77);
    assert_eq!(error.unwrap().to_string(), "submit failed");
}
