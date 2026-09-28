// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 表达式求值器套件：批量求值列投影与标量/向量化表达式。
//
// `EvaluatorSuite` 将纯列引用走列交换（ColumnSwapHelper）快路径，其余表达式交由
// `defaultEvaluator` 按行或按向量求值。可选求值属性（OptionalEvalProp）从标量函数树递归收集。

use std::collections::HashMap;

use crate::*;

/// 默认求值器：对非列表达式按输出下标写入 chunk，可向量化时优先整列求值。
struct defaultEvaluator {
    output_idxes: Vec<usize>,
    exprs: Vec<ExprBox>,
    vectorizable: bool,
}

impl defaultEvaluator {
    /// 在 input 上求值 exprs，结果写入 output 对应列。
    fn run(
        &self,
        ctx: &dyn EvalContext,
        vec_enabled: bool,
        input: &chunk::Chunk,
        output: &mut chunk::Chunk,
    ) -> Result<(), Error> {
        if self.vectorizable {
            // 向量化路径：函数声明 Vectorized 且会话开启时走 evalOneVec，否则按行 evalOneCell。
            for (expression, output_index) in self.exprs.iter().zip(&self.output_idxes) {
                if vec_enabled && expression.Vectorized() {
                    crate::chunk_executor_kernel::evalOneVec(
                        ctx,
                        expression.as_ref(),
                        input,
                        output,
                        *output_index,
                    )?;
                } else {
                    for row_index in 0..input.NumRows() {
                        crate::chunk_executor_kernel::evalOneCell(
                            ctx,
                            expression.as_ref(),
                            input.GetRow(row_index),
                            output,
                            *output_index,
                        )?;
                    }
                }
            }
            return Ok(());
        }

        // 不可向量化时退回逐行标量求值。
        for row_index in 0..input.NumRows() {
            let row = input.GetRow(row_index);
            for (expression, output_index) in self.exprs.iter().zip(&self.output_idxes) {
                crate::chunk_executor_kernel::evalOneCell(
                    ctx,
                    expression.as_ref(),
                    row.clone(),
                    output,
                    *output_index,
                )?;
            }
        }
        Ok(())
    }

    /// 汇总本求值器全部表达式所需的可选求值属性位集。
    fn RequiredOptionalEvalProps(&self) -> exprctx::OptionalEvalPropKeySet {
        self.exprs.iter().fold(
            exprctx::OptionalEvalPropKeySet::default(),
            |properties, expression| {
                exprctx::OptionalEvalPropKeySet(
                    properties.0 | GetOptionalEvalPropsForExpr(expression.as_ref()).0,
                )
            },
        )
    }
}

/// 递归收集标量函数树声明的可选求值属性；非 ScalarFunction 返回空集。
pub fn GetOptionalEvalPropsForExpr(expr: &dyn Expression) -> exprctx::OptionalEvalPropKeySet {
    let Some(function) = expr.as_any().downcast_ref::<ScalarFunction>() else {
        return exprctx::OptionalEvalPropKeySet::default();
    };
    function.GetArgs().iter().fold(
        function.Function.RequiredOptionalEvalProps(),
        |properties, argument| {
            exprctx::OptionalEvalPropKeySet(
                properties.0 | GetOptionalEvalPropsForExpr(argument.as_ref()).0,
            )
        },
    )
}

/// 求值套件：列交换快路径 + 默认表达式求值器。
pub struct EvaluatorSuite {
    pub ColumnSwapHelper: Option<chunk::ColumnSwapHelper>,
    default_evaluator: Option<defaultEvaluator>,
}

/// 构造求值套件；`avoid_column_evaluator` 为真时禁止列交换优化，强制走表达式求值。
pub fn NewEvaluatorSuite(exprs: Vec<ExprBox>, avoid_column_evaluator: bool) -> EvaluatorSuite {
    let capacity = exprs.len();
    let mut column_mapping: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut evaluator = defaultEvaluator {
        output_idxes: Vec::with_capacity(capacity),
        exprs: Vec::with_capacity(capacity),
        vectorizable: false,
    };

    for (output_index, expression) in exprs.into_iter().enumerate() {
        // 纯列引用记入 column_mapping，稍后由 ColumnSwapHelper 直接交换列引用。
        if !avoid_column_evaluator
            && let Some(column) = expression.as_any().downcast_ref::<Column>()
        {
            column_mapping
                .entry(
                    usize::try_from(column.Index)
                        .expect("column evaluator requires a resolved non-negative index"),
                )
                .or_default()
                .push(output_index);
            continue;
        }
        evaluator.output_idxes.push(output_index);
        evaluator.exprs.push(expression);
    }

    // 无待求值表达式时省略 defaultEvaluator；否则探测整组是否可向量化。
    let default_evaluator = if evaluator.exprs.is_empty() {
        None
    } else {
        evaluator.vectorizable = crate::chunk_executor_kernel::Vectorizable(&evaluator.exprs);
        Some(evaluator)
    };
    EvaluatorSuite {
        ColumnSwapHelper: (!column_mapping.is_empty())
            .then(|| chunk::ColumnSwapHelper::New(column_mapping)),
        default_evaluator,
    }
}

impl EvaluatorSuite {
    /// 无默认求值器或全部可向量化时返回 true。
    pub fn Vectorizable(&self) -> bool {
        self.default_evaluator
            .as_ref()
            .is_none_or(|evaluator| evaluator.vectorizable)
    }

    /// 先跑默认求值器，再执行列交换；input 可能被交换路径掏空列数据。
    pub fn Run(
        &self,
        ctx: &dyn EvalContext,
        vec_enabled: bool,
        input: &mut chunk::Chunk,
        output: &mut chunk::Chunk,
    ) -> Result<(), Error> {
        if let Some(evaluator) = &self.default_evaluator {
            evaluator.run(ctx, vec_enabled, input, output)?;
        }
        if let Some(helper) = &self.ColumnSwapHelper {
            helper
                .SwapColumns(input, output)
                .map_err(|error| errors::New(error.to_string()))?;
        }
        Ok(())
    }

    /// 透传默认求值器所需的可选属性；无求值器时为空集。
    pub fn RequiredOptionalEvalProps(&self) -> exprctx::OptionalEvalPropKeySet {
        self.default_evaluator
            .as_ref()
            .map_or_else(exprctx::OptionalEvalPropKeySet::default, |evaluator| {
                evaluator.RequiredOptionalEvalProps()
            })
    }
}
