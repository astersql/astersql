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

// 扩展会话侧事件与认证插件聚合。
//
// 从各 Manifest 的 `SessionHandler` 工厂收集连接/语句事件回调，
// 以及认证插件映射；提供包级便捷函数在 `Option<&SessionExtensions>` 上安全分发。

use crate::ast;
use crate::auth::AuthPlugin;
use crate::auth_identity;
use crate::extensions::Extensions;
use crate::parser;
use crate::stmtctx;
use crate::types;
use crate::util::ExtensionError;
use crate::variable;
use std::collections::HashMap;
use std::sync::Arc;

/// 连接生命周期事件携带的上下文信息。
pub struct ConnEventInfo {
    /// 连接级变量与元信息。
    pub ConnectionInfo: Option<Arc<variable::ConnectionInfo>>,
    /// 会话别名。
    pub SessionAlias: String,
    /// 当前激活的角色身份列表。
    pub ActiveRoles: Vec<Arc<auth_identity::RoleIdentity>>,
    /// 事件相关错误（若有）。
    pub Error: Option<ExtensionError>,
}

impl Default for ConnEventInfo {
    fn default() -> Self {
        Self {
            ConnectionInfo: None,
            SessionAlias: String::new(),
            ActiveRoles: Vec::new(),
            Error: None,
        }
    }
}

/// 连接事件类型：建连、握手成功/拒绝、复位、断开。
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnEventTp {
    ConnConnected = 0,
    ConnHandshakeAccepted = 1,
    ConnHandshakeRejected = 2,
    ConnReset = 3,
    ConnDisconnected = 4,
}

/// 语句事件类型：执行失败或成功。
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StmtEventTp {
    StmtError = 0,
    StmtSuccess = 1,
}

/// 语句事件信息抽象：用户、库、SQL 摘要、影响行数、相关表等。
pub trait StmtEventInfo {
    fn User(&self) -> Option<&auth_identity::UserIdentity>;
    fn ActiveRoles(&self) -> Vec<&auth_identity::RoleIdentity>;
    fn CurrentDB(&self) -> &str;
    fn ConnectionInfo(&self) -> Option<&variable::ConnectionInfo>;
    fn SessionAlias(&self) -> &str;
    fn StmtNode(&self) -> Option<&dyn ast::Node>;
    fn ExecuteStmtNode(&self) -> Option<&ast::ExecuteStmt>;
    fn ExecutePreparedStmt(&self) -> Option<&dyn ast::Node>;
    fn PreparedParams(&self) -> Vec<types::Datum>;
    fn OriginalText(&self) -> &str;
    /// 返回规范化文本与可选的 SQL digest（摘要指纹）。
    fn SQLDigest(&self) -> (String, Option<Arc<parser::Digest>>);
    fn AffectedRows(&self) -> u64;
    fn RelatedTables(&self) -> Vec<stmtctx::TableEntry>;
    fn GetError(&self) -> Option<&ExtensionError>;
}

/// 连接事件回调类型。
pub type ConnectionEventFunc = Arc<dyn Fn(ConnEventTp, &ConnEventInfo) + Send + Sync>;
/// 语句事件回调类型。
pub type StmtEventFunc = Arc<dyn Fn(StmtEventTp, &dyn StmtEventInfo) + Send + Sync>;

/// 单个扩展提供的会话事件处理器。
#[derive(Default)]
pub struct SessionHandler {
    pub OnConnectionEvent: Option<ConnectionEventFunc>,
    pub OnStmtEvent: Option<StmtEventFunc>,
}

/// 遍历全部 Manifest，聚合会话处理器回调与认证插件到 `SessionExtensions`。
pub fn newSessionExtensions(extensions: &Extensions) -> SessionExtensions {
    let mut result = SessionExtensions::default();
    for manifest in extensions.Manifests() {
        if let Some(factory) = &manifest.sessionHandlerFactory {
            if let Some(handler) = factory() {
                if let Some(function) = handler.OnConnectionEvent {
                    result.connectionEventFuncs.push(function);
                }
                if let Some(function) = handler.OnStmtEvent {
                    result.stmtEventFuncs.push(function);
                }
            }
        }
        if let Some(plugins) = &manifest.authPlugins {
            // Go allocates a fresh map for every non-nil authPlugins slice.
            // 与 Go 一致：非空插件切片会清空再建映射，后者覆盖前者。
            result.authPlugins.clear();
            for plugin in plugins {
                result
                    .authPlugins
                    .insert(plugin.Name.clone(), Arc::clone(plugin));
            }
        }
    }
    result
}

/// 单个会话上挂载的扩展能力集合。
#[derive(Default)]
pub struct SessionExtensions {
    connectionEventFuncs: Vec<ConnectionEventFunc>,
    stmtEventFuncs: Vec<StmtEventFunc>,
    authPlugins: HashMap<String, Arc<AuthPlugin>>,
}

impl SessionExtensions {
    /// 向所有已注册连接事件回调广播事件。
    pub fn OnConnectionEvent(&self, event_type: ConnEventTp, event: &ConnEventInfo) {
        for function in &self.connectionEventFuncs {
            function(event_type, event);
        }
    }

    /// 是否存在语句事件监听器。
    pub fn HasStmtEventListeners(&self) -> bool {
        !self.stmtEventFuncs.is_empty()
    }

    /// 向所有已注册语句事件回调广播事件。
    pub fn OnStmtEvent(&self, event_type: StmtEventTp, event: &dyn StmtEventInfo) {
        for function in &self.stmtEventFuncs {
            function(event_type, event);
        }
    }

    /// 按名称查找认证插件，返回 `(插件, 是否存在)`。
    pub fn GetAuthPlugin(&self, name: &str) -> (Option<Arc<AuthPlugin>>, bool) {
        let plugin = self.authPlugins.get(name).cloned();
        let exists = plugin.is_some();
        (plugin, exists)
    }
}

/// 在可选的 SessionExtensions 上分发连接事件；为 None 时无操作。
pub fn OnConnectionEvent(
    extensions: Option<&SessionExtensions>,
    event_type: ConnEventTp,
    event: &ConnEventInfo,
) {
    if let Some(extensions) = extensions {
        extensions.OnConnectionEvent(event_type, event);
    }
}

/// 在可选的 SessionExtensions 上分发语句事件；为 None 时无操作。
pub fn OnStmtEvent(
    extensions: Option<&SessionExtensions>,
    event_type: StmtEventTp,
    event: &dyn StmtEventInfo,
) {
    if let Some(extensions) = extensions {
        extensions.OnStmtEvent(event_type, event);
    }
}

/// 查询可选 SessionExtensions 是否有语句事件监听器。
pub fn HasStmtEventListeners(extensions: Option<&SessionExtensions>) -> bool {
    extensions.is_some_and(SessionExtensions::HasStmtEventListeners)
}

/// 在可选 SessionExtensions 上按名查找认证插件。
pub fn GetAuthPlugin(
    extensions: Option<&SessionExtensions>,
    name: &str,
) -> (Option<Arc<AuthPlugin>>, bool) {
    extensions.map_or((None, false), |extensions| extensions.GetAuthPlugin(name))
}
