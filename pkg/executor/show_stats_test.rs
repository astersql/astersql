// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// `SHOW STATS_*` / `SHOW ANALYZE STATUS` 等语句的集成风格单元测试。
//
// 通过 TestKit 与 mock analyze store 验证元信息、锁定、直方图、桶、快照、
// 列使用情况与分析任务状态在动态/静态分区裁剪下的展示行为。

#![allow(non_snake_case)]

use astersql_testkit::mockstore::CreateAnalyzeStatsStore;
use astersql_testkit::{DbValue, TestKit};

/// 构造带 analyze 统计 mock store 的 TestKit。
fn stats_testkit() -> TestKit {
    TestKit::new(CreateAnalyzeStatsStore())
}

#[test]
/// 验证 STATS_META 过滤、跨库 LIKE，以及动态/静态分区行差异。
fn TestShowStatsMeta() {
    let mut testkit = stats_testkit();
    testkit.MustExec("create table t(a int, b int)", Vec::new());
    testkit.MustExec("create table t1(a int, b int)", Vec::new());
    testkit.MustExec("analyze table t, t1", Vec::new());
    assert_eq!(
        testkit
            .MustQuery("show stats_meta", Vec::new())
            .Rows()
            .len(),
        2
    );
    assert_eq!(
        testkit
            .MustQuery("show stats_meta where table_name = 't'", Vec::new())
            .Rows()
            .len(),
        1
    );
    assert_eq!(
        testkit
            .MustQuery(
                "show stats_meta where db_name = 'missing' or table_name in ('t1','t')",
                Vec::new(),
            )
            .Rows()
            .len(),
        2
    );
    assert_eq!(
        testkit
            .MustQuery(
                "show stats_meta where table_name = 't1' and 1 = 0",
                Vec::new()
            )
            .Rows()
            .len(),
        0
    );
    assert_eq!(
        testkit
            .MustQuery("show stats_meta where table_name like 't%'", Vec::new())
            .Rows()
            .len(),
        2
    );

    testkit.MustExec("create database test2", Vec::new());
    testkit.MustExec("create table test2.cross_t(a int)", Vec::new());
    testkit.MustExec("analyze table test2.cross_t", Vec::new());
    for pattern in ["Test2%", "test2"] {
        let rows = testkit
            .MustQuery(&format!("show stats_meta like '{pattern}'"), Vec::new())
            .Rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(&rows[0][0..2], ["test2", "cross_t"]);
    }

    testkit.MustExec(
        "create table dynamic_t(a int) partition by range(a) (partition p0 values less than (6))",
        Vec::new(),
    );
    testkit.MustExec("insert into dynamic_t values(1)", Vec::new());
    testkit.MustExec("analyze table dynamic_t", Vec::new());
    let dynamic = testkit
        .MustQuery("show stats_meta where table_name = 'dynamic_t'", Vec::new())
        .Rows();
    assert_eq!(dynamic.len(), 2);
    assert!(dynamic.iter().any(|row| row[2] == "global"));
    assert!(dynamic.iter().any(|row| row[2] == "p0"));

    testkit.MustExec("set @@tidb_partition_prune_mode = 'static'", Vec::new());
    testkit.MustExec(
        "create table static_t(a int) partition by range(a) (partition p0 values less than (6))",
        Vec::new(),
    );
    testkit.MustExec("insert into static_t values(1)", Vec::new());
    testkit.MustExec("analyze table static_t", Vec::new());
    let static_rows = testkit
        .MustQuery("show stats_meta where table_name = 'static_t'", Vec::new())
        .Rows();
    assert_eq!(static_rows.len(), 1);
    assert_eq!(&static_rows[0][0..3], ["test", "static_t", "p0"]);
    assert_eq!(&static_rows[0][4..6], ["0", "1"]);
}

#[test]
/// 验证 `lock stats` 后 `SHOW STATS_LOCKED` 列表与 where 过滤。
fn TestShowStatsLocked() {
    let mut testkit = stats_testkit();
    for table in ["t", "t1", "a1", "dc"] {
        testkit.MustExec(&format!("create table {table}(a int)"), Vec::new());
    }
    testkit.MustExec("lock stats t, t1, a1, dc", Vec::new());
    let mut rows = testkit.MustQuery("show stats_locked", Vec::new()).Rows();
    rows.sort();
    assert_eq!(
        rows.iter().map(|row| row[1].as_str()).collect::<Vec<_>>(),
        ["a1", "dc", "t", "t1"]
    );
    testkit
        .MustQuery("show stats_locked where table_name = 't'", Vec::new())
        .Check(vec![vec!["test", "t", "", "locked"]]);
}

#[test]
/// 验证 analyze 前后直方图行出现，以及索引分析加载状态。
fn TestShowStatsHistograms() {
    let mut testkit = stats_testkit();
    testkit.MustExec(
        "create table t(a int, b int, c int, index idx_b(b), index idx_c_a(c,a))",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into t values(1,null,1),(2,null,2),(3,3,3),(4,null,4),(null,null,null)",
        Vec::new(),
    );
    assert!(
        testkit
            .MustQuery("show stats_histograms where table_name = 't'", Vec::new())
            .Rows()
            .is_empty()
    );
    // 统计版本 2 即使只指定单个索引也会收集整表；断言与 Go 一样只检查命名索引。
    // Statistics version 2 collects the whole table even when a single index
    // is named, so only assert on the named index like Go does.
    testkit.MustExec("analyze table t index idx_b", Vec::new());
    let rows = testkit
        .MustQuery("show stats_histograms where table_name = 't'", Vec::new())
        .Rows();
    assert!(rows.iter().any(|row| row[3] == "b" && row[7] == "4"));
    assert!(
        rows.iter()
            .any(|row| row[3] == "idx_b" && row[7] == "4" && row[10] == "allLoaded")
    );
    testkit.MustExec("analyze table t index idx_c_a", Vec::new());
    let mut names = testkit
        .MustQuery("show stats_histograms where table_name = 't'", Vec::new())
        .Rows()
        .into_iter()
        .map(|row| row[3].clone())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, ["a", "b", "c", "idx_b", "idx_c_a"]);
}

#[test]
/// 验证整数与日期时间类型索引桶上下界格式化。
fn TestShowStatsBuckets() {
    let mut testkit = stats_testkit();
    testkit.MustExec(
        "create table ints(a int, b int, index idx(a,b))",
        Vec::new(),
    );
    testkit.MustExec("insert into ints values(1,1)", Vec::new());
    testkit.MustExec("analyze table ints with 0 topn", Vec::new());
    testkit
        .MustQuery(
            "show stats_buckets where table_name = 'ints' and column_name = 'idx'",
            Vec::new(),
        )
        .Check(vec![vec![
            "test", "ints", "", "idx", "1", "0", "1", "1", "(1, 1)", "(1, 1)", "0",
        ]]);

    for (table, kind, expected) in [
        ("typed_datetime", "datetime", "2020-01-01 00:00:00"),
        ("typed_date", "date", "2020-01-01"),
        ("typed_timestamp", "timestamp", "2020-01-01 00:00:00"),
    ] {
        testkit.MustExec(
            &format!("create table {table}(a {kind}, b int, index idx(a,b))"),
            Vec::new(),
        );
        testkit.MustExec(
            &format!("insert into {table} values('2020-01-01',1)"),
            Vec::new(),
        );
        testkit.MustExec(&format!("analyze table {table} with 0 topn"), Vec::new());
        let rows = testkit
            .MustQuery(
                &format!("show stats_buckets where table_name = '{table}' and column_name = 'idx'"),
                Vec::new(),
            )
            .Rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][8], format!("({expected}, 1)"));
        assert_eq!(rows[0][9], format!("({expected}, 1)"));
    }
}

#[test]
/// 含 NULL 的 datetime 复合索引桶应正确展示 NULL 元组。
fn TestShowStatsBucketWithDateNullValue() {
    let mut testkit = stats_testkit();
    testkit.MustExec(
        "create table t(a datetime, b int, index ia(a,b))",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into t values('2023-12-27',1),(null,2),('2023-12-28',3),(null,4)",
        Vec::new(),
    );
    testkit.MustExec("analyze table t with 0 topn", Vec::new());
    testkit
        .MustQuery(
            "explain format='brief' select * from t where a > 1",
            Vec::new(),
        )
        .Check(vec![
            vec!["IndexReader", "3.20", "root", "index:Selection"],
            vec![
                "└─Selection",
                "3.20",
                "cop[tikv]",
                "gt(cast(test.t.a, double BINARY), 1)",
            ],
            vec![
                "  └─IndexFullScan",
                "4.00",
                "cop[tikv]",
                "table:t, index:ia(a, b) keep order:false",
            ],
        ]);
    let rows = testkit
        .MustQuery(
            "show stats_buckets where table_name = 't' and column_name = 'ia'",
            Vec::new(),
        )
        .Rows();
    assert_eq!(rows.len(), 4);
    assert_eq!(
        rows.iter().map(|row| row[8].as_str()).collect::<Vec<_>>(),
        [
            "(NULL, 2)",
            "(NULL, 4)",
            "(2023-12-27 00:00:00, 1)",
            "(2023-12-28 00:00:00, 3)",
        ]
    );
}

#[test]
/// 单列全 NULL 不产生桶；复合 NULL 元组可展示；truncate 清空直方图。
fn TestShowStatsHasNullValue() {
    let mut testkit = stats_testkit();
    testkit.MustExec("create table single_null(a int, index idx(a))", Vec::new());
    testkit.MustExec("insert into single_null values(null)", Vec::new());
    testkit.MustExec("analyze table single_null with 0 topn", Vec::new());
    assert!(
        testkit
            .MustQuery(
                "show stats_buckets where table_name = 'single_null'",
                Vec::new()
            )
            .Rows()
            .is_empty()
    );
    testkit.MustExec("insert into single_null values(1)", Vec::new());
    testkit.MustExec("analyze table single_null with 0 topn", Vec::new());
    let mut rows = testkit
        .MustQuery(
            "show stats_buckets where table_name = 'single_null'",
            Vec::new(),
        )
        .Rows();
    rows.sort();
    assert_eq!(
        rows,
        vec![
            vec![
                "test",
                "single_null",
                "",
                "a",
                "0",
                "0",
                "1",
                "1",
                "1",
                "1",
                "0"
            ],
            vec![
                "test",
                "single_null",
                "",
                "idx",
                "1",
                "0",
                "1",
                "1",
                "1",
                "1",
                "0",
            ],
        ]
    );

    testkit.MustExec(
        "create table tuple_null(a int, b int, index idx(a,b))",
        Vec::new(),
    );
    testkit.MustExec("insert into tuple_null values(null,null)", Vec::new());
    testkit.MustExec("analyze table tuple_null with 0 topn", Vec::new());
    testkit
        .MustQuery(
            "show stats_buckets where table_name = 'tuple_null'",
            Vec::new(),
        )
        .Check(vec![vec![
            "test",
            "tuple_null",
            "",
            "idx",
            "1",
            "0",
            "1",
            "1",
            "(NULL, NULL)",
            "(NULL, NULL)",
            "0",
        ]]);

    testkit.MustExec(
        "create table truncate_t(a int, b int, c int, index idx_b(b), index idx_c_a(c,a))",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into truncate_t values(1,null,1),(2,null,2),(3,3,3),(4,null,4),(null,null,null)",
        Vec::new(),
    );
    // Statistics version 2 collects the whole table even when a single index
    // is named; Go only checks the named index and its column here.
    testkit.MustExec("analyze table truncate_t index idx_b", Vec::new());
    let rows = testkit
        .MustQuery(
            "show stats_histograms where table_name = 'truncate_t'",
            Vec::new(),
        )
        .Rows();
    assert_eq!(
        rows.iter().filter(|row| row[3] == "idx_b").count(),
        1,
        "{rows:?}"
    );
    assert_eq!(
        rows.iter().filter(|row| row[3] == "b").count(),
        1,
        "{rows:?}"
    );
    testkit.MustExec("truncate table truncate_t", Vec::new());
    assert!(
        testkit
            .MustQuery(
                "show stats_histograms where table_name = 'truncate_t'",
                Vec::new(),
            )
            .Rows()
            .is_empty()
    );
    testkit.MustExec(
        "insert into truncate_t values(1,null,1),(2,null,2),(3,3,3),(4,null,4),(null,null,null)",
        Vec::new(),
    );
    testkit.MustExec("analyze table truncate_t index", Vec::new());
    let mut names = testkit
        .MustQuery(
            "show stats_histograms where table_name = 'truncate_t'",
            Vec::new(),
        )
        .Rows()
        .into_iter()
        .map(|row| row[3].clone())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, ["a", "b", "c", "idx_b", "idx_c_a"]);

    testkit.MustExec("truncate table truncate_t", Vec::new());
    testkit.MustExec(
        "insert into truncate_t values(1,null,1),(2,null,2),(3,3,3),(4,null,4),(null,null,null)",
        Vec::new(),
    );
    testkit.MustExec("analyze table truncate_t", Vec::new());
    let mut rows = testkit
        .MustQuery(
            "show stats_histograms where table_name = 'truncate_t'",
            Vec::new(),
        )
        .Rows();
    rows.sort_by(|left, right| left[3].cmp(&right[3]));
    assert_eq!(
        rows.iter().map(|row| row[7].as_str()).collect::<Vec<_>>(),
        ["1", "4", "1", "4", "0"]
    );
}

#[test]
/// `tidb_snapshot` 下 `SHOW TABLE STATUS` 应能看到已删除表的历史元数据。
fn TestShowStatusSnapshot() {
    let mut testkit = stats_testkit();
    for (index, cache_size) in [1_073_741_824_u64, 0].into_iter().enumerate() {
        let table = format!("snapshot_t{index}");
        testkit.MustExec(
            "set @@global.tidb_schema_cache_size = ?",
            vec![DbValue::U64(cache_size)],
        );
        testkit.MustExec(&format!("create table {table}(a int)"), Vec::new());
        let snapshot = snapshot_now();
        testkit.MustExec(&format!("drop table {table}"), Vec::new());
        assert!(
            !testkit
                .MustQuery("show table status", Vec::new())
                .Rows()
                .iter()
                .any(|row| row[0] == table)
        );
        testkit.MustExec("set @@tidb_snapshot = ?", vec![DbValue::String(snapshot)]);
        assert!(
            testkit
                .MustQuery("show table status", Vec::new())
                .Rows()
                .iter()
                .any(|row| row[0] == table)
        );
        testkit.MustExec("set @@tidb_snapshot = null", Vec::new());
    }
}

#[test]
/// 验证列统计使用时间在普通表与分区表（含 global）上的展示。
fn TestShowColumnStatsUsage() {
    let mut testkit = stats_testkit();
    testkit.MustExec("create table t1(a int, b int, index idx(a,b))", Vec::new());
    testkit.MustExec(
        "create table t2(a int, b int) partition by hash(a) partitions 2",
        Vec::new(),
    );
    let context = testkit.AnalyzeStatsContext().unwrap();
    let t1 = context.table("test", "t1").unwrap().table_id;
    let t2 = context.table("test", "t2").unwrap().table_id;
    let catalog = context.catalog();
    let t2_info = &catalog
        .get(&("test".to_owned(), "t2".to_owned()))
        .unwrap()
        .1;
    let p0 = &t2_info.GetPartitionInfo().unwrap().Definitions[0];
    testkit.MustExec(
        "insert into mysql.column_stats_usage values (?, 1, null, '2021-10-20 08:00:00')",
        vec![DbValue::I64(t1)],
    );
    for physical_id in [t2, p0.ID] {
        testkit.MustExec(
            "insert into mysql.column_stats_usage values (?, 1, '2021-10-20 09:00:00', null)",
            vec![DbValue::I64(physical_id)],
        );
    }
    testkit
        .MustQuery(
            "show column_stats_usage where table_name = 't1'",
            Vec::new(),
        )
        .Check(vec![vec![
            "test",
            "t1",
            "",
            "a",
            "<nil>",
            "2021-10-20 08:00:00",
        ]]);
    let rows = testkit
        .MustQuery(
            "show column_stats_usage where table_name = 't2'",
            Vec::new(),
        )
        .Rows();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|row| row[2] == "global"));
    assert!(rows.iter().any(|row| row[2] == "p0"));
}

#[test]
/// 验证 analyze 任务完成/运行中状态字段，以及分区 merge global 作业。
fn TestShowAnalyzeStatus() {
    let mut testkit = stats_testkit();
    testkit.MustExec("create table job_t(a int, b int, index idx(b))", Vec::new());
    testkit.MustExec("insert into job_t values(1,1),(2,2)", Vec::new());
    testkit.MustExec("analyze table job_t", Vec::new());
    let rows = testkit
        .MustQuery("show analyze status where table_name = 'job_t'", Vec::new())
        .Rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].len(), 12);
    assert_eq!(
        &rows[0][0..5],
        [
            "test",
            "job_t",
            "",
            "analyze table all indexes, all columns with 256 buckets, 100 topn, 1 samplerate",
            "2",
        ]
    );
    assert!(valid_datetime(&rows[0][5]));
    assert!(valid_datetime(&rows[0][6]));
    assert_eq!(rows[0][7], "finished");
    assert_eq!(rows[0][8], "<nil>");
    assert_eq!(rows[0][9], "127.0.0.1:4000");
    assert_eq!(rows[0][10], "<nil>");
    assert_eq!(rows[0][11], "<nil>");

    testkit.MustExec(
        "create table job_p(a int, b int) partition by range(a) (partition p0 values less than (6))",
        Vec::new(),
    );
    testkit.MustExec("insert into job_p values(1,1),(2,2)", Vec::new());
    testkit.MustExec("analyze table job_p", Vec::new());
    let rows = testkit
        .MustQuery("show analyze status where table_name = 'job_p'", Vec::new())
        .Rows();
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .any(|row| row[2] == "global" && row[3] == "merge global stats for test.job_p columns")
    );
    // 无索引分区的 job_info 只提及 columns（与 Go prepareIndexes 行为一致）。
    // Go `prepareIndexes` writes nothing when the table has no index, so the
    // job info of an index-less partition only mentions the columns.
    assert!(rows.iter().any(|row| row[2] == "p0"
        && row[3] == "analyze table all columns with 256 buckets, 100 topn, 1 samplerate"));

    // Use a fresh analyze store for the Go subcase that deletes old jobs before
    // adding an index; the lightweight test store intentionally keeps its
    // in-memory job history when the SQL backing table is deleted from.
    let mut indexed_testkit = stats_testkit();
    indexed_testkit.MustExec(
        "create table job_p(a int, b int) partition by range(a) (partition p0 values less than (6))",
        Vec::new(),
    );
    indexed_testkit.MustExec("insert into job_p values(1,1),(2,2)", Vec::new());
    indexed_testkit.MustExec("alter table job_p add index idx(b)", Vec::new());
    indexed_testkit.MustExec("analyze table job_p index idx", Vec::new());
    let rows = indexed_testkit
        .MustQuery("show analyze status", Vec::new())
        .Rows();
    assert_eq!(rows.len(), 3);
    let mut job_infos = rows.iter().map(|row| row[3].clone()).collect::<Vec<_>>();
    job_infos.sort();
    assert_eq!(
        job_infos,
        [
            "analyze table all indexes, all columns with 256 buckets, 100 topn, 1 samplerate",
            "merge global stats for test.job_p columns",
            "merge global stats for test.job_p's index idx",
        ]
    );

    testkit.MustExec(
        "insert into mysql.analyze_jobs(table_schema,table_name,partition_name,job_info,processed_rows,start_time,end_time,state,fail_reason,instance,process_id) values('test','job_t','','manual running analyze',1,'2026-07-21 12:00:00',null,'running',null,'127.0.0.1:4000',42)",
        Vec::new(),
    );
    let running = testkit
        .MustQuery(
            "show analyze status where table_name = 'job_t' and state = 'running'",
            Vec::new(),
        )
        .Rows();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0][10], "42");
    assert_eq!(running[0][11], "0s");
}

/// 粗检 `YYYY-MM-DD HH:MM:SS` 形态。
fn valid_datetime(value: &str) -> bool {
    value.len() == 19
        && value.as_bytes()[4] == b'-'
        && value.as_bytes()[7] == b'-'
        && value.as_bytes()[10] == b' '
        && value.as_bytes()[13] == b':'
        && value.as_bytes()[16] == b':'
        && value
            .bytes()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7 | 10 | 13 | 16) || byte.is_ascii_digit())
}

/// 生成当前时间的 snapshot 字符串（含微秒），供 `tidb_snapshot` 使用。
fn snapshot_now() -> String {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap();
    let seconds = elapsed.as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);
    let shifted_days = days + 719_468;
    let era = if shifted_days >= 0 {
        shifted_days
    } else {
        shifted_days - 146_096
    } / 146_097;
    let day_of_era = shifted_days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{:06}",
        second_of_day / 3_600,
        second_of_day / 60 % 60,
        second_of_day % 60,
        elapsed.subsec_micros()
    )
}
