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

// 可选求值属性的注册表、Reader 契约与按类型安全取 Provider 的公共逻辑。
//
// 对应 Go 侧固定长度的可选属性 Provider 数组；表达式通过 `RequireOptionalEvalProps`
// 声明依赖，经 `get_prop_provider` 做缺失/键不一致/类型不匹配三类错误区分。

use std::any::Any;

use anyhow::{Result, anyhow};

use crate::*;

/// Declares the optional evaluation properties required by an expression.
/// 声明表达式求值所需的可选属性键集合。
pub trait RequireOptionalEvalProps {
    fn required_optional_eval_props(&self) -> exprctx::OptionalEvalPropKeySet;
}

/// The narrow part of EvalContext used by optional-property readers.
///
/// The blanket implementation keeps every existing exprctx::EvalContext
/// compatible while allowing focused tests to provide only optional properties.
///
/// Reader 使用的 EvalContext 窄接口：可取可选属性 Provider，以及可选的时区名。
/// 对完整 `exprctx::EvalContext` 提供 blanket 实现，测试则可只实现本 trait。
pub trait OptionalEvalPropContext {
    fn get_optional_prop_provider(
        &self,
        key: exprctx::OptionalEvalPropKey,
    ) -> Option<&dyn exprctx::OptionalEvalPropProvider>;

    fn location_name(&self) -> Option<String> {
        None
    }
}

impl<T: exprctx::EvalContext + ?Sized> OptionalEvalPropContext for T {
    fn get_optional_prop_provider(
        &self,
        key: exprctx::OptionalEvalPropKey,
    ) -> Option<&dyn exprctx::OptionalEvalPropProvider> {
        self.GetOptionalPropProvider(key)
    }

    fn location_name(&self) -> Option<String> {
        Some(self.Location().to_string())
    }
}

/// Fixed-size registry matching Go's [OptPropsCnt] provider array.
/// 与 Go `[OptPropsCnt]` 定长数组对齐的可选属性注册表。
pub struct OptionalEvalPropProviders {
    providers: Vec<Option<Box<dyn exprctx::OptionalEvalPropProvider>>>,
}

impl Default for OptionalEvalPropProviders {
    fn default() -> Self {
        Self::new()
    }
}

impl OptionalEvalPropProviders {
    /// 预分配 `OPT_PROPS_CNT` 个空槽位。
    pub fn new() -> Self {
        let mut providers = Vec::with_capacity(exprctx::OPT_PROPS_CNT);
        providers.resize_with(exprctx::OPT_PROPS_CNT, || None);
        Self { providers }
    }

    /// 指定键是否已注册 Provider。
    pub fn contains(&self, key: exprctx::OptionalEvalPropKey) -> bool {
        self.get(key).is_some()
    }

    /// 按键取 Provider；键越界或空槽返回 `None`，并断言 Desc 键一致。
    pub fn get(
        &self,
        key: exprctx::OptionalEvalPropKey,
    ) -> Option<&dyn exprctx::OptionalEvalPropProvider> {
        let provider = self.providers.get(key.0)?.as_deref()?;
        intest::Assert(provider.Desc().Key() == key, &[]);
        Some(provider)
    }

    /// 按 Provider 自描述键写入对应槽位（越界会断言/panic）。
    pub fn add(&mut self, provider: Box<dyn exprctx::OptionalEvalPropProvider>) {
        let key = provider.Desc().Key();
        intest::Assert(key.0 < exprctx::OPT_PROPS_CNT, &[]);
        assert!(
            key.0 < exprctx::OPT_PROPS_CNT,
            "optional property key {} is out of range",
            key.0
        );
        self.providers[key.0] = Some(provider);
    }

    /// 汇总已注册键为位集合（bit set）。
    pub fn prop_key_set(&self) -> exprctx::OptionalEvalPropKeySet {
        self.providers.iter().flatten().fold(
            exprctx::OptionalEvalPropKeySet::default(),
            |set, provider| set.Add(provider.Desc().Key()),
        )
    }
}

/// Returns a provider of the requested concrete type.
///
/// A missing key and a key/type mismatch remain distinct errors, matching the
/// two Go branches. Safe Any downcasting replaces Go's runtime type assertion.
///
/// 按请求的具体类型取 Provider：缺失键、键不一致、类型无法 downcast 分别报错，
/// 对应 Go 两分支错误语义；用安全的 `Any` downcast 替代运行时类型断言。
pub fn get_prop_provider<T, C>(ctx: &C, key: exprctx::OptionalEvalPropKey) -> Result<&T>
where
    T: exprctx::OptionalEvalPropProvider + Any,
    C: OptionalEvalPropContext + ?Sized,
{
    // 1) 属性未注册
    let provider = ctx
        .get_optional_prop_provider(key)
        .ok_or_else(|| anyhow!("optional property: '{}' not exists in EvalContext", key))?;

    // 2) Provider 自描述键与请求键不一致
    if provider.Desc().Key() != key {
        return Err(anyhow!(
            "optional property provider key '{}' does not match requested key '{}'",
            provider.Desc().Key(),
            key
        ));
    }

    // 3) 具体类型不匹配
    provider
        .as_any()
        .and_then(|value| value.downcast_ref::<T>())
        .ok_or_else(|| {
            anyhow!(
                "cannot cast OptionalEvalPropProvider to {} for key '{}'",
                std::any::type_name::<T>(),
                key
            )
        })
}
