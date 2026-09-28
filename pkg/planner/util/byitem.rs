// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// ORDER BY / GROUP BY 单项包装（ByItems）。
//
// 每个 ByItems 持有一个排序/分组表达式及其升/降序标志，并实现 Hash64/Equals
// 以便计划缓存与 Memo 指纹比较。

use std::any::Any;

use cascades_base::{Equals, Hash64, Hasher};
use expression::{Expression as _, StringerWithCtx as _};

/// ByItems wraps one expression in an ORDER BY/GROUP BY item.
/// 包装 ORDER BY / GROUP BY 中的单个表达式项。
#[derive(Clone)]
pub struct ByItems {
    /// 排序或分组表达式。
    pub Expr: expression::ExprBox,
    /// 是否降序（true = DESC）。
    pub Desc: bool,
}

impl Hash64 for ByItems {
    fn Hash64(&self, hasher: &mut dyn Hasher) {
        self.Expr.Hash64(hasher);
        hasher.HashBool(self.Desc);
    }
}

impl Equals for ByItems {
    fn Equals(&self, other: &dyn Any) -> bool {
        let Some(other) = other.downcast_ref::<ByItems>() else {
            return false;
        };
        self.Desc == other.Desc && self.Expr.Equals(other.Expr.as_any())
    }
}

impl ByItems {
    /// 带上下文的字符串化；降序时在表达式后追加 ` true`。
    pub fn StringWithCtx(
        &self,
        context: Option<&dyn expression::exprctx::ParamValues>,
        redact: &str,
    ) -> String {
        let text = self.Expr.StringWithCtx(context, redact);
        if self.Desc {
            format!("{text} true")
        } else {
            text
        }
    }

    /// 深拷贝本项。
    pub fn Clone(&self) -> ByItems {
        self.clone()
    }

    /// 在给定求值上下文下比较表达式与升降序是否相等。
    pub fn Equal(&self, context: &dyn expression::exprctx::EvalContext, other: &ByItems) -> bool {
        self.Desc == other.Desc && self.Expr.Equal(context, other.Expr.as_ref())
    }

    /// 估算本项内存占用（布尔标志 + 表达式）。
    pub fn MemoryUsage(&self) -> i64 {
        size::SizeOfBool + self.Expr.MemoryUsage()
    }
}

/// 将 ByItems 切片格式化为 `[item1 item2 ...]` 形式的调试字符串。
pub fn StringifyByItemsWithCtx(
    context: &dyn expression::exprctx::EvalContext,
    items: &[ByItems],
) -> String {
    let body = items
        .iter()
        .map(|item| item.StringWithCtx(Some(context), expression::errors::RedactLogDisable))
        .collect::<Vec<_>>()
        .join(" ");
    format!("[{body}]")
}
