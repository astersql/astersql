// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 扩展自定义 SQL 函数定义与注册钩子。
//
// 定义函数求值上下文、字符串/整型求值回调、函数元数据校验，
// 以及通过全局钩子把扩展函数注册/卸载到表达式子系统的入口。

use crate::auth_identity;
use crate::chunk;
use crate::types;
use crate::util::{ExtensionContext, ExtensionError};
use crate::variable;
use std::sync::{Arc, OnceLock, RwLock};

/// 扩展函数求值时可见的会话与行上下文。
pub trait FunctionContext: ExtensionContext {
    /// 当前登录用户身份。
    fn User(&self) -> Option<&auth_identity::UserIdentity>;
    /// 当前激活角色列表。
    fn ActiveRoles(&self) -> Vec<&auth_identity::RoleIdentity>;
    /// 当前默认数据库名。
    fn CurrentDB(&self) -> String;
    /// 连接信息（客户端地址等）。
    fn ConnectionInfo(&self) -> Option<&variable::ConnectionInfo>;
    /// 将当前行参数求值为 Datum 列表。
    fn EvalArgs(&self, row: chunk::Row) -> Result<Vec<types::Datum>, ExtensionError>;
}

/// 返回字符串结果的求值回调；(值, 是否为 NULL)。
pub type EvalStringFunc = Arc<
    dyn Fn(&dyn FunctionContext, chunk::Row) -> Result<(String, bool), ExtensionError>
        + Send
        + Sync,
>;
/// 返回整型结果的求值回调；(值, 是否为 NULL)。
pub type EvalIntFunc = Arc<
    dyn Fn(&dyn FunctionContext, chunk::Row) -> Result<(i64, bool), ExtensionError> + Send + Sync,
>;
/// 声明本函数所需的动态权限名列表；参数表示是否要求 GRANT OPTION。
pub type RequireDynamicPrivileges = Arc<dyn Fn(bool) -> Vec<String> + Send + Sync>;

/// 扩展函数元数据：名称、求值类型、参数类型与可选回调。
pub struct FunctionDef {
    /// 函数名，注册前不可为空。
    pub Name: String,
    /// 返回值求值类型（EvalType）。
    pub EvalTp: types::EvalType,
    /// 各位置参数的求值类型。
    pub ArgTps: Vec<types::EvalType>,
    /// 可选参数个数；须在 `[0, ArgTps.len()]` 内。
    pub OptionalArgsLen: i32,
    /// 字符串求值实现（按返回类型选用）。
    pub EvalStringFunc: Option<EvalStringFunc>,
    /// 整型求值实现（按返回类型选用）。
    pub EvalIntFunc: Option<EvalIntFunc>,
    /// 可选：动态权限需求声明。
    pub RequireDynamicPrivileges: Option<RequireDynamicPrivileges>,
}

impl Default for FunctionDef {
    fn default() -> Self {
        Self {
            Name: String::new(),
            EvalTp: types::EvalType(0),
            ArgTps: Vec::new(),
            OptionalArgsLen: 0,
            EvalStringFunc: None,
            EvalIntFunc: None,
            RequireDynamicPrivileges: None,
        }
    }
}

impl FunctionDef {
    /// 校验函数名非空，且 OptionalArgsLen 落在合法区间。
    pub fn Validate(&self) -> Result<(), ExtensionError> {
        if self.Name.is_empty() {
            return Err(ExtensionError::new(
                "extension function name should not be empty",
            ));
        }
        if self.OptionalArgsLen < 0 || self.OptionalArgsLen as usize > self.ArgTps.len() {
            return Err(ExtensionError::new(format!(
                "invalid OptionalArgsLen: {}",
                self.OptionalArgsLen
            )));
        }
        Ok(())
    }
}

/// 将函数定义注册到表达式子系统的钩子类型。
pub type RegisterFunction =
    Arc<dyn Fn(&Arc<FunctionDef>) -> Result<(), ExtensionError> + Send + Sync>;
/// 按名称移除已注册扩展函数的钩子类型。
pub type RemoveFunction = Arc<dyn Fn(&str) + Send + Sync>;

/// 成对保存注册与移除钩子。
#[derive(Clone)]
struct FunctionHooks {
    register: RegisterFunction,
    remove: RemoveFunction,
}

/// 进程级钩子存储；未安装时注册会报错。
static FUNCTION_HOOKS: OnceLock<RwLock<Option<FunctionHooks>>> = OnceLock::new();

/// 惰性初始化并返回全局钩子锁。
fn function_hooks() -> &'static RwLock<Option<FunctionHooks>> {
    FUNCTION_HOOKS.get_or_init(|| RwLock::new(None))
}

/// 由内核安装扩展函数的注册/卸载钩子。
pub fn InstallExtensionFunctionHooks(register: RegisterFunction, remove: RemoveFunction) {
    *function_hooks()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        Some(FunctionHooks { register, remove });
}

/// 通过已安装钩子注册扩展函数；钩子未安装则报错。
pub(crate) fn register_extension_function(
    definition: &Arc<FunctionDef>,
) -> Result<(), ExtensionError> {
    let hooks = function_hooks()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
        .ok_or_else(|| ExtensionError::new("RegisterExtensionFunc is not installed"))?;
    (hooks.register)(definition)
}

/// 若钩子已安装则按名移除扩展函数；未安装时静默忽略。
pub(crate) fn remove_extension_function(name: &str) {
    if let Some(hooks) = function_hooks()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
    {
        (hooks.remove)(name);
    }
}
