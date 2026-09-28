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

// GROUP_CONCAT 聚合：按分隔符拼接非空参数值，支持 DISTINCT、ORDER BY 与长度截断。
//
// 最后一个参数为 separator；超过 `group_concat_max_len` 时截断并告警一次。

use crate::*;
use expression::{Expression as _, StringerWithCtx as _};
use std::sync::Arc;

/// GROUP_CONCAT 运行时聚合器。
pub struct concatFunction {
    /// 共享聚合描述符。
    pub aggFunction: aggFunction,
    /// 当前组使用的分隔符字符串。
    pub separator: String,
    /// 允许的最大结果字节长度（来自会话变量）。
    pub maxLen: u64,
    /// 是否已解析过 separator 参数。
    pub sepInited: bool,
    /// 是否已对本组发出过截断告警。
    pub truncated: bool,
}

impl concatFunction {
    /// 将 Datum 以字节或字符串形式追加到结果缓冲。
    fn writeValue(eval_context: &mut AggEvaluateContext, value: types::Datum) -> Result<(), Error> {
        if value.Kind() == types::KindBytes {
            eval_context.Buffer.extend_from_slice(&value.GetBytes());
        } else {
            eval_context
                .Buffer
                .extend_from_slice(value.ToString()?.as_bytes());
        }
        Ok(())
    }

    /// 从最后一个参数求值得到 separator；NULL 视为非法。
    fn initSeparator(
        &mut self,
        ctx: &dyn expression::EvalContext,
        row: chunk::Row,
    ) -> Result<(), Error> {
        let value = self
            .aggFunction
            .AggFuncDesc
            .Args
            .last()
            .unwrap()
            .Eval(ctx, row)?;
        if value.IsNull() {
            return Err(expression::errors::New("Invalid separator argument"));
        }
        self.separator = value.ToString()?;
        Ok(())
    }
}

impl Aggregation for concatFunction {
    fn Update(
        &mut self,
        ctx: &mut AggEvaluateContext,
        sc: &stmtctx::StatementContext,
        row: chunk::Row,
    ) -> Result<(), Error> {
        if !self.sepInited {
            self.initSeparator(ctx.Ctx.as_ref(), row.clone())?;
            self.sepInited = true;
        }
        // 除 separator 外的参数参与拼接；任一为 NULL 则跳过本行。
        let value_args =
            &self.aggFunction.AggFuncDesc.Args[..self.aggFunction.AggFuncDesc.Args.len() - 1];
        let mut values = Vec::with_capacity(value_args.len());
        for argument in value_args {
            let value = argument.Eval(ctx.Ctx.as_ref(), row.clone())?;
            if value.IsNull() {
                return Ok(());
            }
            values.push(value);
        }
        if let Some(checker) = &mut ctx.DistinctChecker
            && !checker.Check(values.clone())?
        {
            return Ok(());
        }
        // 非首段前先写入 separator，再追加本行各字段。
        if ctx.BufferInitialized {
            ctx.Buffer.extend_from_slice(self.separator.as_bytes());
        }
        ctx.BufferInitialized = true;
        for value in values {
            Self::writeValue(ctx, value)?;
        }
        // 超长截断并仅告警一次，避免刷屏。
        if self.maxLen > 0 && ctx.Buffer.len() as u64 > self.maxLen {
            ctx.Buffer
                .truncate(self.maxLen.min(usize::MAX as u64) as usize);
            if !self.truncated {
                let argument = self.aggFunction.AggFuncDesc.Args[0]
                    .StringWithCtx(Some(ctx.Ctx.as_ref()), expression::errors::RedactLogDisable);
                sc.AppendWarning(errors::New(format!(
                    "Some rows were cut by GROUPCONCAT({argument})"
                )));
                self.truncated = true;
            }
        }
        Ok(())
    }
    fn GetResult(&self, ctx: &AggEvaluateContext) -> types::Datum {
        if !ctx.BufferInitialized {
            return types::Datum::default();
        }
        let mut value = types::Datum::default();
        value.SetString(
            String::from_utf8_lossy(&ctx.Buffer).into_owned(),
            self.aggFunction
                .AggFuncDesc
                .RetTp
                .as_ref()
                .map_or_else(String::new, |tp| tp.GetCollate().to_owned()),
        );
        value
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
