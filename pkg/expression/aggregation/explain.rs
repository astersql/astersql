// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 将聚合函数描述格式化为 EXPLAIN 文本，可选展示聚合模式与 DISTINCT。
//
// GROUP_CONCAT 还会输出 ORDER BY 子句与 separator 参数。

use crate::*;

use std::sync::atomic::{AtomicBool, Ordering};

use expression::Expression as _;

/// 测试用开关：为 true 时在 EXPLAIN 中打印聚合 Mode（如 Partial1/Final）。
static SHOW_AGG_MODE: AtomicBool = AtomicBool::new(false);

/// Rust equivalent of Go's `show-agg-mode` failpoint switch.
/// 测试专用：控制 EXPLAIN 是否展示聚合模式。
pub fn SetExplainAggModeForTest(enabled: bool) {
    SHOW_AGG_MODE.store(enabled, Ordering::SeqCst);
}

/// 生成聚合函数的 EXPLAIN 字符串；`normalized` 为 true 时用归一化参数信息。
pub fn ExplainAggFunc(
    ctx: &dyn expression::EvalContext,
    aggregate: &AggFuncDesc,
    normalized: bool,
) -> String {
    let mut result = if SHOW_AGG_MODE.load(Ordering::SeqCst) {
        format!("{}({},", aggregate.Name, aggregate.Mode.ToString())
    } else {
        format!("{}(", aggregate.Name)
    };
    if aggregate.HasDistinct {
        result.push_str("distinct ");
    }
    for (index, argument) in aggregate.Args.iter().enumerate() {
        // GROUP_CONCAT 最后一个参数前插入 order by / separator 文案。
        if aggregate.Name == ast::AggFuncGroupConcat && index + 1 == aggregate.Args.len() {
            if !aggregate.OrderByItems.is_empty() {
                result.push_str(" order by ");
                for (order_index, item) in aggregate.OrderByItems.iter().enumerate() {
                    if order_index != 0 {
                        result.push_str(", ");
                    }
                    let text = if normalized {
                        item.Expr.ExplainNormalizedInfo()
                    } else {
                        item.Expr.ExplainInfo(ctx)
                    };
                    result.push_str(&text);
                    if item.Desc {
                        result.push_str(" desc");
                    }
                }
            }
            result.push_str(" separator ");
        } else if index != 0 {
            result.push_str(", ");
        }
        if normalized {
            result.push_str(&argument.ExplainNormalizedInfo());
        } else {
            result.push_str(&argument.ExplainInfo(ctx));
        }
    }
    result.push(')');
    result
}
