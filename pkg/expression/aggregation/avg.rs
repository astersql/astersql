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

// AVG 聚合函数实现：按阶段累加求和与计数，最终输出平均值。
//
// 分布式聚合中，Partial1/Complete 阶段像 SUM 一样累加原始值；
// Partial2/Final 阶段接收局部 `(count, sum)` 再合并。最终结果为 `sum / count`。

use crate::*;
use expression::Expression as _;
use std::sync::Arc;

/// AVG 运行时聚合器，内嵌通用 `aggFunction` 描述与求和方法。
pub struct avgFunction {
    /// 共享的聚合描述符与辅助更新逻辑。
    pub aggFunction: aggFunction,
}

impl avgFunction {
    /// Partial2/Final：合并上游传来的局部 count（Args[0]）与 sum（Args[1]）。
    fn updateAvg(
        &self,
        type_context: types::Context,
        ctx: &mut AggEvaluateContext,
        row: chunk::Row,
    ) -> Result<(), Error> {
        let value = self.aggFunction.AggFuncDesc.Args[1].Eval(ctx.Ctx.as_ref(), row.clone())?;
        if value.IsNull() {
            return Ok(());
        }
        // 按 MySQL SUM/AVG 规则把局部 sum 累加进 ctx.Value。
        ctx.Value = calculateSum(type_context, ctx.Value.clone(), value)?;
        ctx.Count += self.aggFunction.AggFuncDesc.Args[0]
            .Eval(ctx.Ctx.as_ref(), row)?
            .GetInt64();
        Ok(())
    }
}

impl Aggregation for avgFunction {
    fn Update(
        &mut self,
        ctx: &mut AggEvaluateContext,
        sc: &stmtctx::StatementContext,
        row: chunk::Row,
    ) -> Result<(), Error> {
        // 按聚合模式选择：一阶段直接求局部和，二阶段合并 (count,sum)。
        match self.aggFunction.AggFuncDesc.Mode {
            Partial1Mode | CompleteMode => self.aggFunction.updateSum(sc.TypeCtx(), ctx, row),
            Partial2Mode | FinalMode => self.updateAvg(sc.TypeCtx(), ctx, row),
            DedupMode => panic!("DedupMode is not supported now."),
        }
    }
    /// 用累计 sum 除以 count；Decimal 按返回类型精度四舍五入。
    fn GetResult(&self, ctx: &AggEvaluateContext) -> types::Datum {
        let mut result = types::Datum::default();
        match ctx.Value.Kind() {
            types::KindFloat64 => result.SetFloat64(ctx.Value.GetFloat64() / ctx.Count as f64),
            types::KindMysqlDecimal => {
                let mut quotient = types::MyDecimal::default();
                // Decimal 除法后按 RetTp 小数位（或 MaxDecimalScale）做半入舍入。
                let _ = types::DecimalDiv(
                    &ctx.Value.GetMysqlDecimal(),
                    &types::NewDecFromInt(ctx.Count),
                    &mut quotient,
                    ctx.Ctx.GetDivPrecisionIncrement() as isize,
                );
                let fraction = self.aggFunction.AggFuncDesc.RetTp.as_ref().map_or(
                    mysql::MaxDecimalScale as isize,
                    |tp| {
                        let scale = tp.GetDecimal();
                        if scale == -1 {
                            mysql::MaxDecimalScale as isize
                        } else {
                            scale
                        }
                    },
                );
                let mut rounded = types::MyDecimal::default();
                let _ = quotient.Round(
                    &mut rounded,
                    fraction.min(mysql::MaxDecimalScale as isize),
                    types::ModeHalfUp,
                );
                result.SetMysqlDecimal(rounded);
            }
            _ => {}
        }
        result
    }
    /// 局部结果为 `(count, sum)`，供下一阶段 AVG 合并。
    fn GetPartialResult(&self, ctx: &AggEvaluateContext) -> Vec<types::Datum> {
        vec![types::NewIntDatum(ctx.Count), ctx.Value.clone()]
    }
    fn CreateContext(&self, ctx: Arc<dyn expression::EvalContext>) -> AggEvaluateContext {
        self.aggFunction.CreateContext(ctx)
    }
    fn ResetContext(
        &mut self,
        ctx: Arc<dyn expression::EvalContext>,
        eval: &mut AggEvaluateContext,
    ) {
        self.aggFunction.ResetContext(ctx, eval);
    }
}
