// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

#![allow(non_snake_case)]

// `ANALYZE ... COLUMNS` 回归测试。
//
// 同一组场景分别从显式列清单和谓词列追踪两条入口执行，验证主键、普通/聚簇索引、
// 分区表及虚拟列索引会按依赖补齐统计信息，并核对列使用记录、统计元数据、TopN、
// 直方图桶以及分析任务的资源回收。

use std::sync::Arc;

use astersql_parser_ast::ColumnChoice;
use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{Rows, RowsWithSep, TestKit};

const MISSING_SUFFIX: &str = " are missing in ANALYZE but their stats are needed for calculating \
stats for indexes/primary key/extended stats";

/// 创建隔离的测试存储，固定使用统计版本 2，并在写入后同步行数增量。
fn setup(
    create: &str,
    insert: &str,
    partition_mode: Option<&str>,
) -> (TestKit, Arc<AnalyzeStatsStore>) {
    let (store, _) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store.clone());
    testkit.MustExec("use test", Vec::new());
    testkit.MustExec("set @@tidb_analyze_version = 2", Vec::new());
    if let Some(mode) = partition_mode {
        testkit.MustExec(
            &format!("set @@tidb_partition_prune_mode = '{mode}'"),
            Vec::new(),
        );
    }
    testkit.MustExec(create, Vec::new());
    testkit.MustExec(insert, Vec::new());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    (testkit, store)
}

/// 从指定的列选择方式触发分析，并验证分析前的提示或谓词列使用记录。
fn analyze_choice(
    testkit: &mut TestKit,
    store: &AnalyzeStatsStore,
    choice: ColumnChoice,
    list_sql: &str,
    predicate_sql: &str,
    predicate_column: &str,
    missing_columns: &str,
    partitions: &[&str],
) {
    match choice {
        ColumnChoice::List => {
            testkit.MustExec(list_sql, Vec::new());
            // 非分区表只产生一条采样率提示；分区表则为每个物理分区分别提示。
            let mut expected = partitions
                .iter()
                .map(|partition| {
                    vec![
                        "Note".to_owned(),
                        "1105".to_owned(),
                        if partition.is_empty() {
                            "Analyze use auto adjusted sample rate 1.000000 for table test.t, \
                             reason to use this rate is \"use min(1, 110000/10000) as the \
                             sample-rate=1\""
                                .to_owned()
                        } else {
                            format!(
                                "Analyze use auto adjusted sample rate 1.000000 for table test.t's \
                                 partition {partition}, reason to use this rate is \
                                 \"use min(1, 110000/10000) as the sample-rate=1\""
                            )
                        },
                    ]
                })
                .collect::<Vec<_>>();
            expected.push(vec![
                "Warning".to_owned(),
                "1105".to_owned(),
                format!("Columns {missing_columns}{MISSING_SUFFIX}"),
            ]);
            expected.sort();
            assert_eq!(
                testkit.MustQuery("show warnings", Vec::new()).Sort().Rows(),
                expected
            );
        }
        ColumnChoice::Predicate => {
            testkit.MustExec(predicate_sql, Vec::new());
            // 谓词访问先写入持久化使用记录，后续分析据此选择列。
            store
                .domain()
                .dump_col_stats_usage_to_kv()
                .expect("DumpColStatsUsageToKV");
            let usage = testkit
                .MustQuery(
                    "show column_stats_usage where db_name = 'test' and table_name = 't' \
                     and last_used_at is not null",
                    Vec::new(),
                )
                .Rows();
            assert_eq!(usage.len(), 1);
            assert_eq!(usage[0][0], "test");
            assert_eq!(usage[0][1], "t");
            assert_eq!(
                usage[0][2],
                if partitions.iter().any(|partition| !partition.is_empty()) {
                    "global"
                } else {
                    ""
                }
            );
            assert_eq!(usage[0][3], predicate_column);
            testkit.MustExec(
                "analyze table t predicate columns with 2 topn, 2 buckets",
                Vec::new(),
            );
        }
        _ => unreachable!("Go only exercises LIST and PREDICATE"),
    }
}

/// 核对最终被分析的“分区/列”集合；调用方需按查询排序顺序给出期望值。
fn assert_usage(testkit: &TestKit, expected: &[(&str, &str)]) {
    let usage = testkit
        .MustQuery(
            "show column_stats_usage where db_name = 'test' and table_name = 't' \
             and last_analyzed_at is not null",
            Vec::new(),
        )
        .Sort()
        .Rows();
    let actual = usage
        .iter()
        .map(|row| (row[2].as_str(), row[3].as_str()))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

/// 返回当前测试表的逻辑表 ID，用于直接核对统计元数据。
fn table_id(store: &AnalyzeStatsStore) -> i64 {
    store
        .domain()
        .stats_context()
        .table("test", "t")
        .expect("stats table")
        .table_id
}

/// 返回逻辑表及两个物理分区的 ID，顺序与建表语句中的 p0、p1 一致。
fn table_and_partition_ids(store: &AnalyzeStatsStore) -> (i64, i64, i64) {
    let catalog = store.domain().stats_context().catalog();
    let table = &catalog
        .get(&("test".to_owned(), "t".to_owned()))
        .expect("partitioned stats table")
        .1;
    let definitions = &table
        .Partition
        .as_ref()
        .expect("partition metadata")
        .Definitions;
    assert_eq!(definitions.len(), 2);
    (table.ID, definitions[0].ID, definitions[1].ID)
}

/// 验证分析已清空修改计数，并写入预期行数。
fn assert_meta(testkit: &TestKit, id: i64, count: i64) {
    testkit
        .MustQuery(
            &format!("select modify_count, count from mysql.stats_meta where table_id = {id}"),
            Vec::new(),
        )
        .Sort()
        .Check(Rows(&[&format!("0 {count}")]));
}

/// 确保分析完成后会话内存跟踪器和后台工作线程均已释放。
fn assert_no_analyze_leaks(testkit: &TestKit, store: &AnalyzeStatsStore) {
    assert!(
        testkit
            .Session()
            .GetSessionVars()
            .MemTracker()
            .GetChildrenForTest()
            .is_empty(),
        "session memory trackers must be detached"
    );
    assert!(
        store
            .domain()
            .stats_context()
            .analyze_jobs()
            .iter()
            .all(|job| job.active_workers_after == 0),
        "all analyze workers must be joined"
    );
}

#[test]
/// 主键列即使未显式选择，也必须为主键统计自动补齐。
fn TestAnalyzeColumnsWithPrimaryKey() {
    for choice in [ColumnChoice::List, ColumnChoice::Predicate] {
        let (mut testkit, store) = setup(
            "create table t (a int, b int, c int primary key)",
            "insert into t values (1,1,1), (1,1,2), (2,2,3), (2,2,4), (3,3,5), \
             (4,3,6), (5,4,7), (6,4,8), (null,null,9)",
            None,
        );
        analyze_choice(
            &mut testkit,
            &store,
            choice,
            "analyze table t columns a with 2 topn, 2 buckets",
            "select * from t where a > 1",
            "a",
            "c",
            &[""],
        );
        assert_usage(&testkit, &[("", "a"), ("", "c")]);
        let table_id = table_id(&store);
        assert_meta(&testkit, table_id, 9);
        testkit
            .MustQuery(
                &format!(
                    "select is_index, hist_id, distinct_count, null_count, tot_col_size, \
                     stats_ver, truncate(correlation,2) from mysql.stats_histograms \
                     where table_id = {table_id}"
                ),
                Vec::new(),
            )
            .Sort()
            .Check(Rows(&["0 1 6 1 8 2 1", "0 2 0 0 0 0 0", "0 3 9 0 9 2 1"]));
        testkit
            .MustQuery(
                "show stats_topn where db_name = 'test' and table_name = 't'",
                Vec::new(),
            )
            .Sort()
            .Check(Rows(&[
                "test t  a 0 1 2",
                "test t  a 0 2 2",
                "test t  c 0 1 1",
                "test t  c 0 2 1",
            ]));
        testkit
            .MustQuery(
                "show stats_buckets where db_name = 'test' and table_name = 't'",
                Vec::new(),
            )
            .Sort()
            .Check(Rows(&[
                "test t  a 0 0 3 1 3 5 0",
                "test t  a 0 1 4 1 6 6 0",
                "test t  c 0 0 4 1 3 6 0",
                "test t  c 0 1 7 1 7 9 0",
            ]));
        assert_no_analyze_leaks(&testkit, &store);
    }
}

#[test]
/// 复合索引依赖的所有列都应参与分析，并同时生成列统计与索引统计。
fn TestAnalyzeColumnsWithIndex() {
    for choice in [ColumnChoice::List, ColumnChoice::Predicate] {
        let (mut testkit, store) = setup(
            "create table t (a int, b int, c int, d int, index idx_b_d(b, d))",
            "insert into t values (1,1,null,1), (2,1,9,1), (1,1,8,1), (2,2,7,2), \
             (1,3,7,3), (2,4,6,4), (1,4,6,5), (2,4,6,5), (1,5,6,5)",
            None,
        );
        analyze_choice(
            &mut testkit,
            &store,
            choice,
            "analyze table t columns c with 2 topn, 2 buckets",
            "select * from t where c > 1",
            "c",
            "b,d",
            &[""],
        );
        assert_usage(&testkit, &[("", "b"), ("", "c"), ("", "d")]);
        let table_id = table_id(&store);
        assert_meta(&testkit, table_id, 9);
        testkit
            .MustQuery(
                &format!(
                    "select is_index, hist_id, distinct_count, null_count, tot_col_size, \
                     stats_ver, truncate(correlation,2) from mysql.stats_histograms \
                     where table_id = {table_id}"
                ),
                Vec::new(),
            )
            .Sort()
            .Check(Rows(&[
                "0 1 0 0 0 0 0",
                "0 2 5 0 9 2 1",
                "0 3 4 1 8 2 -0.07",
                "0 4 5 0 9 2 1",
                "1 1 6 0 18 2 0",
            ]));
        testkit
            .MustQuery(
                "show stats_topn where db_name = 'test' and table_name = 't'",
                Vec::new(),
            )
            .Sort()
            .Check(RowsWithSep(
                "|",
                &[
                    "test|t||b|0|1|3",
                    "test|t||b|0|4|3",
                    "test|t||c|0|6|4",
                    "test|t||c|0|7|2",
                    "test|t||d|0|1|3",
                    "test|t||d|0|5|3",
                    "test|t||idx_b_d|1|(1, 1)|3",
                    "test|t||idx_b_d|1|(4, 5)|2",
                ],
            ));
        testkit
            .MustQuery(
                "show stats_buckets where db_name = 'test' and table_name = 't'",
                Vec::new(),
            )
            .Sort()
            .Check(RowsWithSep(
                "|",
                &[
                    "test|t||b|0|0|2|1|2|3|0",
                    "test|t||b|0|1|3|1|5|5|0",
                    "test|t||c|0|0|2|1|8|9|0",
                    "test|t||d|0|0|2|1|2|3|0",
                    "test|t||d|0|1|3|1|4|4|0",
                    "test|t||idx_b_d|1|0|3|1|(2, 2)|(4, 4)|0",
                    "test|t||idx_b_d|1|1|4|1|(5, 5)|(5, 5)|0",
                ],
            ));
        assert_no_analyze_leaks(&testkit, &store);
    }
}

#[test]
/// 聚簇复合主键既是行句柄，也是必须随目标列一起分析的索引依赖。
fn TestAnalyzeColumnsWithClusteredIndex() {
    for choice in [ColumnChoice::List, ColumnChoice::Predicate] {
        let (mut testkit, store) = setup(
            "create table t (a int, b int, c int, d int, primary key(b, d) clustered)",
            "insert into t values (1,1,null,1), (2,2,9,2), (1,3,8,3), (2,4,7,4), \
             (1,5,7,5), (2,6,6,6), (1,7,6,7), (2,8,6,8), (1,9,6,9)",
            None,
        );
        analyze_choice(
            &mut testkit,
            &store,
            choice,
            "analyze table t columns c with 2 topn, 2 buckets",
            "select * from t where c > 1",
            "c",
            "b,d",
            &[""],
        );
        assert_usage(&testkit, &[("", "b"), ("", "c"), ("", "d")]);
        let table_id = table_id(&store);
        assert_meta(&testkit, table_id, 9);
        testkit
            .MustQuery(
                &format!(
                    "select is_index, hist_id, distinct_count, null_count, tot_col_size, \
                     stats_ver, truncate(correlation,2) from mysql.stats_histograms \
                     where table_id = {table_id}"
                ),
                Vec::new(),
            )
            .Sort()
            .Check(Rows(&[
                "0 1 0 0 0 0 0",
                "0 2 9 0 9 2 1",
                "0 3 4 1 8 2 -0.07",
                "0 4 9 0 9 2 1",
                "1 1 9 0 18 2 0",
            ]));
        testkit
            .MustQuery(
                "show stats_topn where db_name = 'test' and table_name = 't'",
                Vec::new(),
            )
            .Sort()
            .Check(RowsWithSep(
                "|",
                &[
                    "test|t||PRIMARY|1|(1, 1)|1",
                    "test|t||PRIMARY|1|(2, 2)|1",
                    "test|t||b|0|1|1",
                    "test|t||b|0|2|1",
                    "test|t||c|0|6|4",
                    "test|t||c|0|7|2",
                    "test|t||d|0|1|1",
                    "test|t||d|0|2|1",
                ],
            ));
        testkit
            .MustQuery(
                "show stats_buckets where db_name = 'test' and table_name = 't'",
                Vec::new(),
            )
            .Sort()
            .Check(RowsWithSep(
                "|",
                &[
                    "test|t||PRIMARY|1|0|4|1|(3, 3)|(6, 6)|0",
                    "test|t||PRIMARY|1|1|7|1|(7, 7)|(9, 9)|0",
                    "test|t||b|0|0|4|1|3|6|0",
                    "test|t||b|0|1|7|1|7|9|0",
                    "test|t||c|0|0|2|1|8|9|0",
                    "test|t||d|0|0|4|1|3|6|0",
                    "test|t||d|0|1|7|1|7|9|0",
                ],
            ));
        assert_no_analyze_leaks(&testkit, &store);
    }
}

const PARTITION_INSERT: &str = "insert into t values \
    (1,2,1), (2,4,1), (3,6,1), (4,8,2), (4,8,2), (5,10,3), (5,10,4), \
    (5,10,5), (null,null,6), (11,22,7), (12,24,8), (13,26,9), (14,28,10), \
    (15,30,11), (16,32,12), (16,32,13), (16,32,13), (16,32,14), \
    (17,34,14), (17,34,14)";

/// 精确核对动态/静态分区模式下列与索引的 TopN。
fn assert_partition_topn(testkit: &TestKit, dynamic: bool) {
    let column_rows: &[&str] = if dynamic {
        &[
            "test t global a 0 16 4",
            "test t global a 0 5 3",
            "test t global c 0 1 3",
            "test t global c 0 14 3",
            "test t p0 a 0 4 2",
            "test t p0 a 0 5 3",
            "test t p0 c 0 1 3",
            "test t p0 c 0 2 2",
            "test t p1 a 0 16 4",
            "test t p1 a 0 17 2",
            "test t p1 c 0 13 2",
            "test t p1 c 0 14 3",
        ]
    } else {
        &[
            "test t p0 a 0 4 2",
            "test t p0 a 0 5 3",
            "test t p0 c 0 1 3",
            "test t p0 c 0 2 2",
            "test t p1 a 0 16 4",
            "test t p1 a 0 17 2",
            "test t p1 c 0 13 2",
            "test t p1 c 0 14 3",
        ]
    };
    let index_rows: &[&str] = if dynamic {
        &[
            "test t global idx 1 1 3",
            "test t global idx 1 14 3",
            "test t p0 idx 1 1 3",
            "test t p0 idx 1 2 2",
            "test t p1 idx 1 13 2",
            "test t p1 idx 1 14 3",
        ]
    } else {
        &[
            "test t p0 idx 1 1 3",
            "test t p0 idx 1 2 2",
            "test t p1 idx 1 13 2",
            "test t p1 idx 1 14 3",
        ]
    };
    testkit
        .MustQuery(
            "show stats_topn where db_name = 'test' and table_name = 't' and is_index = 0",
            Vec::new(),
        )
        .Sort()
        .Check(Rows(column_rows));
    testkit
        .MustQuery(
            "show stats_topn where db_name = 'test' and table_name = 't' and is_index = 1",
            Vec::new(),
        )
        .Sort()
        .Check(Rows(index_rows));
}

/// 精确核对动态/静态分区模式下列与索引的所有直方图桶。
fn assert_partition_buckets(testkit: &TestKit, dynamic: bool) {
    let column_rows: &[&str] = if dynamic {
        &[
            "test t global a 0 0 5 2 1 4 0",
            "test t global a 0 1 12 2 11 17 0",
            "test t global c 0 0 6 1 2 6 0",
            "test t global c 0 1 14 2 7 13 0",
            "test t p0 a 0 0 2 1 1 2 0",
            "test t p0 a 0 1 3 1 3 3 0",
            "test t p0 c 0 0 3 1 3 5 0",
            "test t p0 c 0 1 4 1 6 6 0",
            "test t p1 a 0 0 3 1 11 13 0",
            "test t p1 a 0 1 5 1 14 15 0",
            "test t p1 c 0 0 4 1 7 10 0",
            "test t p1 c 0 1 6 1 11 12 0",
        ]
    } else {
        &[
            "test t p0 a 0 0 2 1 1 2 0",
            "test t p0 a 0 1 3 1 3 3 0",
            "test t p0 c 0 0 3 1 3 5 0",
            "test t p0 c 0 1 4 1 6 6 0",
            "test t p1 a 0 0 3 1 11 13 0",
            "test t p1 a 0 1 5 1 14 15 0",
            "test t p1 c 0 0 4 1 7 10 0",
            "test t p1 c 0 1 6 1 11 12 0",
        ]
    };
    let index_rows: &[&str] = if dynamic {
        &[
            "test t global idx 1 0 6 1 2 6 0",
            "test t global idx 1 1 14 2 7 13 0",
            "test t p0 idx 1 0 3 1 3 5 0",
            "test t p0 idx 1 1 4 1 6 6 0",
            "test t p1 idx 1 0 4 1 7 10 0",
            "test t p1 idx 1 1 6 1 11 12 0",
        ]
    } else {
        &[
            "test t p0 idx 1 0 3 1 3 5 0",
            "test t p0 idx 1 1 4 1 6 6 0",
            "test t p1 idx 1 0 4 1 7 10 0",
            "test t p1 idx 1 1 6 1 11 12 0",
        ]
    };
    testkit
        .MustQuery(
            "show stats_buckets where db_name = 'test' and table_name = 't' and is_index = 0",
            Vec::new(),
        )
        .Sort()
        .Check(Rows(column_rows));
    testkit
        .MustQuery(
            "show stats_buckets where db_name = 'test' and table_name = 't' and is_index = 1",
            Vec::new(),
        )
        .Sort()
        .Check(Rows(index_rows));
}

/// 精确核对逻辑表、p0 与 p1 的列/索引直方图元数据。
fn assert_partition_histograms(
    testkit: &TestKit,
    table_id: i64,
    p0_id: i64,
    p1_id: i64,
    dynamic: bool,
) {
    let expected = if dynamic {
        Rows(&[
            &format!("{table_id} 0 1 12 1 19 2 0"),
            &format!("{table_id} 0 2 0 0 0 0 0"),
            &format!("{table_id} 0 3 14 0 20 2 0"),
            &format!("{table_id} 1 1 14 0 0 2 0"),
            &format!("{p0_id} 0 1 5 1 8 2 1"),
            &format!("{p0_id} 0 2 0 0 0 0 0"),
            &format!("{p0_id} 0 3 6 0 9 2 1"),
            &format!("{p0_id} 1 1 6 0 9 2 0"),
            &format!("{p1_id} 0 1 7 0 11 2 1"),
            &format!("{p1_id} 0 2 0 0 0 0 0"),
            &format!("{p1_id} 0 3 8 0 11 2 1"),
            &format!("{p1_id} 1 1 8 0 11 2 0"),
        ])
    } else {
        Rows(&[
            &format!("{table_id} 0 1 0 0 0 0 0"),
            &format!("{table_id} 0 2 0 0 0 0 0"),
            &format!("{table_id} 0 3 0 0 0 0 0"),
            &format!("{table_id} 1 1 0 0 0 0 0"),
            &format!("{p0_id} 0 1 5 1 8 2 1"),
            &format!("{p0_id} 0 2 0 0 0 0 0"),
            &format!("{p0_id} 0 3 6 0 9 2 1"),
            &format!("{p0_id} 1 1 6 0 9 2 0"),
            &format!("{p1_id} 0 1 7 0 11 2 1"),
            &format!("{p1_id} 0 2 0 0 0 0 0"),
            &format!("{p1_id} 0 3 8 0 11 2 1"),
            &format!("{p1_id} 1 1 8 0 11 2 0"),
        ])
    };
    testkit
        .MustQuery(
            "select table_id, is_index, hist_id, distinct_count, null_count, tot_col_size, \
             stats_ver, truncate(correlation,2) from mysql.stats_histograms \
             order by table_id, is_index, hist_id asc",
            Vec::new(),
        )
        .Check(expected);
}

/// 复用同一份数据对比动态与静态分区裁剪模式下的统计作用域。
fn run_partition_case(dynamic: bool) {
    for choice in [ColumnChoice::List, ColumnChoice::Predicate] {
        let (mut testkit, store) = setup(
            "create table t (a int, b int, c int, index idx(c)) partition by range (a) \
             (partition p0 values less than (10), partition p1 values less than maxvalue)",
            PARTITION_INSERT,
            Some(if dynamic { "dynamic" } else { "static" }),
        );
        analyze_choice(
            &mut testkit,
            &store,
            choice,
            "analyze table t columns a with 2 topn, 2 buckets",
            "select * from t where a < 1",
            "a",
            "c",
            &["p0", "p1"],
        );
        // 动态模式会合并全局统计；静态模式只保留各物理分区的统计。
        let usage = if dynamic {
            vec![
                ("global", "a"),
                ("global", "c"),
                ("p0", "a"),
                ("p0", "c"),
                ("p1", "a"),
                ("p1", "c"),
            ]
        } else {
            vec![("p0", "a"), ("p0", "c"), ("p1", "a"), ("p1", "c")]
        };
        assert_usage(&testkit, &usage);
        let (table_id, p0_id, p1_id) = table_and_partition_ids(&store);

        // 全局元数据仅应随动态模式出现，两个物理分区的行数在两种模式下保持一致。
        let meta = testkit
            .MustQuery(
                "show stats_meta where db_name = 'test' and table_name = 't'",
                Vec::new(),
            )
            .Sort()
            .Rows();
        let projected = meta
            .iter()
            .map(|row| {
                vec![
                    row[0].clone(),
                    row[1].clone(),
                    row[2].clone(),
                    row[4].clone(),
                    row[5].clone(),
                ]
            })
            .collect::<Vec<_>>();
        let expected = if dynamic {
            Rows(&["test t global 0 20", "test t p0 0 9", "test t p1 0 11"])
        } else {
            Rows(&["test t p0 0 9", "test t p1 0 11"])
        };
        assert_eq!(projected, expected);

        assert_partition_topn(&testkit, dynamic);
        assert_partition_buckets(&testkit, dynamic);
        assert_partition_histograms(&testkit, table_id, p0_id, p1_id, dynamic);
        assert_no_analyze_leaks(&testkit, &store);
    }
}

#[test]
/// 动态分区模式应同时产出物理分区统计和合并后的全局统计。
fn TestAnalyzeColumnsWithDynamicPartitionTable() {
    run_partition_case(true);
}

#[test]
/// 静态分区模式只产出物理分区统计，不创建全局 TopN 或直方图桶。
fn TestAnalyzeColumnsWithStaticPartitionTable() {
    run_partition_case(false);
}

#[test]
/// 虚拟列索引可由基列值构造索引统计，但虚拟列本身不计入已分析列记录。
fn TestAnalyzeColumnsWithVirtualColumnIndex() {
    for choice in [ColumnChoice::List, ColumnChoice::Predicate] {
        let (mut testkit, store) = setup(
            "create table t (a int, b int, c int as (b+1), index idx(c))",
            "insert into t (a,b) values (1,1), (2,2), (3,3), (4,4), (5,4), \
             (6,5), (7,5), (8,5), (null,null)",
            None,
        );
        analyze_choice(
            &mut testkit,
            &store,
            choice,
            "analyze table t columns b with 2 topn, 2 buckets",
            "select * from t where b > 1",
            "b",
            "c",
            &[""],
        );
        assert_usage(&testkit, &[("", "b")]);
        let table_id = table_id(&store);
        assert_meta(&testkit, table_id, 9);
        testkit
            .MustQuery(
                &format!(
                    "select is_index, hist_id, distinct_count, null_count, stats_ver, \
                     truncate(correlation,2) from mysql.stats_histograms \
                     where table_id = {table_id}"
                ),
                Vec::new(),
            )
            .Sort()
            .Check(Rows(&[
                "0 1 0 0 0 0",
                "0 2 5 1 2 1",
                "0 3 0 0 0 0",
                "1 1 5 1 2 0",
            ]));
        testkit
            .MustQuery(
                "show stats_topn where db_name = 'test' and table_name = 't'",
                Vec::new(),
            )
            .Sort()
            .Check(Rows(&[
                "test t  b 0 4 2",
                "test t  b 0 5 3",
                "test t  idx 1 5 2",
                "test t  idx 1 6 3",
            ]));
        testkit
            .MustQuery(
                "show stats_buckets where db_name = 'test' and table_name = 't'",
                Vec::new(),
            )
            .Sort()
            .Check(Rows(&[
                "test t  b 0 0 2 1 1 2 0",
                "test t  b 0 1 3 1 3 3 0",
                "test t  idx 1 0 2 1 2 3 0",
                "test t  idx 1 1 3 1 4 4 0",
            ]));
        assert_no_analyze_leaks(&testkit, &store);
    }
}
