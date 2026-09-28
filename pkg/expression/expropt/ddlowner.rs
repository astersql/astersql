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

// DDL Owner（DDL 所有者）可选求值属性的 Provider / Reader。
//
// DDL Owner 是集群中负责执行 DDL（数据定义语言，如 CREATE/ALTER TABLE）的节点角色；
// 表达式求值偶发需要查询「当前节点是否为 DDL Owner」，通过可选属性注入，避免对
// `EvalContext`（表达式求值上下文）产生硬依赖。

use crate::*;

/// 提供「当前是否为 DDL Owner」的闭包封装，注册到可选属性表。
pub struct DDLOwnerInfoProvider {
    provider: Box<dyn Fn() -> bool + Send + Sync>,
}

impl DDLOwnerInfoProvider {
    /// 用可调用闭包构造 Provider；闭包需可跨线程共享。
    pub fn new<F: Fn() -> bool + Send + Sync + 'static>(provider: F) -> Self {
        Self {
            provider: Box::new(provider),
        }
    }

    /// 调用底层闭包，返回当前节点是否为 DDL Owner。
    pub fn call(&self) -> bool {
        (self.provider)()
    }
}

impl exprctx::OptionalEvalPropProvider for DDLOwnerInfoProvider {
    fn Desc(&self) -> &'static exprctx::OptionalEvalPropDesc {
        exprctx::OptPropDDLOwnerInfo.Desc()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// 声明并读取 `OptPropDDLOwnerInfo` 的 Reader 空结构体。
pub struct DDLOwnerPropReader;

impl RequireOptionalEvalProps for DDLOwnerPropReader {
    fn required_optional_eval_props(&self) -> exprctx::OptionalEvalPropKeySet {
        exprctx::OptPropDDLOwnerInfo.AsPropKeySet()
    }
}

impl DDLOwnerPropReader {
    /// 从求值上下文取出 Provider 并查询是否为 DDL Owner。
    ///
    /// 若属性未注册或类型不匹配，返回错误（与 Go 侧缺失/断言失败语义对齐）。
    pub fn is_ddl_owner<C: OptionalEvalPropContext + ?Sized>(
        &self,
        ctx: &C,
    ) -> anyhow::Result<bool> {
        // 按键取具体类型的 Provider，再调用闭包
        let provider =
            get_prop_provider::<DDLOwnerInfoProvider, _>(ctx, exprctx::OptPropDDLOwnerInfo)?;
        Ok(provider.call())
    }
}
