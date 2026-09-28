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

// FIRST_ROW 聚合：保留组内遇到的第一个参数值（含 NULL），之后不再更新。
//
// 优化器常用其表达“按分组唯一键取列”等语义，也用于部分下推场景。

use crate::*;
use expression::Expression as _;
use std::sync::Arc;

/// FIRST_ROW 运行时聚合器。
pub struct firstRowFunction {
    /// 共享聚合描述符。
    pub aggFunction: aggFunction,
}
impl Aggregation for firstRowFunction {
    fn Update(
        &mut self,
        ctx: &mut AggEvaluateContext,
        _sc: &stmtctx::StatementContext,
        row: chunk::Row,
    ) -> Result<(), Error> {
        // 已取到首行则短路，保证“只认第一次”。
        if ctx.GotFirstRow {
            return Ok(());
        }
        if self.aggFunction.AggFuncDesc.Args.len() != 1 {
            return Err(expression::errors::New(
                "Wrong number of args for AggFuncFirstRow",
            ));
        }
        ctx.Value = self.aggFunction.AggFuncDesc.Args[0].Eval(ctx.Ctx.as_ref(), row)?;
        ctx.GotFirstRow = true;
        Ok(())
    }
    fn GetResult(&self, ctx: &AggEvaluateContext) -> types::Datum {
        ctx.Value.clone()
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
        // Go's FIRST_ROW override deliberately preserves Value and the other
        // aggregate state here; the next Update replaces Value after clearing
        // this guard.
        eval.Ctx = ctx;
        eval.GotFirstRow = false;
    }
}
