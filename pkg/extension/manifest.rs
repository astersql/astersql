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

// 扩展清单（Manifest）与声明式配置选项。
//
// Manifest 描述单个扩展的名称、系统变量、动态权限、bootstrap、
// 自定义函数、访问检查、认证插件、会话处理器与关闭回调。
// `With*` 工厂生成 `Option` 闭包，在 `newManifestWithSetup` 中应用到清单，
// 并收集反向清理函数以便失败回滚。

use crate::auth::{AuthPlugin, validateAuthPlugin};
use crate::chunk;
use crate::function::{FunctionDef, register_extension_function, remove_extension_function};
use crate::mysql;
use crate::session::SessionHandler;
use crate::util::{ClearFunc, ExtensionContext, ExtensionError, clearFuncBuilder};
use crate::variable;
use std::sync::{Arc, OnceLock, RwLock};

/// 会话级可回收资源标记 trait；凡 `Send` 均可作为会话资源。
pub trait SessionResource: Send {}

impl<T: Send> SessionResource for T {}

/// 会话资源池：从池中取出/归还 `SessionResource`。
pub trait SessionPool: Send + Sync {
    /// 从池中获取一个会话资源。
    fn Get(&self) -> Result<Box<dyn SessionResource>, ExtensionError>;
    /// 将资源归还池中。
    fn Put(&self, resource: Box<dyn SessionResource>);
}

/// 作用于 `Manifest` 的配置选项闭包类型。
pub type Option = Arc<dyn Fn(&mut Manifest) + Send + Sync>;

/// 配置扩展自定义系统变量列表。
pub fn WithCustomSysVariables(
    variables: Vec<std::option::Option<Arc<variable::SysVar>>>,
) -> Option {
    Arc::new(move |manifest| manifest.sysVariables = variables.clone())
}

/// 配置扩展声明的动态权限（dynamic privilege）名称列表。
pub fn WithCustomDynPrivs(privileges: Vec<String>) -> Option {
    Arc::new(move |manifest| manifest.dynPrivs = privileges.clone())
}

/// 配置扩展注册的自定义函数定义列表。
pub fn WithCustomFunctions(functions: Vec<Arc<FunctionDef>>) -> Option {
    Arc::new(move |manifest| manifest.funcs = functions.clone())
}

/// 访问检查回调：根据用户/主机/库与权限类型返回所需的动态权限名。
pub type AccessCheckFunc =
    Arc<dyn Fn(&str, &str, &str, mysql::PrivilegeType, bool) -> Vec<String> + Send + Sync>;

/// 配置自定义访问检查函数。
pub fn WithCustomAccessCheck(function: AccessCheckFunc) -> Option {
    Arc::new(move |manifest| manifest.accessCheckFunc = Some(Arc::clone(&function)))
}

/// 配置扩展提供的认证插件列表。
pub fn WithCustomAuthPlugins(plugins: Vec<Arc<AuthPlugin>>) -> Option {
    Arc::new(move |manifest| manifest.authPlugins = Some(plugins.clone()))
}

/// 会话事件处理器工厂：每次新建会话时可产出一个 `SessionHandler`。
pub type SessionHandlerFactory = Arc<dyn Fn() -> std::option::Option<SessionHandler> + Send + Sync>;

/// 配置会话处理器工厂。
pub fn WithSessionHandlerFactory<F>(factory: F) -> Option
where
    F: Fn() -> std::option::Option<SessionHandler> + Send + Sync + 'static,
{
    let factory: SessionHandlerFactory = Arc::new(factory);
    Arc::new(move |manifest| manifest.sessionHandlerFactory = Some(Arc::clone(&factory)))
}

/// 扩展关闭时调用的清理回调类型。
pub type CloseFunc = Arc<dyn Fn() + Send + Sync>;

/// 配置扩展关闭回调。
pub fn WithClose<F>(function: F) -> Option
where
    F: Fn() + Send + Sync + 'static,
{
    let function: CloseFunc = Arc::new(function);
    Arc::new(move |manifest| manifest.close = Some(Arc::clone(&function)))
}

/// Bootstrap 上下文：在扩展初始化阶段执行 SQL、访问 etcd 与会话池。
pub trait BootstrapContext: ExtensionContext {
    /// 执行一条 SQL，返回结果行集合。
    fn ExecuteSQL(&mut self, sql: &str) -> Result<Vec<chunk::Row>, ExtensionError>;
    /// 可选的 etcd 客户端，用于分布式协调。
    fn EtcdClient(&self) -> std::option::Option<&etcd_client::Client>;
    /// 会话资源池引用。
    fn SessionPool(&self) -> &dyn SessionPool;
}

/// Bootstrap 回调：在 Setup 完成后对集群做一次性初始化。
pub type BootstrapFunc =
    Arc<dyn Fn(&mut dyn BootstrapContext) -> Result<(), ExtensionError> + Send + Sync>;

/// 配置自定义 Bootstrap 函数。
pub fn WithBootstrap<F>(function: F) -> Option
where
    F: Fn(&mut dyn BootstrapContext) -> Result<(), ExtensionError> + Send + Sync + 'static,
{
    let function: BootstrapFunc = Arc::new(function);
    Arc::new(move |manifest| manifest.bootstrap = Some(Arc::clone(&function)))
}

/// 以 SQL 列表形式配置 Bootstrap：按顺序在上下文中执行每条语句。
pub fn WithBootstrapSQL(sql_list: Vec<String>) -> Option {
    WithBootstrap(move |context| {
        for sql in &sql_list {
            context.ExecuteSQL(sql)?;
        }
        Ok(())
    })
}

/// 单个扩展的清单：聚合名称与各类可插拔能力。
pub struct Manifest {
    name: String,
    pub(crate) sysVariables: Vec<std::option::Option<Arc<variable::SysVar>>>,
    pub(crate) dynPrivs: Vec<String>,
    pub(crate) bootstrap: std::option::Option<BootstrapFunc>,
    pub(crate) funcs: Vec<Arc<FunctionDef>>,
    pub(crate) accessCheckFunc: std::option::Option<AccessCheckFunc>,
    pub(crate) authPlugins: std::option::Option<Vec<Arc<AuthPlugin>>>,
    pub(crate) sessionHandlerFactory: std::option::Option<SessionHandlerFactory>,
    pub(crate) close: std::option::Option<CloseFunc>,
}

impl Manifest {
    /// 创建仅含名称、其余字段为空的清单。
    fn empty(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            sysVariables: Vec::new(),
            dynPrivs: Vec::new(),
            bootstrap: None,
            funcs: Vec::new(),
            accessCheckFunc: None,
            authPlugins: None,
            sessionHandlerFactory: None,
            close: None,
        }
    }

    /// 返回扩展名称。
    pub fn Name(&self) -> &str {
        &self.name
    }
}

/// 向权限子系统注册动态权限的钩子类型。
pub type RegisterDynamicPrivilege = Arc<dyn Fn(&str) -> Result<(), ExtensionError> + Send + Sync>;
/// 从权限子系统移除动态权限的钩子类型。
pub type RemoveDynamicPrivilege = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// 已安装的动态权限注册/移除钩子对。
#[derive(Clone)]
struct DynamicPrivilegeHooks {
    register: RegisterDynamicPrivilege,
    remove: RemoveDynamicPrivilege,
}

static DYNAMIC_PRIVILEGE_HOOKS: OnceLock<RwLock<std::option::Option<DynamicPrivilegeHooks>>> =
    OnceLock::new();

/// 获取全局动态权限钩子存储（惰性初始化）。
fn dynamic_privilege_hooks() -> &'static RwLock<std::option::Option<DynamicPrivilegeHooks>> {
    DYNAMIC_PRIVILEGE_HOOKS.get_or_init(|| RwLock::new(None))
}

/// 由权限子系统安装动态权限注册/移除钩子，供扩展 Setup 时调用。
pub fn InstallDynamicPrivilegeHooks(
    register: RegisterDynamicPrivilege,
    remove: RemoveDynamicPrivilege,
) {
    *dynamic_privilege_hooks()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        Some(DynamicPrivilegeHooks { register, remove });
}

/// 通过已安装钩子注册单个动态权限；未安装则报错。
fn register_dynamic_privilege(privilege: &str) -> Result<(), ExtensionError> {
    let hooks = dynamic_privilege_hooks()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
        .ok_or_else(|| ExtensionError::new("RegisterDynamicPrivilege is not installed"))?;
    (hooks.register)(privilege)
}

/// 通过已安装钩子移除动态权限；未安装则静默跳过。
fn remove_dynamic_privilege(privilege: &str) {
    if let Some(hooks) = dynamic_privilege_hooks()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
    {
        (hooks.remove)(privilege);
    }
}

/// 根据工厂产出的选项构建 Manifest，并注册其声明的资源；返回清单与清理函数。
///
/// 任一步失败时执行已收集的清理回调并返回错误。
pub(crate) fn newManifestWithSetup<F>(
    name: String,
    factory: F,
) -> Result<(Manifest, ClearFunc), ExtensionError>
where
    F: FnOnce() -> Result<Vec<Option>, ExtensionError>,
{
    let mut clear_builder = clearFuncBuilder::default();
    // 在闭包内完成选项应用与资源注册，便于统一错误回滚。
    let result = (|| {
        let mut manifest = Manifest::empty(name);
        let options = factory()?;
        for option in options {
            option(&mut manifest);
        }
        // 关闭回调加入清理链，Reset/失败时调用。
        if let Some(close) = &manifest.close {
            let close = Arc::clone(close);
            clear_builder
                .DoWithCollectClear(move || Ok(Some(Box::new(move || close()) as ClearFunc)))?;
        }

        // 注册动态权限，并为每项登记移除清理。
        for privilege in &manifest.dynPrivs {
            register_dynamic_privilege(privilege)?;
            let privilege = privilege.clone();
            clear_builder.DoWithCollectClear(move || {
                Ok(Some(
                    Box::new(move || remove_dynamic_privilege(&privilege)) as ClearFunc
                ))
            })?;
        }

        // 注册系统变量：校验非空名且未重复，并登记注销清理。
        for sys_var in &manifest.sysVariables {
            let sys_var = sys_var
                .as_ref()
                .ok_or_else(|| ExtensionError::new("system var should not be nil"))?;
            if sys_var.Name.is_empty() {
                return Err(ExtensionError::new("system var name should not be empty"));
            }
            if variable::GetSysVar(&sys_var.Name).is_some() {
                return Err(ExtensionError::new(format!(
                    "system var '{}' has already registered",
                    sys_var.Name
                )));
            }
            variable::RegisterSysVar((**sys_var).clone());
            let name = sys_var.Name.clone();
            clear_builder.DoWithCollectClear(move || {
                Ok(Some(
                    Box::new(move || variable::UnregisterSysVar(&name)) as ClearFunc
                ))
            })?;
        }

        // 注册扩展函数，并为每个函数名登记移除清理。
        for definition in &manifest.funcs {
            register_extension_function(definition)?;
            let name = definition.Name.clone();
            clear_builder.DoWithCollectClear(move || {
                Ok(Some(
                    Box::new(move || remove_extension_function(&name)) as ClearFunc
                ))
            })?;
        }

        validateAuthPlugin(&manifest)?;
        Ok(manifest)
    })();

    match result {
        Ok(manifest) => Ok((manifest, clear_builder.Build())),
        Err(error) => {
            // 失败时立即执行已收集的清理，回滚部分注册。
            clear_builder.Build()();
            Err(error)
        }
    }
}
