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

// TestKit 核心行为单元测试。
//
// 覆盖多语句执行后语句上下文（statement context）与 MemTracker 子节点是否泄漏等问题。

use crate::db_driver::{DbValue, QueryRows};
use crate::mockstore::{CreateMockStoreAndDomain, MockStore, MockStoreConfig};
use crate::{Rows, TestKit};
use std::sync::Arc;

/// TestMultiStatementInTk tests whether statement context will leak with
/// multi-statements in testkit. See #47365.
/// 多语句场景下重复查询，断言 MemTracker 子节点始终为 0，避免语句上下文泄漏（#47365）。
#[test]
fn TestMultiStatementInTk() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    assert_eq!(
        tk.Session()
            .GetSessionVars()
            .MemTracker()
            .GetChildrenForTest()
            .len(),
        0
    );
    for _ in 0..100 {
        // should return the first result set
        // 多语句只返回第一个结果集；随后检查内存追踪子节点未累积。
        tk.MustQuery("select 1;select 2;", Vec::new())
            .Check(Rows(&["1"]));
        assert_eq!(
            tk.Session()
                .GetSessionVars()
                .MemTracker()
                .GetChildrenForTest()
                .len(),
            0
        );
    }
}

#[test]
fn plan_assertions_use_the_access_object_column() {
    let store = Arc::new(MockStore::new(MockStoreConfig::default()));
    store.expect_query(
        "explain select * from t",
        QueryRows {
            columns: vec![
                "id".into(),
                "estRows".into(),
                "task".into(),
                "access object".into(),
            ],
            rows: vec![vec![
                DbValue::String("IndexReader".into()),
                DbValue::String("1".into()),
                DbValue::String("root".into()),
                DbValue::String("table:t, index:idx_a".into()),
            ]],
        },
    );
    store.expect_query(
        "explain select * from no_index",
        QueryRows {
            columns: vec![
                "id".into(),
                "estRows".into(),
                "task".into(),
                "access object".into(),
            ],
            rows: vec![vec![
                DbValue::String("IndexReader".into()),
                DbValue::String("1".into()),
                DbValue::String("root".into()),
                DbValue::String("table:no_index".into()),
            ]],
        },
    );
    let tk = TestKit::new(store);
    tk.MustUseIndex("select * from t", "idx_a");
    tk.MustNoIndexUsed("select * from no_index");
}

#[test]
fn plan_and_operator_info_helpers_use_the_go_columns() {
    let store = Arc::new(MockStore::new(MockStoreConfig::default()));
    store.expect_query(
        "explain select * from misleading_plan",
        QueryRows {
            columns: vec![
                "id".into(),
                "estRows".into(),
                "task".into(),
                "access object".into(),
                "operator info".into(),
            ],
            rows: vec![vec![
                DbValue::String("TableReader".into()),
                DbValue::String("1".into()),
                DbValue::String("root".into()),
                DbValue::String("table:t, index:Point_Get_decoy".into()),
                DbValue::String("keep order:false".into()),
            ]],
        },
    );
    store.expect_query(
        "explain select * from misleading_operator_info",
        QueryRows {
            columns: vec![
                "id".into(),
                "estRows".into(),
                "task".into(),
                "access object".into(),
                "operator info".into(),
                "execution info".into(),
            ],
            rows: vec![vec![
                DbValue::String("TableReader".into()),
                DbValue::String("1".into()),
                DbValue::String("root".into()),
                DbValue::String("table:t".into()),
                DbValue::String("keep order:false".into()),
                DbValue::String("keyword_decoy".into()),
            ]],
        },
    );
    store.expect_query(
        "explain select * from compact_operator_info",
        QueryRows {
            columns: vec![
                "id".into(),
                "task".into(),
                "access object".into(),
                "operator info".into(),
            ],
            rows: vec![vec![
                DbValue::String("IndexMerge".into()),
                DbValue::String("root".into()),
                DbValue::String(String::new()),
                DbValue::String("type: intersection".into()),
            ]],
        },
    );
    store.expect_query(
        "explain select * from single_column_operator_info",
        QueryRows {
            columns: vec!["plan".into()],
            rows: vec![vec![DbValue::String(
                "IndexMerge root type: intersection, limit embedded(offset:0, count:1)".into(),
            )]],
        },
    );

    let tk = TestKit::new(store);
    assert!(!tk.HasPlan("select * from misleading_plan", "Point_Get"));
    assert!(
        !tk.HasKeywordInOperatorInfo("select * from misleading_operator_info", "keyword_decoy")
    );
    assert!(tk.HasKeywordInOperatorInfo("select * from compact_operator_info", "intersection"));
    assert!(tk.HasKeywordInOperatorInfo(
        "select * from single_column_operator_info",
        "limit embedded"
    ));
}
