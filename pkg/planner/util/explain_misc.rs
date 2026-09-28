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

// EXPLAIN 相关杂项辅助。
//
// 将 ORDER BY / GROUP BY 等 `ByItems`（排序或分组项，含表达式与升降序）
// 格式化为可读的 Explain 文本片段，供执行计划展示使用。

use std::fmt::Write;

use expression::Expression as _;

use crate::ByItems;

/// 把一组 ByItems 写入 buffer：降序项追加 `:desc`，多项之间用 `, ` 分隔。
///
/// `EvalContext` 为表达式求值上下文；返回同一可变 buffer 便于链式拼接。
pub fn ExplainByItems<'a>(
    context: &dyn expression::exprctx::EvalContext,
    buffer: &'a mut String,
    items: &[ByItems],
) -> &'a mut String {
    for (index, item) in items.iter().enumerate() {
        // 先输出表达式自身的 Explain 文本，再按 Desc 决定是否标注降序。
        let explanation = item.Expr.ExplainInfo(context);
        if item.Desc {
            write!(buffer, "{explanation}:desc").expect("writing into String cannot fail");
        } else {
            buffer.push_str(&explanation);
        }
        if index + 1 < items.len() {
            buffer.push_str(", ");
        }
    }
    buffer
}
