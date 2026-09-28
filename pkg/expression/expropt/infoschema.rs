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

// InfoSchema（信息模式，元数据视图）可选求值属性的 Provider / Reader。
//
// InfoSchema 描述库表列等元数据快照。表达式可能需要会话级快照或 Domain
// （域，节点级最新元数据）快照；`is_domain` 参数区分二者。

use std::sync::Arc;

use crate::*;

/// 按是否取 Domain 最新快照，返回具体 InfoSchema 实现的 Provider。
pub struct InfoSchemaPropProvider<T: infoschema::MetaOnlyInfoSchema + Send + Sync + 'static> {
    provider: Box<dyn Fn(bool) -> Arc<T> + Send + Sync>,
}

impl<T: infoschema::MetaOnlyInfoSchema + Send + Sync + 'static> InfoSchemaPropProvider<T> {
    /// 用闭包构造 Provider；`true` 表示 Domain 最新，`false` 表示会话级。
    pub fn new<F: Fn(bool) -> Arc<T> + Send + Sync + 'static>(provider: F) -> Self {
        Self {
            provider: Box::new(provider),
        }
    }

    /// 调用闭包获取对应 InfoSchema 引用。
    pub fn call(&self, is_domain: bool) -> Arc<T> {
        (self.provider)(is_domain)
    }
}

impl<T: infoschema::MetaOnlyInfoSchema + Send + Sync + 'static> exprctx::OptionalEvalPropProvider
    for InfoSchemaPropProvider<T>
{
    fn Desc(&self) -> &'static exprctx::OptionalEvalPropDesc {
        exprctx::OptPropInfoSchema.Desc()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// 声明并读取 `OptPropInfoSchema` 的 Reader。
pub struct InfoSchemaPropReader;

impl RequireOptionalEvalProps for InfoSchemaPropReader {
    fn required_optional_eval_props(&self) -> exprctx::OptionalEvalPropKeySet {
        exprctx::OptPropInfoSchema.AsPropKeySet()
    }
}

impl InfoSchemaPropReader {
    /// 获取会话绑定的 InfoSchema 快照（`is_domain = false`）。
    pub fn get_session_info_schema<T, C>(&self, ctx: &C) -> anyhow::Result<Arc<T>>
    where
        T: infoschema::MetaOnlyInfoSchema + Send + Sync + 'static,
        C: OptionalEvalPropContext + ?Sized,
    {
        let provider =
            get_prop_provider::<InfoSchemaPropProvider<T>, _>(ctx, exprctx::OptPropInfoSchema)?;
        Ok(provider.call(false))
    }

    /// 获取 Domain 最新 InfoSchema 快照（`is_domain = true`）。
    pub fn get_latest_info_schema<T, C>(&self, ctx: &C) -> anyhow::Result<Arc<T>>
    where
        T: infoschema::MetaOnlyInfoSchema + Send + Sync + 'static,
        C: OptionalEvalPropContext + ?Sized,
    {
        let provider =
            get_prop_provider::<InfoSchemaPropProvider<T>, _>(ctx, exprctx::OptPropInfoSchema)?;
        Ok(provider.call(true))
    }
}
