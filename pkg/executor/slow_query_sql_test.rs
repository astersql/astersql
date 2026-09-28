// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 慢查询（slow query）日志行切分与运行时统计合并的单元测试。
//
// 慢日志按行解析；`\r\n` 需规范化。`slowQueryRuntimeStats` 聚合多文件读耗时与并发度。

use std::time::Duration;

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Rows, TestKit};

use crate::slow_query::{calculateLogSize, slowQueryRuntimeStats, splitSlowLogLines};

fn slow_query_testkit() -> TestKit {
    TestKit::new(CreateMockStoreAndDomain().0)
}

/// Go `TestSlowQueryWithoutSlowLog`: the virtual table is empty before slow
/// logging is enabled and remains queryable with a time predicate.
#[test]
fn slow_query_without_slow_log_is_empty() {
    let tk = slow_query_testkit();
    tk.MustQuery(
        "select query from information_schema.slow_query",
        Vec::new(),
    )
    .Check(Rows(&[]));
    tk.MustQuery(
        "select query from information_schema.slow_query where time > '2020-09-15 12:16:39' and time < now()",
        Vec::new(),
    )
    .Check(Rows(&[]));
}

/// Exercise the real session -> executor -> information_schema slow-query
/// path rather than replacing Go's SQL integration assertion with helpers.
#[test]
fn slow_query_threshold_records_statement_and_plan() {
    let mut tk = slow_query_testkit();
    tk.MustExec("set tidb_slow_log_threshold=0", Vec::new());
    tk.MustExec("create table t (a int)", Vec::new());
    tk.MustExec("insert into t values (1)", Vec::new());

    tk.MustQuery(
        "select query from information_schema.slow_query where query = 'insert into t values (1);'",
        Vec::new(),
    )
    .Check(Rows(&["insert into t values (1);"]));
    tk.MustQuery(
        "select plan from information_schema.slow_query where query = 'insert into t values (1);'",
        Vec::new(),
    )
    .Check(Rows(&[
        "Insert time: 1 loops: 1 prepare: 1 check_insert: 1 mem_insert_time: 1 prefetch: 1 rpc: 1",
    ]));
}

/// 验证慢日志分行、日志字节估算，以及 runtime stats 的 Merge/String。
#[test]
fn slow_query_sql_blocks_preserve_lines_and_merge_runtime_stats() {
    // CRLF 规范化后按行切分，末尾空行丢弃。
    let lines = splitSlowLogLines("# Time: 1\r\nselect 1;\r\n");
    assert_eq!(lines, vec!["# Time: 1", "select 1;"]);
    assert_eq!(calculateLogSize(&lines), 18);

    // Merge 累加文件数/耗时，并发度取 max。
    let mut total = slowQueryRuntimeStats {
        totalFileNum: 1,
        readFileNum: 1,
        readFile: Duration::from_millis(2),
        concurrent: 1,
        ..Default::default()
    };
    total.Merge(&slowQueryRuntimeStats {
        totalFileNum: 2,
        readFileNum: 1,
        readFile: Duration::from_millis(3),
        concurrent: 4,
        ..Default::default()
    });
    assert_eq!(total.totalFileNum, 3);
    assert_eq!(total.readFileNum, 2);
    assert_eq!(total.readFile, Duration::from_millis(5));
    assert_eq!(total.concurrent, 4);
    assert!(total.String().contains("concurrency:4"));
}
