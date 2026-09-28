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

// COUNT 聚合：统计非空行（或 DISTINCT 组合）；二阶段直接累加局部 count。

use crate::*;
use expression::Expression as _;
use std::sync::Arc;

/// COUNT 运行时聚合器。
pub struct countFunction {
    /// 共享聚合描述符。
    pub aggFunction: aggFunction,
}

impl Aggregation for countFunction {
    fn Update(
        &mut self,
        ctx: &mut AggEvaluateContext,
        _sc: &stmtctx::StatementContext,
        row: chunk::Row,
    ) -> Result<(), Error> {
        let mut values = Vec::with_capacity(self.aggFunction.AggFuncDesc.Args.len());
        for argument in &self.aggFunction.AggFuncDesc.Args {
            let value = argument.Eval(ctx.Ctx.as_ref(), row.clone())?;
            // 任一参数为 NULL 则本行不计入 COUNT。
            if value.IsNull() {
                return Ok(());
            }
            // Final/Partial2：参数已是局部计数值，直接相加。
            if matches!(self.aggFunction.AggFuncDesc.Mode, FinalMode | Partial2Mode) {
                ctx.Count += value.GetInt64();
            }
            if self.aggFunction.AggFuncDesc.HasDistinct {
                values.push(value);
            }
        }
        if matches!(self.aggFunction.AggFuncDesc.Mode, FinalMode | Partial2Mode) {
            return Ok(());
        }
        // DISTINCT：重复组合跳过；否则计数 +1。
        if let Some(checker) = &mut ctx.DistinctChecker
            && !checker.Check(values)?
        {
            return Ok(());
        }
        if matches!(
            self.aggFunction.AggFuncDesc.Mode,
            CompleteMode | Partial1Mode
        ) {
            ctx.Count += 1;
        }
        Ok(())
    }
    fn GetResult(&self, ctx: &AggEvaluateContext) -> types::Datum {
        types::NewIntDatum(ctx.Count)
    }
    fn GetPartialResult(&self, ctx: &AggEvaluateContext) -> Vec<types::Datum> {
        vec![self.GetResult(ctx)]
    }
    fn CreateContext(&self, ctx: Arc<dyn expression::EvalContext>) -> AggEvaluateContext {
        self.aggFunction.CreateContext(ctx)
    }
    fn ResetContext(
        &mut self,
        ctx: Arc<dyn expression::EvalContext>,
        eval: &mut AggEvaluateContext,
    ) {
        self.aggFunction.ResetContext(ctx, eval)
    }
}
