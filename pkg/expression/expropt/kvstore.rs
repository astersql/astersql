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

// KV Store（键值存储）可选求值属性的 Provider / Reader。
//
// KV Store 对应底层分布式存储引擎（如 TiKV）的访问入口；部分表达式求值需要
// 直接拿到 Storage 句柄。具体类型 `T` 由调用方选定，以保持与 Go 侧泛型注入一致。

use std::sync::Arc;

use crate::*;

/// T is the concrete implementation of pkg/kv::Storage selected by the caller.
/// `T` 为调用方选定的 `pkg/kv::Storage` 具体实现类型。
pub struct KVStorePropProvider<T: Send + Sync + 'static> {
    provider: Box<dyn Fn() -> Arc<T> + Send + Sync>,
}

impl<T: Send + Sync + 'static> KVStorePropProvider<T> {
    /// 用返回 Storage 的闭包构造 Provider。
    pub fn new<F: Fn() -> Arc<T> + Send + Sync + 'static>(provider: F) -> Self {
        Self {
            provider: Box::new(provider),
        }
    }

    /// 调用闭包，返回当前 KV Store 句柄。
    pub fn call(&self) -> Arc<T> {
        (self.provider)()
    }
}

impl<T: Send + Sync + 'static> exprctx::OptionalEvalPropProvider for KVStorePropProvider<T> {
    fn Desc(&self) -> &'static exprctx::OptionalEvalPropDesc {
        exprctx::OptPropKVStore.Desc()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// 声明并读取 `OptPropKVStore` 的 Reader。
pub struct KVStorePropReader;

impl RequireOptionalEvalProps for KVStorePropReader {
    fn required_optional_eval_props(&self) -> exprctx::OptionalEvalPropKeySet {
        exprctx::OptPropKVStore.AsPropKeySet()
    }
}

impl KVStorePropReader {
    /// 从求值上下文取出并返回具体类型的 KV Store。
    pub fn get_kv_store<T, C>(&self, ctx: &C) -> anyhow::Result<Arc<T>>
    where
        T: Send + Sync + 'static,
        C: OptionalEvalPropContext + ?Sized,
    {
        let provider =
            get_prop_provider::<KVStorePropProvider<T>, _>(ctx, exprctx::OptPropKVStore)?;
        Ok(provider.call())
    }
}
