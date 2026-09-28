// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// TRACE 执行器单元测试：树形前缀布局与 log 事件过滤。
//
// 块注释内保留自 Go 的集成测试草稿；下方可运行用例校验 `dfsTree` 与 `generateLogResult`。

/*
// TRACE 语句在不同 format 下返回行数以及 row 格式 snapshotTS 有序性的检查。

// test_trace_exec 对应 Go 的 TestTraceExec：创建 trace 表后覆盖 insert/select/delete/analyze 的 trace 输出。
#[test]
fn test_trace_exec() {
    let store = testkit::CreateMockStore();
    let mut tk = testkit::NewTestKit(store);
    tk.MustExec("use test");
    tk.MustExec("create table trace (id int PRIMARY KEY AUTO_INCREMENT, c1 int, c2 int, c3 int default 1);");
    tk.MustExec("trace insert into trace (c1, c2, c3) values (1, 2, 3)");
    let mut rows = tk.MustQuery("trace select * from trace where id = 0;").Rows();
    require::GreaterOrEqual(rows.len(), 1);

    // Go 注释里给出了 TRACE row 格式示例，核心检查是 operation/snapshotTS/duration 多行返回且 snapshotTS 有序。
    rows = tk.MustQuery("trace format='row' select * from trace where id = 0;").Rows();
    require::Greater(rows.len(), 1);
    require::True(rows_ordered(&rows));

    rows = tk.MustQuery("trace format='row' delete from trace where id = 0").Rows();
    require::Greater(rows.len(), 1);
    require::True(rows_ordered(&rows));

    rows = tk.MustQuery("trace format='row' analyze table trace").Rows();
    require::Greater(rows.len(), 1);
    require::True(rows_ordered(&rows));

    tk.MustExec("trace format='log' insert into trace (c1, c2, c3) values (1, 2, 3)");
    rows = tk.MustQuery("trace format='log' select * from trace where id = 0;").Rows();
    require::GreaterOrEqual(rows.len(), 1);
}

// rows_ordered 对应 Go 的 rowsOrdered：要求每行第二列为 string，并按字符串顺序非递减。
fn rows_ordered(rows: &[Vec<any::Any>]) -> bool {
    for idx in 0..rows.len() {
        if !rows[idx][1].is::<String>() {
            return false;
        }
        if idx == 0 {
            continue;
        }
        if rows[idx - 1][1].as_string() > rows[idx][1].as_string() {
            return false;
        }
    }
    true
}
*/

use crate::trace::{
    RawSpan, TraceChunk, TraceLog, TraceLogField, TraceNode, TraceTimespan, dfsTree,
    generateLogResult,
};

#[derive(Default)]
/// 简易行缓冲：按列追加字符串，列 0 开启新行。
struct TestChunk {
    rows: Vec<Vec<String>>,
}

impl TestChunk {
    /// 列 0 新建一行，再写入指定列。
    fn append(&mut self, column: usize, value: String) {
        if column == 0 {
            self.rows.push(Vec::new());
        }
        let row = self.rows.last_mut().expect("column zero starts a row");
        while row.len() <= column {
            row.push(String::new());
        }
        row[column] = value;
    }
}

/// 将 TraceChunk 操作落到字符串行上，便于断言。
impl TraceChunk for TestChunk {
    type Time = u64;

    fn reset(&mut self) {
        self.rows.clear();
    }

    fn num_rows(&self) -> usize {
        self.rows.len()
    }

    fn append_string(&mut self, column: usize, value: &str) {
        self.append(column, value.to_owned());
    }

    fn append_bytes(&mut self, column: usize, value: &[u8]) {
        self.append(column, String::from_utf8_lossy(value).into_owned());
    }

    fn append_time(&mut self, column: usize, value: &Self::Time) {
        self.append(column, value.to_string());
    }
}

#[test]
/// 子节点按 start_order 排序，并保留 ├─ / └─ 树前缀。
fn trace_tree_sorts_children_and_preserves_row_prefixes() {
    // late 的 start_order 更大，排序后应排在 early 之后。
    let mut root = TraceNode {
        operation: "root".to_owned(),
        timespan: Some(TraceTimespan {
            start_order: 1,
            formatted_start: "00:00:01.000000".to_owned(),
            formatted_duration: "3ms".to_owned(),
        }),
        children: vec![
            TraceNode {
                operation: "late".to_owned(),
                timespan: Some(TraceTimespan {
                    start_order: 3,
                    formatted_start: "00:00:01.002000".to_owned(),
                    formatted_duration: "1ms".to_owned(),
                }),
                children: Vec::new(),
            },
            TraceNode {
                operation: "early".to_owned(),
                timespan: Some(TraceTimespan {
                    start_order: 2,
                    formatted_start: "00:00:01.001000".to_owned(),
                    formatted_duration: "1ms".to_owned(),
                }),
                children: Vec::new(),
            },
        ],
    };
    let mut chunk = TestChunk::default();
    // 根无前缀；首子 ├─，末子 └─。
    dfsTree(&mut root, "", false, &mut chunk);
    assert_eq!(chunk.rows[0][0], "root");
    assert_eq!(chunk.rows[1][0], "  ├─early");
    assert_eq!(chunk.rows[2][0], "  └─late");
    assert_eq!(root.children[0].operation, "early");
    assert_eq!(root.children[1].operation, "late");
}

#[test]
/// log 格式只输出 key=event 的字段，并附带 span tags。
fn trace_log_emits_only_event_fields_with_span_tags() {
    // ignored 字段不应出现在结果行中。
    let spans = vec![RawSpan {
        operation: "select".to_owned(),
        start: 10,
        formatted_tags: Some("table=t".to_owned()),
        logs: vec![TraceLog {
            timestamp: 11,
            fields: vec![
                TraceLogField {
                    key: "event".to_owned(),
                    value: "read row".to_owned(),
                },
                TraceLogField {
                    key: "ignored".to_owned(),
                    value: "not emitted".to_owned(),
                },
            ],
        }],
    }];
    let mut chunk = TestChunk::default();
    generateLogResult(&spans, &mut chunk);
    assert_eq!(chunk.rows.len(), 2);
    assert_eq!(chunk.rows[1], vec!["11", "read row", "table=t", "select"]);
}
