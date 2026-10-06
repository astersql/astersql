// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

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

#[test]
fn analyze_default_resets_saved_table_and_partition_options() {
    let (_domain, session) = CreateAnalyzeSession().expect("create statistics session");
    for sql in [
        "use test",
        "set tidb_analyze_version=2",
        "set tidb_partition_prune_mode='static'",
        "create table analyze_default_t(a int) partition by range(a) (partition p0 values less than (10), partition p1 values less than (20))",
        "insert into analyze_default_t values (1),(11)",
        "analyze table analyze_default_t with 8 buckets, 7 topn",
    ] {
        session
            .execute(sql)
            .unwrap_or_else(|error| panic!("execute {sql}: {error}"));
    }

    let mut before = session
        .execute("select buckets,topn from mysql.analyze_options order by table_id")
        .expect("read saved options before reset")
        .remove(0);
    let mut before_rows = Vec::new();
    while let Some(row) = before.Next().expect("read saved option") {
        before_rows.push(row);
    }
    assert_eq!(
        before_rows,
        vec![vec![String::from("8"), String::from("7")]; 3]
    );

    session
        .execute("set tidb_partition_prune_mode='dynamic'")
        .expect("enable dynamic pruning");
    session
        .execute("analyze table analyze_default_t with default buckets, default topn")
        .expect("reset saved options");

    let mut after = session
        .execute("select buckets,topn from mysql.analyze_options order by table_id")
        .expect("read saved options after reset")
        .remove(0);
    let mut after_rows = Vec::new();
    while let Some(row) = after.Next().expect("read reset option") {
        after_rows.push(row);
    }
    assert_eq!(
        after_rows,
        vec![vec![String::from("0"), String::from("-1")]; 3]
    );
}

#[test]
fn combined_merge_deprecated_concurrency_sql_warning() {
    let (domain, session) = CreateAnalyzeSession().unwrap();
    domain
        .set_stats_global_variable("tidb_merge_partition_stats_concurrency", "4")
        .unwrap();
    for scope in ["session", "global"] {
        session
            .execute(&format!(
                "set @@{scope}.tidb_merge_partition_stats_concurrency=4"
            ))
            .unwrap();
        let mut rows = session.execute("show warnings").unwrap().remove(0);
        let row = rows.Next().unwrap().unwrap();
        assert_eq!(row[1], "1287");
        assert_eq!(
            row[2],
            "tidb_merge_partition_stats_concurrency is deprecated: the merge no longer runs concurrently, so this setting has no effect. Kept for backward compatibility."
        );
        assert!(rows.Next().unwrap().is_none());
        let mut rows = session.execute("select @@session.tidb_merge_partition_stats_concurrency, @@global.tidb_merge_partition_stats_concurrency").unwrap().remove(0);
        assert_eq!(rows.Next().unwrap().unwrap(), vec!["1", "1"]);
        session
            .execute(&format!(
                "set @@{scope}.tidb_merge_partition_stats_concurrency=1"
            ))
            .unwrap();
        assert!(
            session
                .execute("show warnings")
                .unwrap()
                .remove(0)
                .Next()
                .unwrap()
                .is_none()
        );
    }
    for (value, codes) in [("0", vec!["1292"]), ("999", vec!["1292", "1287"])] {
        session
            .execute(&format!(
                "set tidb_merge_partition_stats_concurrency={value}"
            ))
            .unwrap();
        let mut rows = session.execute("show warnings").unwrap().remove(0);
        let mut actual = Vec::new();
        while let Some(row) = rows.Next().unwrap() {
            actual.push(row[1].clone());
        }
        assert_eq!(actual, codes);
    }
    assert!(
        session
            .execute("set tidb_merge_partition_stats_concurrency='abc'")
            .is_err()
    );
}

#[test]
fn combined_merge_issue24349_sql_buckets() {
    let (_domain, session) = CreateAnalyzeSession().unwrap();
    for sql in [
        "use test",
        "set tidb_partition_prune_mode='dynamic'",
        "set tidb_analyze_version=2",
        "create table combined_issue (a int,b int) partition by hash(a) partitions 3",
        "insert into combined_issue values (0,3),(0,3),(0,3),(0,2),(1,1),(1,2),(1,2),(1,2),(1,3),(1,4),(2,1),(2,1)",
        "analyze table combined_issue with 1 topn,3 buckets",
    ] {
        session.execute(sql).expect(sql);
    }
    let mut rows = session
        .execute("show stats_topn where table_name='combined_issue' and partition_name='global'")
        .unwrap()
        .remove(0);
    let mut top = Vec::new();
    while let Some(row) = rows.Next().unwrap() {
        top.push((row[3].clone(), row[5].clone(), row[6].clone()));
    }
    top.sort();
    assert_eq!(
        top,
        vec![
            ("a".into(), "1".into(), "6".into()),
            ("b".into(), "2".into(), "4".into())
        ]
    );
    let mut rows=session.execute("show stats_buckets where table_name='combined_issue' and partition_name='global' and column_name='b'").unwrap().remove(0);
    let mut buckets = Vec::new();
    while let Some(row) = rows.Next().unwrap() {
        buckets.push((
            row[6].clone(),
            row[7].clone(),
            row[8].clone(),
            row[9].clone(),
        ));
    }
    assert_eq!(
        buckets,
        vec![
            ("2".into(), "2".into(), "1".into(), "1".into()),
            ("4".into(), "1".into(), "1".into(), "3".into()),
            ("8".into(), "1".into(), "3".into(), "4".into())
        ]
    );
}

#[test]
fn combined_merge_global_index_exact_topn_estimates() {
    let (_domain, session) = CreateAnalyzeSession().unwrap();
    session.execute("use test").unwrap();
    session
        .execute("set tidb_partition_prune_mode='dynamic'")
        .unwrap();
    for (i, analyze) in ["", " index idx", " index"].iter().enumerate() {
        let table = format!("combined_index_{i}");
        session.execute(&format!("create table {table}(a int,b int,c int default 0,unique index idx(b) global) partition by range(a)(partition p0 values less than(10),partition p1 values less than(20),partition p2 values less than(30),partition p3 values less than(40))")).unwrap();
        session
            .execute(&format!(
                "insert into {table}(a,b) values(1,1),(2,2),(3,3),(15,15),(25,25),(35,35)"
            ))
            .unwrap();
        session
            .execute(&format!("analyze table {table}{analyze}"))
            .unwrap();
        let mut top = session.execute(&format!("show stats_topn where table_name='{table}' and partition_name='global' and column_name='idx'")).unwrap().remove(0);
        let mut values = Vec::new();
        let mut estimate = 0;
        while let Some(row) = top.Next().unwrap() {
            let value = row[5].parse::<i64>().unwrap();
            let count = row[6].parse::<i64>().unwrap();
            values.push(value);
            if value < 16 {
                estimate += count;
            }
        }
        values.sort();
        assert_eq!(values, vec![1, 2, 3, 15, 25, 35]);
        assert_eq!(estimate, 4, "{analyze}: exact global TopN membership");
        let mut rows = session
            .execute(&format!(
                "select b from {table} use index(idx) where b<16 order by b"
            ))
            .unwrap()
            .remove(0);
        let mut values = Vec::new();
        while let Some(row) = rows.Next().unwrap() {
            values.push(row[0].clone());
        }
        assert_eq!(values, vec!["1", "2", "3", "15"]);
    }
}

#[test]
fn combined_merge_sql_100010_rows_seven_partitions() {
    let (_domain, session) = CreateAnalyzeSession().unwrap();
    for sql in [
        "use test",
        "set tidb_partition_prune_mode='dynamic'",
        "set tidb_analyze_version=2",
        "create table combined_large(a int primary key auto_increment,b int not null default 1,c int,d varchar(255) not null default '',e varchar(255),key idx_ab(a,b),key idx_be(b,e),key idx_d(d),key idx_ec(e,c)) partition by hash(a) partitions 7",
        "insert into combined_large(a) values(1),(2),(3),(4),(5),(6),(7),(8),(9),(10)",
        "insert into combined_large(a) select null from combined_large t1,combined_large t2,combined_large t3,combined_large t4,combined_large t5",
        "alter table combined_large add unique key uidx_cd(c,d) global",
        "alter table combined_large add unique key uidx_e(e) global",
        "analyze table combined_large with 1 topn,3 buckets",
    ] {
        session.execute(sql).expect(sql);
    }
    let mut rows = session
        .execute("show stats_topn where table_name='combined_large' and partition_name='global'")
        .unwrap()
        .remove(0);
    let mut actual = std::collections::BTreeMap::new();
    while let Some(row) = rows.Next().unwrap() {
        actual.insert(row[3].clone(), (row[5].clone(), row[6].clone()));
    }
    for (name, value, count) in [
        ("a", "1", "1"),
        ("b", "1", "100010"),
        ("d", "", "100010"),
        ("idx_ab", "(1, 1)", "1"),
        ("idx_be", "(1, NULL)", "100010"),
        ("idx_d", "", "100010"),
        ("idx_ec", "(NULL, NULL)", "100010"),
        ("uidx_cd", "(NULL, )", "100010"),
    ] {
        assert_eq!(
            actual.get(name),
            Some(&(value.into(), count.into())),
            "{name}: {actual:?}"
        );
    }
    let mut rows=session.execute("show stats_buckets where table_name='combined_large' and partition_name='global' and column_name='a'").unwrap().remove(0);
    let mut actual = Vec::new();
    while let Some(row) = rows.Next().unwrap() {
        actual.push((
            row[6].clone(),
            row[7].clone(),
            row[8].clone(),
            row[9].clone(),
        ));
    }
    assert_eq!(
        actual,
        vec![
            ("7".into(), "0".into(), "2".into(), "9".into()),
            ("33353".into(), "0".into(), "9".into(), "33355".into()),
            ("100009".into(), "1".into(), "33355".into(), "100010".into())
        ]
    );
}

#[test]
fn combined_merge_sql_53_partitions_diverse_distributions_repeatable() {
    let (_domain, session) = CreateAnalyzeSession().unwrap();
    for sql in [
        "use test",
        "set tidb_partition_prune_mode='dynamic'",
        "set tidb_analyze_version=2",
        "create table combined_paths(id int primary key auto_increment,uniform_col int not null,skewed_col int not null,sparse_col int,bimodal_col int not null,str_col varchar(64) not null,key idx_uniform(uniform_col),key idx_skewed(skewed_col),key idx_bimodal(bimodal_col),key idx_str(str_col)) partition by hash(id) partitions 53",
    ] {
        session.execute(sql).expect(sql);
    }
    let values = (1..=100)
        .map(|i| {
            format!(
                "({},{},{},{},'v{:04}_{}')",
                i % 97,
                if i % 10 == 0 { i % 5 + 1 } else { 0 },
                if i % 3 == 0 {
                    "NULL".into()
                } else {
                    (i % 50).to_string()
                },
                if i > 50 { 500 + i % 20 } else { i % 20 },
                i % 80,
                "x".repeat(i % 17)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    session.execute(&format!("insert into combined_paths(uniform_col,skewed_col,sparse_col,bimodal_col,str_col) values{values}")).unwrap();
    for _ in 0..7 {
        session.execute("insert into combined_paths(uniform_col,skewed_col,sparse_col,bimodal_col,str_col) select uniform_col,skewed_col,sparse_col,bimodal_col,str_col from combined_paths").unwrap();
    }
    let collect = |sql: &str| {
        let mut rs = session.execute(sql).unwrap().remove(0);
        let mut rows = Vec::new();
        while let Some(row) = rs.Next().unwrap() {
            rows.push(row);
        }
        rows.sort();
        rows
    };
    let mut previous = None;
    // Rust's existing SQL runtime publishes merges before ANALYZE returns.
    // This checks the Go fixture's result consistency; background job timing
    // remains an existing runtime capability outside a17d9ca122's increment.
    for flag in ["OFF", "ON"] {
        session
            .execute(&format!("set tidb_enable_async_merge_global_stats={flag}"))
            .unwrap();
        session
            .execute("analyze table combined_paths with 10 topn,20 buckets")
            .unwrap();
        let top = collect(
            "show stats_topn where table_name='combined_paths' and partition_name='global'",
        );
        let buckets = collect(
            "show stats_buckets where table_name='combined_paths' and partition_name='global'",
        );
        assert!(!top.is_empty() && !buckets.is_empty());
        if let Some(previous) = previous.take() {
            assert_eq!(previous, (top.clone(), buckets.clone()));
        }
        previous = Some((top, buckets));
    }
}
