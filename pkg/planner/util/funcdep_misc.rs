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

// 函数依赖（functional dependency, FD）相关杂项提取。
//
// 从过滤条件中抽取非空列、常量列与等价列对，供 FDSet（函数依赖集合）
// 维护与后续优化（如列剪枝、半连接简化）使用。

use expression::Expression as _;
use expression::exprctx::BuildContext as _;

use crate::IsNullRejected;

/// 从条件中收集「空值拒绝」成立的列 UniqueID 集合。
///
/// 空值拒绝（null-reject）：当列取 NULL 时谓词不可能为 TRUE，
/// 因而该列在满足条件下必然非空。
pub fn ExtractNotNullFromConds(
    conditions: &[expression::ExprBox],
    context: &dyn plan_base::PlanContext,
) -> intset::FastIntSet {
    let mut not_null = intset::NewFastIntSet(Vec::new());
    for condition in conditions {
        // 取出条件中出现的列，逐列用单列表 schema 做 null-reject 判定。
        let columns =
            expression::ExtractColumnsMapFromExpressions(|_| true, std::slice::from_ref(condition));
        for column in columns.into_values() {
            let schema = expression::NewSchema(vec![column.clone()]);
            if IsNullRejected(context, &schema, condition.clone()) {
                not_null.Insert(column.UniqueID as i32);
            }
        }
    }
    not_null
}

/// 从等值常量条件中抽取列为常量的 UniqueID；标量函数会注册到 FDSet 后分配 ID。
pub fn ExtractConstantCols(
    conditions: &[expression::ExprBox],
    context: &dyn plan_base::PlanContext,
    dependencies: &mut funcdep::FDSet,
) -> intset::FastIntSet {
    let mut ids = intset::NewFastIntSet(Vec::new());
    let objects =
        expression::ExtractConstantEqColumnsOrScalar(context.GetExprCtx(), Vec::new(), conditions);
    for object in objects {
        if let Some(column) = object.as_any().downcast_ref::<expression::Column>() {
            ids.Insert(column.UniqueID as i32);
        } else if object.as_any().is::<expression::ScalarFunction>() {
            // 非常量列表达式：用哈希注册到 FDSet，复用或新分配 UniqueID。
            ids.Insert(uniqueIDForExpression(
                context,
                dependencies,
                object.as_ref(),
            ));
        }
    }
    ids
}

/// 从等价条件中抽取成对 UniqueID 集合（左右各一个 FastIntSet）。
pub fn ExtractEquivalenceCols(
    conditions: &[expression::ExprBox],
    context: &dyn plan_base::PlanContext,
    dependencies: &mut funcdep::FDSet,
) -> Vec<[intset::FastIntSet; 2]> {
    expression::ExtractEquivalenceColumns(Vec::new(), conditions)
        .into_iter()
        .filter_map(|pair| {
            if pair.len() != 2 {
                return None;
            }
            let left = uniqueIDForExpression(context, dependencies, pair[0].as_ref());
            let right = uniqueIDForExpression(context, dependencies, pair[1].as_ref());
            Some([
                intset::NewFastIntSet(vec![left]),
                intset::NewFastIntSet(vec![right]),
            ])
        })
        .collect()
}

/// 列直接取其 UniqueID；其它表达式按 HashCode 在 FDSet 中查找或新分配计划列 ID。
fn uniqueIDForExpression(
    context: &dyn plan_base::PlanContext,
    dependencies: &mut funcdep::FDSet,
    object: &dyn expression::Expression,
) -> i32 {
    if let Some(column) = object.as_any().downcast_ref::<expression::Column>() {
        return column.UniqueID as i32;
    }
    let hash = hashCodeKey(&object.HashCode());
    let (registered, found) = dependencies.IsHashCodeRegistered(&hash);
    if found {
        return registered;
    }
    let allocated = context.GetExprCtx().AllocPlanColumnID() as i32;
    dependencies.RegisterUniqueID(hash, allocated);
    allocated
}

/// 将 Go 可用作 string map key 的任意哈希字节转换为 Rust String key。
pub(crate) fn hashCodeKey(hash_code: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut key = String::with_capacity(hash_code.len() * 2);
    for byte in hash_code {
        key.push(HEX[(byte >> 4) as usize] as char);
        key.push(HEX[(byte & 0x0f) as usize] as char);
    }
    key
}
