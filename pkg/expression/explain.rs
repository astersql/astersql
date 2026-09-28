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

// 表达式 EXPLAIN 文本格式化。
//
// 为 ScalarFunction、Column、Constant 生成可读/归一化说明字符串，并提供表达式列表、
// 排序列表与列列表的展示辅助。支持 redact（脱敏）模式以隐藏敏感常量。

use crate::*;

impl ScalarFunction {
    /// 生成带求值上下文的函数说明，例如 `cast(col, bigint)`。
    pub fn ExplainInfo(&self, ctx: &dyn EvalContext) -> String {
        self.explainInfo(Some(ctx), false)
    }

    /// `normalized` 为真时用归一化参数文本且不依赖 ctx；IN 对物理表 ID=-1 特例输出 dual。
    fn explainInfo(&self, ctx: Option<&dyn EvalContext>, normalized: bool) -> String {
        assert!(normalized || ctx.is_some());
        let mut buffer = format!("{}(", self.FuncName.L);

        // ExtraPhysTblIDName 且常量 -1：分区裁剪后落到 dual 表的展示特例。
        if self.FuncName.L == ast::In {
            let args = self.GetArgs();
            if args.len() == 2
                && args[0]
                    .ExplainNormalizedInfo()
                    .ends_with(&model::ExtraPhysTblIDName.L)
                && args[1]
                    .as_any()
                    .downcast_ref::<Constant>()
                    .is_some_and(|constant| constant.Value.GetInt64() == -1)
            {
                buffer.push_str(&args[0].ExplainNormalizedInfo());
                buffer.push_str(", dual)");
                return buffer;
            }
        }

        match self.FuncName.L.as_str() {
            // CAST 额外附加返回类型字符串。
            ast::Cast => {
                for arg in self.GetArgs() {
                    if normalized {
                        buffer.push_str(&arg.ExplainNormalizedInfo());
                    } else {
                        buffer.push_str(&arg.ExplainInfo(ctx.expect("explain context")));
                    }
                    buffer.push_str(", ");
                    buffer.push_str(&self.RetType.as_ref().unwrap().String());
                }
            }
            _ => {
                for (index, arg) in self.GetArgs().iter().enumerate() {
                    if normalized {
                        buffer.push_str(&arg.ExplainNormalizedInfo());
                    } else {
                        buffer.push_str(&arg.ExplainInfo(ctx.expect("explain context")));
                    }
                    if index + 1 < self.GetArgs().len() {
                        buffer.push_str(", ");
                    }
                }
            }
        }
        buffer.push(')');
        buffer
    }

    /// 归一化说明：常量等敏感细节折叠，便于计划缓存键比对。
    pub fn ExplainNormalizedInfo(&self) -> String {
        self.explainInfo(None, true)
    }

    /// IN 列表归一化时参数折叠为 `...`，避免长列表淹没计划文本。
    pub fn ExplainNormalizedInfo4InList(&self) -> String {
        let mut buffer = format!("{}(", self.FuncName.L);
        match self.FuncName.L.as_str() {
            ast::Cast => {
                for arg in self.GetArgs() {
                    buffer.push_str(&arg.ExplainNormalizedInfo4InList());
                    buffer.push_str(", ");
                    buffer.push_str(&self.RetType.as_ref().unwrap().String());
                }
            }
            ast::In => buffer.push_str("..."),
            _ => {
                for (index, arg) in self.GetArgs().iter().enumerate() {
                    buffer.push_str(&arg.ExplainNormalizedInfo4InList());
                    if index + 1 < self.GetArgs().len() {
                        buffer.push_str(", ");
                    }
                }
            }
        }
        buffer.push(')');
        buffer
    }
}

impl Column {
    /// 列说明；归一化时用原名或 `?`，否则按参数上下文格式化并可选去掉列号。
    pub fn ColumnExplainInfo(&self, ctx: &dyn ParamValues, normalized: bool) -> String {
        if normalized {
            return self.ColumnExplainInfoNormalized();
        }
        self.StringWithCtxForExplain(
            ctx,
            errors::RedactLogDisable,
            shouldRemoveColumnNumbers(ctx),
        )
    }

    /// 有 OrigName 则用之，否则占位 `?`。
    pub fn ColumnExplainInfoNormalized(&self) -> String {
        if self.OrigName.is_empty() {
            "?".to_owned()
        } else {
            self.OrigName.clone()
        }
    }

    /// 非归一化列说明，委托 ColumnExplainInfo。
    pub fn ExplainInfo(&self, ctx: &dyn EvalContext) -> String {
        self.ColumnExplainInfo(ctx, false)
    }

    /// 归一化列说明。
    pub fn ExplainNormalizedInfo(&self) -> String {
        self.ColumnExplainInfoNormalized()
    }

    /// IN 列表场景下的归一化列说明（与普通归一化相同）。
    pub fn ExplainNormalizedInfo4InList(&self) -> String {
        self.ColumnExplainInfoNormalized()
    }
}

impl Constant {
    /// 按 redact 模式输出常量；子查询引用带 ScalarQueryCol#ID 前缀。
    pub fn ExplainInfo(&self, ctx: &dyn EvalContext) -> String {
        let redact = ctx.GetTiDBRedactLog();
        if redact == errors::RedactLogEnable {
            return if self.SubqueryRefID > 0 {
                format!("ScalarQueryCol#{}(?)", self.SubqueryRefID)
            } else {
                "?".to_owned()
            };
        }

        let datum = match self.Eval(ctx, chunk::Row::default()) {
            Ok(datum) => datum,
            Err(_) => return "not recognized const value".to_owned(),
        };
        let mut value = self.formatDatum(datum);
        if redact == errors::RedactLogMarker {
            value = format!("‹{}›", value);
        }
        if self.SubqueryRefID > 0 {
            format!("ScalarQueryCol#{}({})", self.SubqueryRefID, value)
        } else {
            value
        }
    }

    /// 归一化常量一律为 `?`。
    pub fn ExplainNormalizedInfo(&self) -> String {
        "?".to_owned()
    }

    /// IN 列表归一化常量一律为 `?`。
    pub fn ExplainNormalizedInfo4InList(&self) -> String {
        "?".to_owned()
    }

    /// 将 Datum 格式化为 EXPLAIN 文本；字符串类加引号。
    fn formatDatum(&self, datum: types::Datum) -> String {
        match datum.Kind() {
            types::KindNull => "NULL".to_owned(),
            types::KindString
            | types::KindBytes
            | types::KindMysqlEnum
            | types::KindMysqlSet
            | types::KindMysqlJSON
            | types::KindBinaryLiteral
            | types::KindMysqlBit => format!("\"{}\"", datum.TruncatedStringify()),
            _ => datum.TruncatedStringify(),
        }
    }
}

/// 按 redact 模式把值写入 builder：Marker 加书名号、Enable 用 `?`、否则原文。
fn writeRedact(builder: &mut String, value: &str, mode: &str) {
    match mode {
        errors::RedactLogMarker => {
            builder.push('‹');
            builder.push_str(value);
            builder.push('›');
        }
        errors::RedactLogEnable => builder.push('?'),
        _ => builder.push_str(value),
    }
}

/// 格式化投影表达式列表：列别名变化时用 `expr->schema`，常量/其它表达式亦同。
pub fn ExplainExpressionList(
    ctx: &dyn EvalContext,
    exprs: &[ExprBox],
    schema: &Schema,
    redact_mode: &str,
) -> String {
    ExplainExpressionListWithColumnNumbers(
        ctx,
        exprs,
        schema,
        redact_mode,
        shouldRemoveColumnNumbers(ctx),
    )
}

/// 生成表达式列表，并允许计划层显式传入 plan_tree 的列编号隐藏策略。
pub fn ExplainExpressionListWithColumnNumbers(
    ctx: &dyn EvalContext,
    exprs: &[ExprBox],
    schema: &Schema,
    redact_mode: &str,
    remove_column_numbers: bool,
) -> String {
    let mut builder = String::new();
    for (index, expr) in exprs.iter().enumerate() {
        let schema_column =
            schema.Columns[index].StringWithCtxForExplain(ctx, redact_mode, remove_column_numbers);
        let column = expr.as_any().downcast_ref::<Column>().or_else(|| {
            expr.as_any()
                .downcast_ref::<CorrelatedColumn>()
                .map(|c| &c.column)
        });
        if let Some(column) = column {
            let value = column.StringWithCtxForExplain(ctx, redact_mode, remove_column_numbers);
            builder.push_str(&value);
            // 与 Go 一致：展示文本不同即追加别名箭头，包括不同的自动生成列编号。
            if value != schema_column {
                builder.push_str("->");
                builder.push_str(&schema_column);
            }
        } else if let Some(constant) = expr.as_any().downcast_ref::<Constant>() {
            let value = constant.StringWithCtx(ctx, errors::RedactLogDisable);
            writeRedact(&mut builder, &value, redact_mode);
            builder.push_str("->");
            builder.push_str(&schema_column);
        } else {
            builder.push_str(&expr.StringWithCtx(Some(ctx), redact_mode));
            builder.push_str("->");
            builder.push_str(&schema_column);
        }
        if index + 1 < exprs.len() {
            builder.push_str(", ");
        }
    }
    builder
}

/// 对表达式说明排序后拼接，便于与顺序无关的计划对比。
pub fn SortedExplainExpressionList(ctx: &dyn EvalContext, exprs: &[ExprBox]) -> Vec<u8> {
    sortedExplainExpressionList(Some(ctx), exprs, false, false)
}

/// 排序说明且 IN 列表折叠；不依赖求值上下文。
pub fn SortedExplainExpressionListIgnoreInlist(exprs: &[ExprBox]) -> Vec<u8> {
    sortedExplainExpressionList(None, exprs, false, true)
}

/// 按模式生成各表达式说明、排序并以逗号连接为字节串。
fn sortedExplainExpressionList(
    ctx: Option<&dyn EvalContext>,
    exprs: &[ExprBox],
    normalized: bool,
    ignore_in_list: bool,
) -> Vec<u8> {
    assert!(ignore_in_list || normalized || ctx.is_some());
    let mut infos = exprs
        .iter()
        .map(|expr| {
            if ignore_in_list {
                expr.ExplainNormalizedInfo4InList()
            } else if normalized {
                expr.ExplainNormalizedInfo()
            } else {
                expr.ExplainInfo(ctx.expect("explain context"))
            }
        })
        .collect::<Vec<_>>();
    infos.sort();
    infos.join(", ").into_bytes()
}

/// 归一化表达式列表的排序说明。
pub fn SortedExplainNormalizedExpressionList(exprs: &[ExprBox]) -> Vec<u8> {
    sortedExplainExpressionList(None, exprs, true, false)
}

/// 标量函数列表先 Clone 为表达式再做归一化排序说明。
pub fn SortedExplainNormalizedScalarFuncList(exprs: &[ScalarFunction]) -> Vec<u8> {
    let expressions = exprs.iter().map(ScalarFunction::Clone).collect::<Vec<_>>();
    sortedExplainExpressionList(None, &expressions, true, false)
}

/// 列说明列表，逗号分隔。
pub fn ExplainColumnList(ctx: &dyn EvalContext, columns: &[Column]) -> Vec<u8> {
    columns
        .iter()
        .map(|column| column.ExplainInfo(ctx))
        .collect::<Vec<_>>()
        .join(", ")
        .into_bytes()
}
