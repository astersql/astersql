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

// 显式 COLLATE 子句应用到表达式。
//
// 对应规划器表达式改写中的 `ast.SetCollationExpr`：列与 JSON 经 CAST 包装以免就地修改共享类型；
// 并设置显式 coercibility（强制优先级），保证后续排序规则推导尊重用户指定。

use crate::*;

/// Applies a SQL `COLLATE` clause to an expression.
///
/// This is the reusable expression-layer counterpart of the
/// `ast.SetCollationExpr` branch in `planner/core/expression_rewriter.go`.
/// Columns are wrapped in a cast so their shared `FieldType` is not mutated;
/// JSON values are first converted to LONGTEXT, exactly as in the Go rewriter.
///
/// 将 SQL `COLLATE` 应用到表达式：新排序规则模式下校验字符集匹配；列/JSON 走 CAST，
/// 其它类型就地改 collate；最后标记为显式 coercibility。
pub fn SetCollationToExpression(
    ctx: &dyn BuildContext,
    mut argument: ExprBox,
    collation: &str,
    use_new_collation: bool,
) -> Result<ExprBox, Error> {
    let eval_ctx = ctx.GetEvalCtx();
    let argument_type = argument.GetType(eval_ctx);

    if use_new_collation {
        // 新排序规则框架：按名称查表，并拒绝与参数字符集不一致的 COLLATE。
        let collation_info = collate::GetCollationByName(collation)?;
        let charset_name = if argument_type.GetType() == mysql::TypeJSON {
            charset::CharsetUTF8MB4.to_owned()
        } else {
            argument_type.GetCharset().to_owned()
        };
        if !charset_name.is_empty() && collation_info.CharsetName != charset_name {
            return Err(charset::ErrCollationCharsetMismatch
                .GenWithStackByArgs(&[
                    charset::errors::ErrorArg::String(collation_info.Name),
                    charset::errors::ErrorArg::String(charset_name),
                ])
                .into());
        }
    }

    let is_column = argument.as_any().is::<Column>();
    let argument_mysql_type = argument_type.GetType();
    if is_column || argument_mysql_type == mysql::TypeJSON {
        if argument_mysql_type == mysql::TypeEnum || argument_mysql_type == mysql::TypeSet {
            return Err(errors::New(
                "This version of AsterSQL doesn't yet support 'use collate clause for enum or set'",
            ));
        }

        // JSON 先转为 utf8mb4 LONGTEXT，再设目标 collate；列复制类型后 CAST，避免改共享 FieldType。
        let mut target_type = if argument_mysql_type == mysql::TypeJSON {
            let mut target = *types::NewFieldType(mysql::TypeLongBlob);
            target.SetCharset(charset::CharsetUTF8MB4.to_owned());
            target
        } else {
            argument_type.clone()
        };
        target_type.SetCollate(collation.to_owned());
        argument = BuildCastFunction(ctx, &argument, &target_type);
    } else {
        argument.GetTypeMut().SetCollate(collation.to_owned());
    }

    argument.SetCoercibility(CoercibilityExplicit);
    let (charset_name, collation_name) = {
        let result_type = argument.GetType(eval_ctx);
        (
            result_type.GetCharset().to_owned(),
            result_type.GetCollate().to_owned(),
        )
    };
    argument.SetCharsetAndCollation(charset_name, collation_name);
    Ok(argument)
}
