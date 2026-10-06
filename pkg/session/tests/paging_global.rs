// Copyright 2026 AsterSQL.

use std::sync::Arc;

#[test]
fn paging_global_updates_existing_sessions_without_changing_captured_requests() {
    use astersql_session::testutil::TestSession;
    struct RestoreBudget(i64);
    impl Drop for RestoreBudget {
        fn drop(&mut self) {
            astersql_sessionctx_vardef::PagingSizeBytes.Store(self.0);
        }
    }
    let _restore = RestoreBudget(astersql_sessionctx_vardef::PagingSizeBytes.Load());
    let (domain, reader) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    reader
        .Execute("create table t (a int primary key, b int)")
        .unwrap();
    let writer = astersql_session::runtime::ConcreteSession::new(Arc::clone(&domain));
    writer
        .Execute("set global tidb_paging_size_bytes=0")
        .unwrap();
    writer
        .Execute("set global tidb_enable_resource_control=on")
        .unwrap();
    writer
        .Execute("alter resource group `default` ru_per_sec=1000 burstable=off")
        .unwrap();
    writer.Execute("begin").unwrap();
    writer
        .Execute("insert into t values (1,10),(2,20)")
        .unwrap();
    writer.Execute("commit").unwrap();
    reader.Execute("begin").unwrap();
    reader.Execute("insert into t values (3,30)").unwrap();
    let mut result = reader.Execute("select * from t order by a").unwrap();
    assert_eq!(
        result[0].Next().unwrap(),
        Some(vec!["1".into(), "10".into()])
    );
    assert_eq!(
        result[0].Next().unwrap(),
        Some(vec!["2".into(), "20".into()])
    );
    assert_eq!(
        result[0].Next().unwrap(),
        Some(vec!["3".into(), "30".into()])
    );
    assert!(result[0].Next().unwrap().is_none());
    let mut previous = reader.LastSelectRequestForTest().unwrap().request.clone();
    assert_eq!(previous.Paging.PagingSizeBytes, 0);
    let mut previous_context = reader.WithSessionVars(|vars| {
        vars.StmtCtx
            .GetOrInitDistSQLFromCache(|| panic!("SQL must initialize its request budget"))
    });
    for (value, expected) in [
        ("4194304", 4_194_304),
        ("1048576", 1_048_576),
        ("8388608", 8_388_608),
        ("default", 0),
        ("4194304", 4_194_304),
        ("0", 0),
    ] {
        let captured = previous.Paging.PagingSizeBytes;
        writer
            .Execute(&format!("set global tidb_paging_size_bytes={value}"))
            .unwrap();
        let cached = reader.WithSessionVars(|vars| {
            vars.StmtCtx.GetOrInitDistSQLFromCache(|| {
                panic!("existing statement context must remain cached")
            })
        });
        assert!(Arc::ptr_eq(&previous_context, &cached));
        let mut rows = reader.Execute("select * from t order by a").unwrap();
        for (a, b) in [(1, 10), (2, 20), (3, 30)] {
            assert_eq!(
                rows[0].Next().unwrap(),
                Some(vec![a.to_string(), b.to_string()])
            );
        }
        assert!(rows[0].Next().unwrap().is_none());
        let current = reader.LastSelectRequestForTest().unwrap().request.clone();
        assert_eq!(current.Paging.PagingSizeBytes, expected);
        assert_eq!(previous.Paging.PagingSizeBytes, captured);
        // The reader must still see its own uncommitted row after every update.
        let mut rows = writer.Execute("select * from t order by a").unwrap();
        assert_eq!(rows[0].Next().unwrap(), Some(vec!["1".into(), "10".into()]));
        assert_eq!(rows[0].Next().unwrap(), Some(vec!["2".into(), "20".into()]));
        assert!(rows[0].Next().unwrap().is_none());
        let current_context = reader.WithSessionVars(|vars| {
            vars.StmtCtx.GetOrInitDistSQLFromCache(|| {
                panic!("new statement must initialize its request budget")
            })
        });
        assert!(!Arc::ptr_eq(&previous_context, &current_context));
        previous_context = current_context;
        previous = current;
    }
    reader.Execute("rollback").unwrap();
    writer
        .Execute("set global tidb_paging_size_bytes=4194304")
        .unwrap();
    let new_reader = astersql_session::runtime::ConcreteSession::new(Arc::clone(&domain));
    let mut rows = new_reader
        .Execute("select @@global.tidb_paging_size_bytes, @@tidb_paging_size_bytes")
        .unwrap();
    assert_eq!(
        rows[0].Next().unwrap(),
        Some(vec!["4194304".into(), "4194304".into()])
    );
    let mut rows = new_reader.Execute("select * from t order by a").unwrap();
    assert_eq!(rows[0].Next().unwrap(), Some(vec!["1".into(), "10".into()]));
    assert_eq!(rows[0].Next().unwrap(), Some(vec!["2".into(), "20".into()]));
    assert!(
        rows[0].Next().unwrap().is_none(),
        "rollback must discard the reader's uncommitted row"
    );
    assert_eq!(
        new_reader
            .LastSelectRequestForTest()
            .unwrap()
            .request
            .Paging
            .PagingSizeBytes,
        4_194_304
    );
    assert!(
        reader
            .Execute("set session tidb_paging_size_bytes=0")
            .is_err()
    );
    assert!(reader.Execute("set @@tidb_paging_size_bytes=0").is_err());
    let mut rows = reader
        .Execute("select /*+ set_var(tidb_paging_size_bytes=1048576) */ @@tidb_paging_size_bytes")
        .unwrap();
    assert_eq!(rows[0].Next().unwrap(), Some(vec!["4194304".into()]));
    let mut warnings = reader.Execute("show warnings").unwrap();
    let mut warning_rows = Vec::new();
    while let Some(row) = warnings[0].Next().unwrap() {
        warning_rows.push(row);
    }
    assert_eq!(
        warning_rows
            .iter()
            .map(|row| row[1].as_str())
            .collect::<Vec<_>>(),
        vec!["3637", "1229", "1229"],
        "{warning_rows:?}"
    );
    assert_eq!(
        warning_rows
            .iter()
            .map(|row| row[2].as_str())
            .collect::<Vec<_>>(),
        vec![
            "Variable 'tidb_paging_size_bytes' might not be affected by SET_VAR hint.",
            "Variable 'tidb_paging_size_bytes' is a GLOBAL variable and should be set with SET GLOBAL",
            "Variable 'tidb_paging_size_bytes' is a GLOBAL variable and should be set with SET GLOBAL",
        ]
    );
    for (group_sql, rc, expected) in [
        (
            "alter resource group `default` ru_per_sec=1000 burstable=unlimited",
            "on",
            0,
        ),
        (
            "alter resource group `default` ru_per_sec=1000 burstable=off",
            "off",
            0,
        ),
        (
            "alter resource group `default` ru_per_sec=1000 burstable=off",
            "on",
            4_194_304,
        ),
    ] {
        writer.Execute(group_sql).unwrap();
        writer
            .Execute(&format!("set global tidb_enable_resource_control={rc}"))
            .unwrap();
        reader.Execute("select * from t order by a").unwrap();
        assert_eq!(
            reader
                .LastSelectRequestForTest()
                .unwrap()
                .request
                .Paging
                .PagingSizeBytes,
            expected
        );
    }
    domain.close();
}
