// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 序列（Sequence）算子可选求值属性的 Provider / Reader。
//
// Sequence 是按规则生成单调数值的数据库对象；`NEXTVAL` / `SETVAL` 等表达式需要
// 按库名与序列名取得算子句柄。Provider 可能因权限或对象不存在返回错误。

use crate::*;

/// 序列算子：查询 id、取下一值、设置当前值。
pub trait SequenceOperator {
    /// 返回序列对象 id。
    fn get_sequence_id(&self) -> i64;
    /// 推进并返回下一个序列值。
    fn get_sequence_next_val(&mut self) -> anyhow::Result<i64>;
    /// 设置序列值；返回（生效后的值，新值是否已低于当前基线）。
    fn set_sequence_val(&mut self, new_val: i64) -> anyhow::Result<(i64, bool)>;
}

/// 按库名与序列名解析 `SequenceOperator` 的 Provider。
pub struct SequenceOperatorProvider {
    provider: Box<dyn Fn(&str, &str) -> anyhow::Result<Box<dyn SequenceOperator>> + Send + Sync>,
}

impl SequenceOperatorProvider {
    /// 用 `(db, name) -> Result<Operator>` 闭包构造 Provider。
    pub fn new<F>(provider: F) -> Self
    where
        F: Fn(&str, &str) -> anyhow::Result<Box<dyn SequenceOperator>> + Send + Sync + 'static,
    {
        Self {
            provider: Box::new(provider),
        }
    }

    /// 按库名与序列名调用闭包。
    pub fn call(&self, db: &str, name: &str) -> anyhow::Result<Box<dyn SequenceOperator>> {
        (self.provider)(db, name)
    }
}

impl exprctx::OptionalEvalPropProvider for SequenceOperatorProvider {
    fn Desc(&self) -> &'static exprctx::OptionalEvalPropDesc {
        exprctx::OptPropSequenceOperator.Desc()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// 声明并读取 `OptPropSequenceOperator` 的 Reader。
pub struct SequenceOperatorPropReader;

impl RequireOptionalEvalProps for SequenceOperatorPropReader {
    fn required_optional_eval_props(&self) -> exprctx::OptionalEvalPropKeySet {
        exprctx::OptPropSequenceOperator.AsPropKeySet()
    }
}

impl SequenceOperatorPropReader {
    /// 从上下文取 Provider，再按 db/name 解析序列算子。
    pub fn get_sequence_operator<C: OptionalEvalPropContext + ?Sized>(
        &self,
        ctx: &C,
        db: &str,
        name: &str,
    ) -> anyhow::Result<Box<dyn SequenceOperator>> {
        let provider = get_prop_provider::<SequenceOperatorProvider, _>(
            ctx,
            exprctx::OptPropSequenceOperator,
        )?;
        provider.call(db, name)
    }
}
