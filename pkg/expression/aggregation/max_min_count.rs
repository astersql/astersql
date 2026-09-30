// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use crate::*;
use expression::Expression as _;
use std::sync::Arc;

/// Counts rows tied for the maximum or minimum non-NULL value.
pub struct maxMinCountFunction {
    pub aggFunction: aggFunction,
    pub isMax: bool,
    pub ctor: Box<dyn collate::Collator>,
}

impl Aggregation for maxMinCountFunction {
    fn Update(
        &mut self,
        eval_ctx: &mut AggEvaluateContext,
        sc: &stmtctx::StatementContext,
        row: chunk::Row,
    ) -> Result<(), Error> {
        let args = &self.aggFunction.AggFuncDesc.Args;
        let (value, count) = if args.len() > 1 {
            let (count, is_null) = args[0].EvalInt(eval_ctx.Ctx.as_ref(), row.clone())?;
            if is_null || count == 0 {
                return Ok(());
            }
            let value = args[1].Eval(eval_ctx.Ctx.as_ref(), row)?;
            if value.IsNull() {
                return Ok(());
            }
            (value, count)
        } else {
            let value = args[0].Eval(eval_ctx.Ctx.as_ref(), row)?;
            if value.IsNull() {
                return Ok(());
            }
            (value, 1)
        };

        if eval_ctx.Value.IsNull() {
            eval_ctx.Value = value;
            eval_ctx.Count = count;
            return Ok(());
        }
        let comparison = eval_ctx
            .Value
            .Compare(sc.TypeCtx(), &value, self.ctor.as_ref())?;
        if (self.isMax && comparison < 0) || (!self.isMax && comparison > 0) {
            eval_ctx.Value = value;
            eval_ctx.Count = count;
        } else if comparison == 0 {
            eval_ctx.Count += count;
        }
        Ok(())
    }

    fn GetResult(&self, eval_ctx: &AggEvaluateContext) -> types::Datum {
        types::NewIntDatum(eval_ctx.Count)
    }

    fn GetPartialResult(&self, eval_ctx: &AggEvaluateContext) -> Vec<types::Datum> {
        vec![self.GetResult(eval_ctx), eval_ctx.Value.clone()]
    }

    fn CreateContext(&self, ctx: Arc<dyn expression::EvalContext>) -> AggEvaluateContext {
        self.aggFunction.CreateContext(ctx)
    }

    fn ResetContext(
        &mut self,
        ctx: Arc<dyn expression::EvalContext>,
        eval_ctx: &mut AggEvaluateContext,
    ) {
        self.aggFunction.ResetContext(ctx, eval_ctx);
        eval_ctx.Count = 0;
    }
}
