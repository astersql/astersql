// Copyright 2026 AsterSQL.

use crate::stubs::testkit;
use crate::workload::*;
use astersql_tests_realtikvtest::stubs::{Storage, TestCtx};

#[test]
fn clock_uses_current_local_date() {
    let expected = std::process::Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .unwrap();
    let expected = String::from_utf8(expected.stdout).unwrap();
    assert!(genColval(12).starts_with(expected.trim()));
}

#[test]
fn random_rows_cover_both_endpoints() {
    let sql: Vec<_> = (0..128).map(|_| updateStr(1, "t0", &[1])).collect();
    assert!(sql.iter().any(|s| s.ends_with("=0")));
    assert!(sql.iter().any(|s| s.ends_with("=1")));
}

#[test]
fn execution_rejects_invalid_sql() {
    let tk = testkit::NewTestKit(&TestCtx::new(), Storage::new("workload-audit"));
    assert!(tk.Exec("this is not SQL").is_err());
}

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

fn fixture() -> (crate::SuiteContext, testkit::TestKit) {
    let t = TestCtx::new();
    let store = Storage::new("workload-real-sql");
    let tk = testkit::NewTestKit(&t, store.clone());
    tk.MustExec("create database addindex");
    tk.MustExec(&crate::common::genTableStr("t0"));
    let ctx = crate::common::newSuiteContext(&t, tk.clone(), store);
    (ctx, tk)
}

#[test]
fn real_dml_updates_storage_and_failed_insert_preserves_id() {
    let (ctx, tk) = fixture();
    let id = Mutex::new(10000);
    insertWorker(&tk, "t0", "2008-02-02", &id, &ctx.isPK, &ctx.isUnique).unwrap();
    assert_eq!(*id.lock().unwrap(), 10001);
    assert_eq!(
        tk.Query("select c0, c6, c19 from addindex.t0").unwrap(),
        vec![vec!["10000", "10000", "aaaa10000"]]
    );
    let other = testkit::NewTestKit(&ctx.t, ctx.store.clone());
    assert_eq!(
        other.Query("select count(*) from addindex.t0").unwrap(),
        vec![vec!["1"]]
    );
    tk.MustExec("alter table addindex.t0 add unique index idx(c0)");
    *id.lock().unwrap() = 10000;
    assert!(insertWorker(&tk, "t0", "2008-02-02", &id, &ctx.isPK, &ctx.isUnique).is_err());
    assert_eq!(*id.lock().unwrap(), 10000);
    ctx.set_unique(true);
    insertWorker(&tk, "t0", "2008-02-02", &id, &ctx.isPK, &ctx.isUnique).unwrap();
    assert_eq!(*id.lock().unwrap(), 10001);
    tk.MustExec("update addindex.t0 set c0=0");
    updateWorker(&tk, "t0", &[6], 0, &ctx.isPK, &ctx.isUnique).unwrap();
    assert_eq!(
        tk.Query("select c6 from addindex.t0").unwrap(),
        vec![vec!["9936"]]
    );
    deleteWorker(&tk, "t0", 0, &ctx.isPK, &ctx.isUnique).unwrap();
    assert_eq!(
        tk.Query("select count(*) from addindex.t0").unwrap(),
        vec![vec!["0"]]
    );
}

#[test]
fn result_sets_and_error_paths_match_go() {
    let (ctx, tk) = fixture();
    let id = Mutex::new(10000);
    // Isolate only the result-set boundary, including Go's simultaneous rs/error.
    let rs = testkit::ResultSet::new();
    let returned = rs.clone();
    tk.set_outcome_handler(move |_| (Some(returned.clone()), Some("9007 conflict".into())));
    insertWorker(&tk, "t0", "2008-02-02", &id, &ctx.isPK, &ctx.isUnique).unwrap();
    assert!(rs.is_closed());
    assert_eq!(*id.lock().unwrap(), 10001);
    let rs = testkit::ResultSet::with_close_error("close failed");
    let returned = rs.clone();
    tk.set_outcome_handler(move |_| (Some(returned.clone()), None));
    assert_eq!(
        insertWorker(&tk, "t0", "2008-02-02", &id, &ctx.isPK, &ctx.isUnique),
        Err("close failed".into())
    );
    assert_eq!(*id.lock().unwrap(), 10001);
    assert!(rs.is_closed());
    assert_eq!(
        updateWorker(&tk, "t0", &[6], 0, &ctx.isPK, &ctx.isUnique),
        Err("close failed".into())
    );
    assert_eq!(
        deleteWorker(&tk, "t0", 0, &ctx.isPK, &ctx.isUnique),
        Err("close failed".into())
    );
    let rs = testkit::ResultSet::new();
    let returned = rs.clone();
    tk.set_outcome_handler(move |_| (Some(returned.clone()), Some("fatal".into())));
    assert_eq!(
        insertWorker(&tk, "t0", "2008-02-02", &id, &ctx.isPK, &ctx.isUnique),
        Err("fatal".into())
    );
    assert!(
        !rs.is_closed(),
        "Go returns before Close on an unskippable Exec error"
    );
    let flag = ctx.isPK.clone();
    tk.set_outcome_handler(move |_| {
        flag.store(true, Ordering::SeqCst);
        (None, Some("[kv:1062] duplicate".into()))
    });
    insertWorker(&tk, "t0", "2008-02-02", &id, &ctx.isPK, &ctx.isUnique).unwrap();
    assert_eq!(
        *id.lock().unwrap(),
        10002,
        "read flags after Exec, not at worker startup"
    );
}

#[test]
fn workers_publish_counter_and_stop_with_cancelled_context() {
    let (mut ctx, tk) = fixture();
    initWorkloadParams(&mut ctx);
    let old_cancel = ctx.cancelled.clone();
    ctx.cancel();
    initWorkloadParams(&mut ctx);
    assert!(old_cancel.load(Ordering::SeqCst));
    assert!(!ctx.done());
    assert!(!Arc::ptr_eq(&old_cancel, &ctx.cancelled));
    let mut workload = Workload::default();
    workload.start(&ctx, &[0, 6]);
    let deadline = Instant::now() + Duration::from_secs(60);
    while *workload.insertID.lock().unwrap() == 10000 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    workload.stop(&ctx, -1).unwrap();
    assert!(ctx.done());
    let id = *workload.insertID.lock().unwrap();
    assert!(id > 10000);
    assert_eq!(
        tk.Query("select count(*) from addindex.t0").unwrap(),
        vec![vec![(id - 10000).to_string()]]
    );
    std::thread::sleep(Duration::from_millis(25));
    assert_eq!(*workload.insertID.lock().unwrap(), id);
}

#[test]
fn initialization_builders_and_error_codes_match_go() {
    let mut w = Workload::default();
    initWorkLoadContext(&mut w, &[2, 1, 6]);
    assert_eq!(w.tableName, "t2");
    assert_eq!(w.colID, vec![1, 6]);
    assert_eq!(*w.insertID.lock().unwrap(), 10000);
    initWorkLoadContext(&mut w, &[9]);
    assert_eq!(w.tableID, 2);
    initWorkLoadContext(&mut w, &[1, 28]);
    assert_eq!(w.colID, vec![28]);
    assert_eq!(w.date, "2008-02-02");
    for (cid, expected) in [
        (1, "c1 + 1"),
        (2, "c2 + 1"),
        (3, "c3 + 1"),
        (4, "c4 + 1"),
        (5, "c5 + 1"),
        (6, "c6 - 64"),
        (7, "c7 + 1"),
        (8, "%\n"),
        (9, "%\n"),
        (10, "%\n"),
        (11, "adddate(c11, 90)"),
        (19, "c19 + c0"),
        (28, "json_object('name', 'NanJing', 'population', 2566)"),
        (0, ""),
        (16, ""),
        (29, ""),
    ] {
        assert_eq!(genColval(cid), expected);
    }
    for cid in [18, 20, 21, 22, 23, 24, 25, 26, 27] {
        assert_eq!(genColval(cid), "ABCDEEEF");
    }
    assert_eq!(
        updateStr(0, "t0", &[1, 6]),
        "update addindex.t0 set c1=c1 + 1, c6=c6 - 64 where c0=0"
    );
    assert_eq!(updateStr(0, "t0", &[]), "update addindex.t0 where c0=0");
    assert_eq!(updateStr(0, "t0", &[1, 16]), "");
    assert_eq!(deleteStr("t0", 7), "delete from addindex.t0 where c0 =7");
    for flags in [(false, false), (true, false), (false, true), (true, true)] {
        assert!(isSkippedError(&None, flags.0, flags.1));
        for code in ["8028", "9007", "global:2"] {
            assert!(isSkippedError(&Some(code.into()), flags.0, flags.1));
        }
        assert!(!isSkippedError(
            &Some("1062 duplicate".into()),
            flags.0,
            flags.1
        ));
        assert_eq!(
            isSkippedError(&Some("[1062] duplicate".into()), flags.0, flags.1),
            flags.0 || flags.1
        );
        assert!(!isSkippedError(&Some("fatal".into()), flags.0, flags.1));
    }
}

#[test]
#[should_panic]
fn negative_row_count_matches_go_intn_panic() {
    updateStr(-1, "t0", &[1]);
}

#[test]
fn maximum_row_count_does_not_overflow_range() {
    let sql = updateStr(i32::MAX, "t0", &[1]);
    let id: i64 = sql.rsplit('=').next().unwrap().parse().unwrap();
    assert!((0..=i64::from(i32::MAX)).contains(&id));
}

#[test]
fn clock_preserves_local_zone_year_and_monotonic_suffix() {
    let zone = std::process::Command::new("date")
        .arg("+%z %Z")
        .output()
        .unwrap();
    let zone = String::from_utf8(zone.stdout).unwrap();
    for cid in [12, 13, 14, 15] {
        let value = genColval(cid);
        assert!(value.contains(zone.trim()), "{value} vs {zone}");
        assert!(value.contains(" m=+"));
    }
    use chrono::Datelike;
    assert_eq!(genColval(17), chrono::Local::now().year().to_string());
}

#[test]
fn insert_sql_equals_go_oracle() {
    assert_eq!(
        insertStr("t0", 10000, "2008-02-02"),
        r#"insert into addindex.t0(c0, c1, c2, c3, c4, c5, c6, c7, c8, c9, c10, c11, c12, c13, c14, c15, c16, c17, c18, c19, c20, c21, c22, c23, c24, c25, c26, c27, c28) values(10000,3, 3, 3, 3, 3, 10000, 3, 3.0, 3.0, 1113.1111, adddate('2008-02-02', 0), '11:11:13', '2001-01-03 11:11:13', '2001-01-03 11:11:11.123456', 2001, 'cccc', 'cccc', 'cccc', 'aaaa10000', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', '{"name": "Beijing", "population": 102}')"#
    );
}

#[test]
fn real_date_arithmetic_uses_calendar_and_null_semantics() {
    let (_, tk) = fixture();
    for (sql, expected) in [
        ("select adddate('2024-02-28', 1)", "2024-02-29"),
        ("select adddate('2024-02-28', 2)", "2024-03-01"),
        ("select adddate('2008-02-02', -1)", "2008-02-01"),
        (
            "select date_add('2024-01-31', interval 1 month)",
            "2024-02-29",
        ),
        ("select subdate('2024-03-01', 1)", "2024-02-29"),
        ("select adddate(NULL, 1)", "<nil>"),
    ] {
        assert_eq!(tk.Query(sql).unwrap(), vec![vec![expected]], "{sql}");
    }
}

#[test]
fn primary_key_ddl_validates_rows_and_persists_constraints() {
    let (_, tk) = fixture();
    tk.MustExec("create table addindex.pk(a int, b int)");
    tk.MustExec("insert into addindex.pk values (null, 1)");
    let err = tk
        .Exec("alter table addindex.pk add primary key named(a)")
        .unwrap_err();
    assert!(err.contains("1138"), "{err}");
    tk.MustExec("delete from addindex.pk");
    tk.MustExec("insert into addindex.pk values (1,1), (1,2)");
    let err = tk
        .Exec("alter table addindex.pk add primary key named(a)")
        .unwrap_err();
    assert!(err.contains("1062"), "{err}");
    tk.MustExec("delete from addindex.pk");
    tk.MustExec("insert into addindex.pk values (1,1)");
    tk.MustExec("alter table addindex.pk add primary key named(a)");
    let err = tk
        .Exec("alter table addindex.pk add primary key another(b)")
        .unwrap_err();
    assert!(err.contains("1068"), "{err}");
    assert!(
        tk.Exec("insert into addindex.pk values (1,2)")
            .unwrap_err()
            .contains("1062")
    );
    assert!(tk.Exec("insert into addindex.pk values (null,2)").is_err());
    let ddl = tk.Query("show create table addindex.pk").unwrap();
    assert!(ddl[0][1].contains("PRIMARY KEY"), "{ddl:?}");
    tk.MustExec("alter table addindex.pk drop primary key");
    let ddl = tk.Query("show create table addindex.pk").unwrap();
    assert!(!ddl[0][1].contains("PRIMARY KEY"), "{ddl:?}");
    tk.MustExec("insert into addindex.pk values (1,2)");
    assert!(
        tk.Exec("insert into addindex.pk values (null,2)").is_err(),
        "DROP PRIMARY KEY retains NOT NULL"
    );
}
