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

// `stringer` 模块的单元测试：校验执行计划字符串化与函数依赖（FD）摘要格式。

use crate::{FDToString, PlanKind, PlanNode, ToString, needIncludeChildrenString};

/// 覆盖一元链（Scan->Sel->Projection）与分支 UnionAll 的 `ToString` 行为。
#[test]
fn plan_stringer_formats_unary_and_branching_plans() {
    let scan = PlanNode::New(
        1,
        PlanKind::TableScan {
            table: "t".to_owned(),
        },
        Vec::new(),
    );
    let selection = PlanNode::New(
        2,
        PlanKind::Selection {
            conditions: vec!["eq(t.a, 1)".to_owned()],
        },
        vec![scan],
    );
    let projection = PlanNode::New(3, PlanKind::Projection, vec![selection]);
    assert_eq!(
        ToString(&projection),
        "Table(t)->Sel(eq(t.a, 1))->Projection"
    );
    assert!(!needIncludeChildrenString(&projection));

    let union = PlanNode::New(
        4,
        PlanKind::UnionAll { partition: false },
        vec![
            PlanNode::New(5, PlanKind::Dual, Vec::new()),
            PlanNode::New(6, PlanKind::Dual, Vec::new()),
        ],
    );
    assert!(needIncludeChildrenString(&union));
    assert_eq!(ToString(&union), "UnionAll{Dual->Dual}");
}

/// 仅对维护 FD 的算子收集依赖，并按自底向上顺序用 ` >>> ` 连接。
#[test]
fn fd_stringer_walks_bottom_up_only_for_fd_operators() {
    let mut source = PlanNode::New(
        1,
        PlanKind::DataSource {
            table: "t".to_owned(),
            alias: None,
            partition_id: None,
        },
        Vec::new(),
    );
    source.fd = "a->b".to_owned();
    let mut projection = PlanNode::New(2, PlanKind::Projection, vec![source]);
    projection.fd = "b->c".to_owned();
    assert_eq!(FDToString(&projection), "{a->b} >>> {b->c}");
}

/// Go 的 `fdToString` 在 Apply/Join/UnionAll 处记录当前 FD 后停止，不继续遍历子树。
#[test]
fn fd_stringer_stops_at_non_recursive_fd_operators() {
    let source = || {
        let mut source = PlanNode::New(
            1,
            PlanKind::DataSource {
                table: "t".to_owned(),
                alias: None,
                partition_id: None,
            },
            Vec::new(),
        );
        source.fd = "source-fd".to_owned();
        source
    };

    for (kind, expected) in [
        (PlanKind::Apply, "{apply-fd}"),
        (
            PlanKind::Join {
                equal_conditions: Vec::new(),
            },
            "{apply-fd}",
        ),
        (PlanKind::UnionAll { partition: false }, "{apply-fd}"),
    ] {
        let mut plan = PlanNode::New(2, kind, vec![source()]);
        plan.fd = "apply-fd".to_owned();
        assert_eq!(FDToString(&plan), expected);
    }
}

/// Go 使用 `%v` 格式化 TopN 的字符串切片，元素间以空格分隔且不带 Rust 调试引号。
#[test]
fn plan_stringer_matches_go_collection_and_empty_alias_formatting() {
    let top_n = PlanNode::New(
        1,
        PlanKind::TopN {
            by_items: vec!["t.a".to_owned(), "t.b true".to_owned()],
            offset: 2,
            count: 3,
        },
        Vec::new(),
    );
    assert_eq!(ToString(&top_n), "TopN([t.a t.b true],2,3)");

    let source = PlanNode::New(
        2,
        PlanKind::DataSource {
            table: "t".to_owned(),
            alias: Some(String::new()),
            partition_id: None,
        },
        Vec::new(),
    );
    assert_eq!(ToString(&source), "DataScan(t)");

    let index = PlanNode::New(
        3,
        PlanKind::IndexScan {
            table: "t".to_owned(),
            index: "idx".to_owned(),
            ranges: vec!["[1,1]".to_owned(), "[3,+inf]".to_owned()],
        },
        Vec::new(),
    );
    assert_eq!(ToString(&index), "Index(t.idx)[[1,1] [3,+inf]]");
}

/// Go 对 ExchangeReceiver 明确跳过子计划遍历。
#[test]
fn plan_stringer_exchange_receiver_ignores_children() {
    let receiver = PlanNode::New(
        1,
        PlanKind::ExchangeReceiver {
            task_ids: vec![7, 9],
        },
        vec![PlanNode::New(2, PlanKind::Dual, Vec::new())],
    );
    assert_eq!(ToString(&receiver), "Recv(7, 9, )");
}
