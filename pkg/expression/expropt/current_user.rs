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

// 当前用户与活跃角色的可选求值属性。
//
// 对应 Go `expropt` 中 `CURRENT_USER` / 角色相关内置函数所需的会话身份注入：
// 提供者惰性回调返回用户标识与角色列表；读取器从求值上下文取出后供表达式使用。

use std::sync::Arc;

use crate::*;

/// 可选属性提供者：闭包返回当前用户与活跃角色集合。
pub struct CurrentUserPropProvider {
    provider:
        Box<dyn Fn() -> (Arc<auth::UserIdentity>, Vec<Arc<auth::RoleIdentity>>) + Send + Sync>,
}

impl CurrentUserPropProvider {
    /// 用可发送/同步的回调构造提供者；回调在每次读取时调用。
    pub fn new<F>(provider: F) -> Self
    where
        F: Fn() -> (Arc<auth::UserIdentity>, Vec<Arc<auth::RoleIdentity>>) + Send + Sync + 'static,
    {
        Self {
            provider: Box::new(provider),
        }
    }

    /// 执行底层回调，返回用户身份与角色列表。
    pub fn call(&self) -> (Arc<auth::UserIdentity>, Vec<Arc<auth::RoleIdentity>>) {
        (self.provider)()
    }
}

impl exprctx::OptionalEvalPropProvider for CurrentUserPropProvider {
    fn Desc(&self) -> &'static exprctx::OptionalEvalPropDesc {
        exprctx::OptPropCurrentUser.Desc()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// 声明依赖 `OptPropCurrentUser`，并提供用户/角色读取入口。
pub struct CurrentUserPropReader;

impl RequireOptionalEvalProps for CurrentUserPropReader {
    fn required_optional_eval_props(&self) -> exprctx::OptionalEvalPropKeySet {
        exprctx::OptPropCurrentUser.AsPropKeySet()
    }
}

impl CurrentUserPropReader {
    /// 从上下文取得当前用户身份。
    pub fn current_user<C: OptionalEvalPropContext + ?Sized>(
        &self,
        ctx: &C,
    ) -> anyhow::Result<Arc<auth::UserIdentity>> {
        let (user, _) = self.get_provider(ctx)?.call();
        Ok(user)
    }

    /// 从上下文取得当前会话的活跃角色列表。
    pub fn active_roles<C: OptionalEvalPropContext + ?Sized>(
        &self,
        ctx: &C,
    ) -> anyhow::Result<Vec<Arc<auth::RoleIdentity>>> {
        let (_, roles) = self.get_provider(ctx)?.call();
        Ok(roles)
    }

    /// 取出 `CurrentUserPropProvider`；属性未注入时返回错误。
    fn get_provider<'a, C: OptionalEvalPropContext + ?Sized>(
        &self,
        ctx: &'a C,
    ) -> anyhow::Result<&'a CurrentUserPropProvider> {
        get_prop_provider(ctx, exprctx::OptPropCurrentUser)
    }
}
