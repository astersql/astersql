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

// SUM_INT 聚合：仅接受整型参数，在整数域累加（可带 UNSIGNED / DISTINCT）。
//
// 与通用 SUM 不同，结果保持 longlong，便于 TiFlash 等引擎下推整数求和。

use crate::*;
use expression::Expression as _;
use std::sync::Arc;

/// SUM_INT 运行时聚合器。
pub struct sumIntFunction {
    /// 共享聚合描述符。
    pub aggFunction: aggFunction,
}
impl Aggregation for sumIntFunction {
    fn Update(
        &mut self,
        ctx: &mut AggEvaluateContext,
        _sc: &stmtctx::StatementContext,
        row: chunk::Row,
    ) -> Result<(), Error> {
        let argument = &self.aggFunction.AggFuncDesc.Args[0];
        let (value, is_null) = argument.EvalInt(ctx.Ctx.as_ref(), row)?;
        if is_null {
            return Ok(());
        }
        // 按参数无符号标志选择 u64 / i64 累加路径，并处理 DISTINCT。
        if mysql::HasUnsignedFlag(argument.GetType(ctx.Ctx.as_ref()).GetFlag()) {
            let value = value as u64;
            if let Some(checker) = &mut ctx.DistinctChecker
                && !checker.Check(vec![types::NewUintDatum(value)])?
            {
                return Ok(());
            }
            let total = if ctx.Value.IsNull() {
                value
            } else {
                types::AddUint64(ctx.Value.GetUint64(), value)
                    .map_err(|error| expression::errors::New(error.to_string()))?
            };
            ctx.Value.SetUint64(total);
        } else {
            if let Some(checker) = &mut ctx.DistinctChecker
                && !checker.Check(vec![types::NewIntDatum(value)])?
            {
                return Ok(());
            }
            let total = if ctx.Value.IsNull() {
                value
            } else {
                types::AddInt64(ctx.Value.GetInt64(), value)
                    .map_err(|error| expression::errors::New(error.to_string()))?
            };
            ctx.Value.SetInt64(total);
        }
        ctx.Count += 1;
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
