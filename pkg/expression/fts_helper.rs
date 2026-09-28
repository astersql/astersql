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

// 全文检索（FTS，Full-Text Search）表达式辅助函数。
//
// 在表达式树中识别 `FTSMatchWord`，并尝试将其解释为
// “常量查询词 + 列引用”的标准形状，供优化器/执行器做 FTS 下推或改写。

use crate::*;

// FTSInfo 对应 Go 的便捷解释结果：保存常量查询词及参与全文检索的列。
/// 全文检索便捷解释结果：常量查询词与参与匹配的列。
pub struct FTSInfo<'a> {
    /// 常量查询文本（来自第一个参数 Constant）。
    pub Query: String,
    // Go 保存 *Column；用借用表达该结果不取得表达式节点所有权。
    /// 参与全文匹配的列引用（借用，不取得表达式节点所有权）。
    pub Column: &'a Column,
}

// ContainsFullTextSearchFn 递归检查表达式树中是否存在可能的 FullTextSearch 函数。
/// 递归检查表达式树中是否出现 `FTSMatchWord` 标量函数。
pub fn ContainsFullTextSearchFn(expr: &dyn Expression) -> bool {
    let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() else {
        return false;
    };
    if function.FuncName.L == ast::FTSMatchWord {
        return true;
    }

    // 向下递归子表达式参数。
    function
        .GetArgs()
        .iter()
        .any(|argument| ContainsFullTextSearchFn(argument.as_ref()))
}

// InterpretFullTextSearchExpr 尝试把表达式解释为“常量查询词 + 列”的标准全文检索形状。
// 任一结构条件不满足都返回 None，对应 Go 的 nil，不产生部分结果或副作用。
/// 尝试将表达式解释为标准 FTS 形状；条件不满足返回 `None`（对应 Go 的 nil）。
pub fn InterpretFullTextSearchExpr(expr: &dyn Expression) -> Option<FTSInfo<'_>> {
    let function = expr.as_any().downcast_ref::<ScalarFunction>()?;
    if function.FuncName.L != ast::FTSMatchWord {
        return None;
    }

    let args = function.GetArgs();
    if args.len() != 2 {
        return None;
    }

    // 参数位置严格沿用 Go 约定：第一个必须是常量查询文本，第二个必须是列引用。
    let query = args[0].as_any().downcast_ref::<Constant>()?;
    let column = args[1].as_any().downcast_ref::<Column>()?;
    Some(FTSInfo {
        Query: query.Value.GetString(),
        Column: column,
    })
}
