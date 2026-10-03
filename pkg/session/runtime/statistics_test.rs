// Copyright 2026 AsterSQL.

use crate::runtime::CreateAnalyzeSession;
use crate::testutil::TestRecordSet;

#[test]
fn show_stats_histograms_distinguishes_lite_entries_from_loaded_empty_histograms() {
    let (domain, session) = CreateAnalyzeSession().expect("create statistics session");
    domain
        .set_stats_lease(std::time::Duration::from_secs(1))
        .expect("set statistics lease");
    for sql in [
        "use test",
        "create table empty_hist (c int, index idx(c))",
        "analyze table empty_hist",
    ] {
        session.execute(sql).expect(sql);
    }
    let histogram_count = || {
        let mut result = session
            .execute("show stats_histograms where Table_name = 'empty_hist'")
            .expect("show histograms")
            .remove(0);
        let mut count = 0;
        while result.Next().expect("read histogram").is_some() {
            count += 1;
        }
        count
    };
    assert_eq!(histogram_count(), 2);
    domain.stats_handle().lock().expect("stats handle").clear();
    domain.update_stats().expect("load lite statistics");
    assert_eq!(histogram_count(), 0);
    session
        .execute("explain select * from empty_hist where c = 1")
        .expect("request histograms");
    domain.load_needed_histograms().expect("load histograms");
    assert_eq!(histogram_count(), 2);
}

#[test]
fn show_stats_like_underscore_matches_one_unicode_character() {
    let (_domain, session) = CreateAnalyzeSession().expect("create statistics parity session");
    for sql in [
        "create database `库a`",
        "use `库a`",
        "create table t (id int)",
        "analyze table t",
    ] {
        session
            .execute(sql)
            .unwrap_or_else(|error| panic!("execute {sql}: {error}"));
    }

    let mut result = session
        .execute("show stats_meta like '_a'")
        .expect("execute Unicode SHOW LIKE")
        .remove(0);
    let row = result
        .Next()
        .expect("read SHOW STATS_META row")
        .expect("SHOW LIKE must match Unicode characters, not UTF-8 bytes");
    assert_eq!(row[0], "库a");
    assert_eq!(result.Next().expect("SHOW rows exhausted"), None);
}

#[test]
fn full_sampling_analyze_keeps_unsigned_boundary_rows_and_single_remaining_row() {
    let (domain, session) = CreateAnalyzeSession().expect("create statistics session");
    for sql in [
        "use test",
        "create table tu(a bigint unsigned primary key)",
        "insert into tu values (9223372036854775807), (9223372036854775808)",
        "analyze table tu with 1 samplerate, 0 topn, 2 buckets",
    ] {
        session.execute(sql).expect(sql);
    }
    let mut buckets = session
        .execute("show stats_buckets where db_name = 'test' and table_name = 'tu' and column_name = 'a' and is_index = 0")
        .expect("show unsigned histogram")
        .remove(0);
    let mut bounds = std::collections::BTreeSet::new();
    while let Some(row) = buckets.Next().expect("read bucket") {
        bounds.insert(row[8].clone());
        bounds.insert(row[9].clone());
    }
    assert!(bounds.contains("9223372036854775807"), "{bounds:?}");
    assert!(bounds.contains("9223372036854775808"), "{bounds:?}");
    for sql in [
        "truncate table tu",
        "insert into tu values (1)",
        "analyze table tu with 1 samplerate, 0 topn, 2 buckets",
    ] {
        session.execute(sql).expect(sql);
    }
    let meta = domain
        .stats_meta_rows()
        .into_iter()
        .filter(|row| row.database == "test" && row.table == "tu")
        .collect::<Vec<_>>();
    assert_eq!(meta.len(), 1);
    assert_eq!(meta[0].row_count, 1);
}
