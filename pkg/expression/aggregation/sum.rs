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

// SUM 聚合：委托 `aggFunction::updateSum`，按 MySQL 规则累加 DECIMAL/DOUBLE。

use crate::*;
use std::sync::Arc;

/// SUM 运行时聚合器。
pub struct sumFunction {
    /// 共享聚合描述符与求和辅助逻辑。
    pub aggFunction: aggFunction,
}

impl Aggregation for sumFunction {
    fn Update(
        &mut self,
        ctx: &mut AggEvaluateContext,
        sc: &stmtctx::StatementContext,
        row: chunk::Row,
    ) -> Result<(), Error> {
        self.aggFunction.updateSum(sc.TypeCtx(), ctx, row)
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
        self.aggFunction.ResetContext(ctx, eval)
    }
}
