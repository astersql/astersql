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

// `exchange_partition_test.go::TestExchangeRangeColumnsPartition` 的可运行 Rust 对照。
// 覆盖多列 RANGE COLUMNS 的 NULL、整数极值、边界字符串、五个分区往返交换，
// 以及每个非目标分区行均被 WITH VALIDATION 拒绝的错误路径。

use std::collections::HashMap;

use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateAnalyzeStatsStore;

#[test]
fn test_exchange_range_columns_partition() {
    let store = CreateAnalyzeStatsStore();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@tidb_enable_exchange_partition=1", Vec::new());
    tk.MustExec(
        "CREATE TABLE t1 (id INT NOT NULL, age INT, name VARCHAR(50)) \
         PARTITION BY RANGE COLUMNS(age, name) (\
         PARTITION p0 VALUES LESS THAN (20, 'm'),\
         PARTITION p1 VALUES LESS THAN (30, 'm'),\
         PARTITION p2 VALUES LESS THAN (30, MAXVALUE),\
         PARTITION p3 VALUES LESS THAN (40, 'm'),\
         PARTITION p4 VALUES LESS THAN (MAXVALUE, MAXVALUE))",
        Vec::new(),
    );

    let age_values = [
        None,
        Some(i32::MIN),
        Some(i32::MAX),
        Some(0),
        Some(19),
        Some(20),
        Some(29),
        Some(30),
        Some(39),
        Some(40),
    ];
    let name_values = [None, Some(""), Some("l"), Some("m"), Some("n")];
    let mut id = 0;
    let mut values = Vec::new();
    for age in age_values {
        for name in name_values {
            id += 1;
            let age = age.map_or_else(|| "NULL".to_owned(), |age| age.to_string());
            let name = name.map_or_else(
                || "NULL".to_owned(),
                |name| format!("'{}'", name.replace('\'', "''")),
            );
            values.push(format!("({id}, {age}, {name})"));
        }
    }
    tk.MustExec(
        &format!("INSERT INTO t1 VALUES {}", values.join(",")),
        Vec::new(),
    );

    let partition_names = ["p0", "p1", "p2", "p3", "p4"];
    let mut initial_results = HashMap::new();
    for partition in partition_names {
        let mut result = tk.MustQuery(
            &format!("SELECT * FROM t1 PARTITION({partition})"),
            Vec::new(),
        );
        result.Sort();
        initial_results.insert(partition, result.Rows());
    }

    tk.MustExec(
        "CREATE TABLE t2 (id INT NOT NULL, age INT, name VARCHAR(50))",
        Vec::new(),
    );
    for (index, partition) in partition_names.iter().enumerate() {
        tk.MustExec(
            &format!("ALTER TABLE t1 EXCHANGE PARTITION {partition} WITH TABLE t2"),
            Vec::new(),
        );
        tk.MustQuery(
            &format!("SELECT COUNT(*) FROM t1 PARTITION({partition})"),
            Vec::new(),
        )
        .Check(vec![vec!["0"]]);

        let mut exchanged = tk.MustQuery("SELECT * FROM t2", Vec::new());
        exchanged.Sort();
        exchanged.Check(initial_results[partition].clone());

        tk.MustExec(
            &format!("ALTER TABLE t1 EXCHANGE PARTITION {partition} WITH TABLE t2"),
            Vec::new(),
        );
        let mut restored = tk.MustQuery(
            &format!("SELECT * FROM t1 PARTITION({partition})"),
            Vec::new(),
        );
        restored.Sort();
        restored.Check(initial_results[partition].clone());

        let other_partitions = partition_names
            .iter()
            .enumerate()
            .filter_map(|(other_index, name)| (other_index != index).then_some(*name))
            .collect::<Vec<_>>()
            .join(",");
        for row_id in 1..=id {
            let row = tk.MustQuery(
                &format!("SELECT * FROM t1 PARTITION({partition}) WHERE id = {row_id}"),
                Vec::new(),
            );
            if !row.Rows().is_empty() {
                continue;
            }
            tk.MustExec(
                &format!(
                    "INSERT INTO t2 SELECT * FROM t1 PARTITION({other_partitions}) WHERE id = {row_id}"
                ),
                Vec::new(),
            );
            tk.MustContainErrMsg(
                &format!(
                    "ALTER TABLE t1 EXCHANGE PARTITION {partition} WITH TABLE t2 /* j = {row_id} */"
                ),
                "[ddl:1737]Found a row that does not match the partition",
            );
            tk.MustExec("TRUNCATE TABLE t2", Vec::new());
        }
    }

    tk.MustExec("DROP TABLE t2", Vec::new());
    tk.MustExec("DROP TABLE t1", Vec::new());
}
