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

// MAX/MIN 聚合：在非空值中按 Collator 比较，保留更大或更小者。

use crate::*;
use expression::Expression as _;
use std::sync::Arc;

/// MAX/MIN 共用实现；`isMax` 区分取最大或最小。
pub struct maxMinFunction {
    /// 共享聚合描述符。
    pub aggFunction: aggFunction,
    /// true 为 MAX，false 为 MIN。
    pub isMax: bool,
    /// 字符串比较用的排序规则（Collation）实现。
    pub ctor: Box<dyn collate::Collator>,
}
impl Aggregation for maxMinFunction {
    fn Update(
        &mut self,
        ctx: &mut AggEvaluateContext,
        sc: &stmtctx::StatementContext,
        row: chunk::Row,
    ) -> Result<(), Error> {
        let value = self.aggFunction.AggFuncDesc.Args[0].Eval(ctx.Ctx.as_ref(), row)?;
        if value.IsNull() {
            return Ok(());
        }
        // 当前结果为 NULL 时直接采用首个非空值。
        if ctx.Value.IsNull() {
            ctx.Value = value;
            return Ok(());
        }
        let comparison = value.Compare(sc.TypeCtx(), &ctx.Value, self.ctor.as_ref())?;
        if (self.isMax && comparison > 0) || (!self.isMax && comparison < 0) {
            ctx.Value = value;
        }
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
        self.aggFunction.ResetContext(ctx, eval);
    }
}
