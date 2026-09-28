// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// `task` 模块中 PhysicalUnionScan 附着逻辑的单元测试。
//
// UnionScan 会读到事务内未提交的脏写；附着时需保持 Projection/Selection
// 相对位置，避免打乱列投影语义。

use crate::task::{
    FieldType, PlanKind, PlanNode, Task, TypeCode, attach2Task4PhysicalUnionScan, needConvert,
};

fn root_metadata(task: &Task) -> (&[String], bool) {
    match task {
        Task::Root {
            warnings,
            index_join,
            ..
        } => (&warnings.0, *index_join),
        _ => panic!("expected root task"),
    }
}

/// Selection 之下已有 Projection 时，UnionScan 应插在 Projection 与 Selection 之间，
/// 并最终把 Projection 提升为根。
#[test]
fn physical_union_scan_preserves_projection_above_selection() {
    let scan = PlanNode::new(PlanKind::TableScan);
    let projection = PlanNode::new(PlanKind::Projection).with_children(vec![scan.clone()]);
    let selection = PlanNode::new(PlanKind::Selection).with_children(vec![projection]);
    let task = attach2Task4PhysicalUnionScan(
        PlanNode::new(PlanKind::UnionScan),
        vec![Task::root(selection)],
    );
    let root = task.plan().expect("root plan");
    assert_eq!(root.kind, PlanKind::Projection);
    assert_eq!(root.children[0].kind, PlanKind::UnionScan);
    assert_eq!(root.children[0].children[0].kind, PlanKind::Selection);
    assert_eq!(root.children[0].children[0].children[0].kind, scan.kind);
}

/// 子任务根已是 Projection 时，UnionScan 插入其子树，Projection 仍作根。
#[test]
fn physical_union_scan_preserves_existing_projection_root() {
    let scan = PlanNode::new(PlanKind::TableScan);
    let projection = PlanNode::new(PlanKind::Projection).with_children(vec![scan.clone()]);
    let task = attach2Task4PhysicalUnionScan(
        PlanNode::new(PlanKind::UnionScan),
        vec![Task::root(projection)],
    );
    let root = task.plan().expect("root plan");
    assert_eq!(root.kind, PlanKind::Projection);
    assert_eq!(root.children[0].kind, PlanKind::UnionScan);
    assert_eq!(root.children[0].children[0].kind, scan.kind);
}

/// 缺少子 Task 时应返回 Invalid，而不是 panic。
#[test]
fn union_scan_rejects_missing_child_task() {
    let task = attach2Task4PhysicalUnionScan(PlanNode::new(PlanKind::UnionScan), Vec::new());
    assert!(task.is_invalid());
}

/// Go 在原 RootTask 上重排 UnionScan，因此三个计划形状都必须保留已有警告。
#[test]
fn physical_union_scan_preserves_root_task_warnings() {
    let cases = [
        PlanNode::new(PlanKind::Selection).with_children(vec![
            PlanNode::new(PlanKind::Projection)
                .with_children(vec![PlanNode::new(PlanKind::TableScan)]),
        ]),
        PlanNode::new(PlanKind::Projection).with_children(vec![PlanNode::new(PlanKind::TableScan)]),
        PlanNode::new(PlanKind::TableScan),
    ];

    for child in cases {
        let task = Task::Root {
            plan: Some(child),
            warnings: crate::task::TaskWarnings(vec!["kept warning".into()]),
            index_join: true,
        };
        let attached =
            attach2Task4PhysicalUnionScan(PlanNode::new(PlanKind::UnionScan), vec![task]);
        let (warnings, index_join) = root_metadata(&attached);
        assert_eq!(warnings, &["kept warning"]);
        assert!(index_join);
    }
}

#[test]
fn need_convert_matches_go_type_and_decimal_boundaries() {
    let field = |code, flen, decimal| FieldType {
        code,
        flen,
        decimal,
        unsigned: false,
    };

    // Rust 的简化类型模型用宽度变化表示 Go 公共整数类型的物化转换。
    assert!(needConvert(
        &field(TypeCode::Int, 8, 0),
        &field(TypeCode::Int, 20, 0),
    ));
    assert!(!needConvert(
        &field(TypeCode::Decimal, 40, 2),
        &field(TypeCode::Decimal, 65, 2),
    ));
    assert!(needConvert(
        &field(TypeCode::Decimal, 66, 2),
        &field(TypeCode::Decimal, 70, 2),
    ));
}
