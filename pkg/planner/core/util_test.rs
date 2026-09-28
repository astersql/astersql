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

// `util` 模块的单元测试：只读判定、聚合/窗口抽取、格式化与脏表/库名大小写。

use std::collections::BTreeSet;

use crate::{
    AggregateFuncExtractor, AstNode, CIString, IsReadOnly, IsReadOnlyInternal, PlannerContext,
    SessionVars, TableInfo, WindowFuncExtractor, extractStringFromBoolSlice,
    extractStringFromStringSet, extractStringFromStringSlice, extractStringFromUint64Slice,
    getLowerDB, tableHasDirtyContent,
};

/// DML 始终非只读；SET GLOBAL 与 EXPLAIN DML 同样非只读。
#[test]
fn readonly_detection_matches_go_dml_and_global_set_rules() {
    let mut vars = SessionVars::default();
    vars.snapshot_tables.insert(7);
    assert!(IsReadOnly(&AstNode::Select(vec![AstNode::Show]), &vars));
    assert!(!IsReadOnly(&AstNode::Update { table_id: 7 }, &vars));
    assert!(!IsReadOnly(&AstNode::Delete { table_id: 8 }, &vars));
    assert!(!IsReadOnly(&AstNode::Set { global: true }, &vars));
    assert!(IsReadOnlyInternal(
        &AstNode::Set { global: true },
        &vars,
        false
    ));
    assert!(!IsReadOnly(
        &AstNode::Explain(Box::new(AstNode::Insert {
            table_id: 7,
            replace: true
        })),
        &vars,
    ));
}

/// 抽取器不进入子查询：仅收集外层一个聚合与一个窗口函数。
#[test]
fn aggregate_and_window_extractors_do_not_cross_subqueries() {
    let root = AstNode::Select(vec![
        AstNode::AggregateFunc {
            name: "sum".to_owned(),
            args: vec![AstNode::Value("a".into())],
        },
        AstNode::WindowFunc {
            name: "row_number".to_owned(),
            args: Vec::new(),
        },
        AstNode::Subquery(vec![
            AstNode::AggregateFunc {
                name: "count".to_owned(),
                args: Vec::new(),
            },
            AstNode::WindowFunc {
                name: "rank".to_owned(),
                args: Vec::new(),
            },
        ]),
    ]);
    let mut aggregates = AggregateFuncExtractor::default();
    aggregates.Extract(&root);
    assert_eq!(aggregates.AggFuncs.len(), 1);
    let mut windows = WindowFuncExtractor::default();
    windows.Extract(&root);
    assert_eq!(windows.WindowFuncs.len(), 1);
}

/// 字符串/数值/布尔拼接、脏表检测与 `getLowerDB` 行为对齐 Go。
#[test]
fn utility_formatting_dirty_tables_and_database_case_match_go() {
    let set = BTreeSet::from(["b".to_owned(), "a".to_owned()]);
    assert_eq!(extractStringFromStringSet(&set), "\"a\",\"b\"");
    let mut slice = vec!["b".to_owned(), "a".to_owned()];
    assert_eq!(extractStringFromStringSlice(&mut slice), "a,b");
    assert_eq!(extractStringFromUint64Slice(&[5, 10, 3]), "10,3,5");
    assert_eq!(extractStringFromBoolSlice(&[true, false]), "false,true");

    let mut context = PlannerContext::default();
    context.vars.dirty_tables.insert(42);
    assert!(tableHasDirtyContent(
        &context,
        &TableInfo {
            id: 42,
            temp_table: false,
            partition_ids: Vec::new(),
        }
    ));
    assert!(!tableHasDirtyContent(
        &context,
        &TableInfo {
            id: 42,
            temp_table: false,
            partition_ids: vec![41, 43],
        }
    ));
    context.vars.dirty_tables.insert(43);
    assert!(tableHasDirtyContent(
        &context,
        &TableInfo {
            id: 100,
            temp_table: false,
            partition_ids: vec![41, 43],
        }
    ));
    let database = CIString::New("TeSt");
    assert_eq!(
        getLowerDB(database.clone(), &SessionVars::default()),
        "test"
    );
    let vars = SessionVars {
        lower_case_table_names: 1,
        ..Default::default()
    };
    assert_eq!(getLowerDB(database, &vars), "test");
}
