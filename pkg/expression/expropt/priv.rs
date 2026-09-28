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

// 权限检查（Privilege Checker）可选求值属性的 Provider / Reader。
//
// 表达式求值中偶发需要校验库/表/列级权限或动态权限名（如 BACKUP_ADMIN）；
// 通过可选属性注入 `PrivilegeChecker`，避免求值路径硬编码权限子系统。

use std::sync::Arc;

use crate::*;

/// 权限校验接口：静态权限类型与动态权限名两类请求。
pub trait PrivilegeChecker: Send + Sync {
    /// 按库/表/列与 `PrivilegeType` 请求校验，返回是否通过。
    fn request_verification(
        &self,
        db: &str,
        table: &str,
        column: &str,
        privilege: mysql::PrivilegeType,
    ) -> bool;

    /// 按动态权限名与是否可 GRANT 请求校验。
    fn request_dynamic_verification(&self, privilege_name: &str, grantable: bool) -> bool;
}

/// 返回 `PrivilegeChecker` 动态分发句柄的 Provider。
pub struct PrivilegeCheckerProvider {
    provider: Box<dyn Fn() -> Arc<dyn PrivilegeChecker> + Send + Sync>,
}

impl PrivilegeCheckerProvider {
    /// 用闭包构造 Provider。
    pub fn new<F: Fn() -> Arc<dyn PrivilegeChecker> + Send + Sync + 'static>(provider: F) -> Self {
        Self {
            provider: Box::new(provider),
        }
    }

    /// 调用闭包获取当前权限检查器。
    pub fn call(&self) -> Arc<dyn PrivilegeChecker> {
        (self.provider)()
    }
}

impl exprctx::OptionalEvalPropProvider for PrivilegeCheckerProvider {
    fn Desc(&self) -> &'static exprctx::OptionalEvalPropDesc {
        exprctx::OptPropPrivilegeChecker.Desc()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// 声明并读取 `OptPropPrivilegeChecker` 的 Reader。
pub struct PrivilegeCheckerPropReader;

impl RequireOptionalEvalProps for PrivilegeCheckerPropReader {
    fn required_optional_eval_props(&self) -> exprctx::OptionalEvalPropKeySet {
        exprctx::OptPropPrivilegeChecker.AsPropKeySet()
    }
}

impl PrivilegeCheckerPropReader {
    /// 从求值上下文取出权限检查器。
    pub fn get_privilege_checker<C: OptionalEvalPropContext + ?Sized>(
        &self,
        ctx: &C,
    ) -> anyhow::Result<Arc<dyn PrivilegeChecker>> {
        let provider = get_prop_provider::<PrivilegeCheckerProvider, _>(
            ctx,
            exprctx::OptPropPrivilegeChecker,
        )?;
        Ok(provider.call())
    }
}
