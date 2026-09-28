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

// TopN 下推时“重函数”（向量距离等计算昂贵的表达式）优化路径的单元测试。
//
// 验证 `tryReturnDistanceFromIndex`：在 Local TopN 前插入 Projection，
// 把重函数物化为列，避免在排序键中重复求值。

use crate::task::{
    ContainHeavyFunction, Expression, FieldType, PlanKind, PlanNode, StoreType, TypeCode,
    getPushedDownTopN,
};

/// 构造测试用 Float 字段类型。
fn float_type() -> FieldType {
    FieldType {
        code: TypeCode::Float,
        flen: 22,
        decimal: -1,
        unsigned: false,
    }
}

/// 排序键中轻函数在前、重函数在后时，仍应成功物化距离列并改写 by_items。
#[test]
fn pushed_down_topn_handles_heavy_function_after_first_by_item() {
    let mut child = PlanNode::new(PlanKind::TableScan);
    child.schema = vec![float_type(), float_type()];
    let light = Expression {
        name: "coalesce".to_owned(),
        return_type: Some(float_type()),
        ..Default::default()
    };
    let heavy = Expression {
        name: "vec_cosine_distance".to_owned(),
        function_count: 1,
        return_type: Some(float_type()),
        ..Default::default()
    };
    assert!(!ContainHeavyFunction(&light));
    assert!(ContainHeavyFunction(&heavy));

    let mut top_n = PlanNode::new(PlanKind::TopN);
    top_n.by_items = vec![light.clone(), heavy];
    top_n.count = 10;

    let (local, global) = getPushedDownTopN(&top_n, &child, StoreType::TiFlash);
    let local = local.expect("heavy-function TopN should be pushed down");
    let global = global.expect("heavy-function optimization should retain a global TopN");
    assert_eq!(local.by_items[0].name, light.name);
    assert_eq!(local.by_items[1].column, Some(child.schema.len()));
    assert_eq!(global.by_items[1].column, Some(child.schema.len()));
    assert_eq!(local.children[0].kind, PlanKind::Projection);
    assert_eq!(local.children[0].schema.len(), child.schema.len() + 1);
}
