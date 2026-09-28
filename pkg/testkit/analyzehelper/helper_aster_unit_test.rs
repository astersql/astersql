// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// ANALYZE 辅助函数的单元测试。
//
// 通过记录 SQL 与落盘次数的可控运行时，验证谓词列查询的生成顺序，
// 以及查询失败后必须短路、不得继续落盘列统计用量的错误传播契约。

use crate::{AnalyzeError, AnalyzeRuntime, TriggerPredicateColumnsCollection};

#[derive(Default)]
/// 记录辅助函数副作用，并可在指定查询处注入失败的测试运行时。
struct MockRuntime {
    queries: Vec<String>,
    dump_count: usize,
    fail_on_query: Option<usize>,
    fail_on_dump: bool,
    events: Vec<String>,
}

impl AnalyzeRuntime for MockRuntime {
    fn execute(&mut self, sql: &str) -> Result<(), AnalyzeError> {
        self.queries.push(sql.to_owned());
        self.events.push(format!("query:{sql}"));
        if self.fail_on_query == Some(self.queries.len()) {
            return Err(AnalyzeError("execute failed".into()));
        }
        Ok(())
    }

    fn dump_column_stats_usage_to_kv(&mut self) -> Result<(), AnalyzeError> {
        self.dump_count += 1;
        self.events.push("dump".into());
        if self.fail_on_dump {
            return Err(AnalyzeError("dump failed".into()));
        }
        Ok(())
    }
}

#[test]
fn generated_queries_and_dump_order_match_go() {
    let mut runtime = MockRuntime::default();
    TriggerPredicateColumnsCollection(&mut runtime, "test.t", &["a+b".into(), "plain".into()])
        .unwrap();

    assert_eq!(
        runtime.queries,
        [
            "SELECT * FROM test.t WHERE a+b = '1'",
            "SELECT * FROM test.t WHERE plain = '1'",
        ]
    );
    assert_eq!(runtime.dump_count, 1);
    assert_eq!(
        runtime.events,
        [
            "query:SELECT * FROM test.t WHERE a+b = '1'",
            "query:SELECT * FROM test.t WHERE plain = '1'",
            "dump",
        ]
    );

    // 即使没有谓词列，也必须执行一次统计用量落盘，与 Go 实现保持一致。
    let mut empty = MockRuntime::default();
    TriggerPredicateColumnsCollection(&mut empty, "t", &[]).unwrap();
    assert!(empty.queries.is_empty());
    assert_eq!(empty.dump_count, 1);
}

#[test]
fn query_failure_stops_before_remaining_queries_and_dump() {
    let mut runtime = MockRuntime {
        // 在第二条查询入队后失败，以便同时观察已执行前缀与后续短路行为。
        fail_on_query: Some(2),
        ..MockRuntime::default()
    };
    let error =
        TriggerPredicateColumnsCollection(&mut runtime, "t", &["a".into(), "b".into(), "c".into()])
            .unwrap_err();

    assert_eq!(error, AnalyzeError("execute failed".into()));
    assert_eq!(
        runtime.queries,
        [
            "SELECT * FROM t WHERE a = '1'",
            "SELECT * FROM t WHERE b = '1'"
        ]
    );
    assert_eq!(runtime.dump_count, 0);
}

#[test]
fn dump_failure_is_propagated_after_all_queries() {
    let mut runtime = MockRuntime {
        fail_on_dump: true,
        ..MockRuntime::default()
    };

    let error = TriggerPredicateColumnsCollection(&mut runtime, "t", &["a".into(), "b".into()])
        .unwrap_err();

    assert_eq!(error, AnalyzeError("dump failed".into()));
    assert_eq!(runtime.dump_count, 1);
    assert_eq!(
        runtime.events,
        [
            "query:SELECT * FROM t WHERE a = '1'",
            "query:SELECT * FROM t WHERE b = '1'",
            "dump",
        ]
    );
}
