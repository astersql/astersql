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

// BIT_XOR 聚合：对非空输入做按位异或，空输入组默认结果为 0。

use crate::*;
use expression::Expression as _;
use std::sync::Arc;

/// BIT_XOR 运行时聚合器。
pub struct bitXorFunction {
    /// 共享聚合描述符。
    pub aggFunction: aggFunction,
}
impl Aggregation for bitXorFunction {
    fn Update(
        &mut self,
        ctx: &mut AggEvaluateContext,
        sc: &stmtctx::StatementContext,
        row: chunk::Row,
    ) -> Result<(), Error> {
        let value = self.aggFunction.AggFuncDesc.Args[0].Eval(ctx.Ctx.as_ref(), row)?;
        if !value.IsNull() {
            // 统一为 u64 后与累计值做按位异或；NULL 行跳过。
            let value = if value.Kind() == types::KindUint64 {
                value.GetUint64()
            } else {
                value.ToInt64(sc.TypeCtx())? as u64
            };
            ctx.Value.SetUint64(ctx.Value.GetUint64() ^ value);
        }
        Ok(())
    }
    fn GetResult(&self, ctx: &AggEvaluateContext) -> types::Datum {
        ctx.Value.clone()
    }
    fn GetPartialResult(&self, ctx: &AggEvaluateContext) -> Vec<types::Datum> {
        vec![self.GetResult(ctx)]
    }
    /// 初值设为 0，异或单位元。
    fn CreateContext(&self, ctx: Arc<dyn expression::EvalContext>) -> AggEvaluateContext {
        let mut e = self.aggFunction.CreateContext(ctx);
        e.Value.SetUint64(0);
        e
    }
    fn ResetContext(&mut self, ctx: Arc<dyn expression::EvalContext>, e: &mut AggEvaluateContext) {
        e.Ctx = ctx;
        e.Value.SetUint64(0);
    }
}
