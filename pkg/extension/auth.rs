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

// 扩展鉴权（auth）插件与连接适配模块。
//
// 定义自定义认证插件（Auth Plugin）的回调约定、鉴权请求/权限校验入参，
// 以及把 privilege 层原始连接适配为扩展侧 `AuthConn` 的适配器。
// 注册前通过 [`validate_auth_plugins`] 校验名称唯一性、非空必填回调，
// 并禁止占用 MySQL 内置默认鉴权插件名。

use crate::auth_identity;
use crate::manifest::Manifest;
use crate::mysql;
use crate::privilege_conn;
use crate::util::ExtensionError;
use std::collections::HashSet;
use std::sync::Arc;

/// TLS 握手后的服务端连接状态别名（基于 rustls）。
pub type TlsConnectionState = rustls::ServerConnection;

/// 扩展鉴权过程中与客户端交换更多数据的连接抽象。
///
/// 对应 MySQL 认证握手里 AuthMoreData / 读写包 / Flush 的能力边界。
pub trait AuthConn: Send {
    /// 向客户端写入 AuthMoreData 负载。
    fn WriteAuthMoreData(&mut self, data: &[u8]) -> Result<(), ExtensionError>;
    /// 读取下一帧客户端认证相关数据包。
    fn ReadPacket(&mut self) -> Result<Vec<u8>, ExtensionError>;
    /// 刷新发送缓冲，确保已写数据到达对端。
    fn Flush(&mut self) -> Result<(), ExtensionError>;
}

/// 将 privilege 层原始鉴权连接包装为 [`AuthConn`] 的适配器。
pub struct AuthConnAdapter<T, C> {
    /// 底层原始连接实现。
    inner: T,
    /// Flush 时传入底层的上下文（如会话/连接标识）。
    context: C,
}

impl<T, C> AuthConnAdapter<T, C> {
    /// 用底层连接与上下文构造适配器。
    pub fn new(inner: T, context: C) -> Self {
        Self { inner, context }
    }

    /// 拆出底层原始连接，丢弃适配器外壳。
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T, C> AuthConn for AuthConnAdapter<T, C>
where
    T: privilege_conn::RawAuthConn<Context = C> + Send,
    C: Send,
{
    fn WriteAuthMoreData(&mut self, data: &[u8]) -> Result<(), ExtensionError> {
        // 将底层错误统一映射为扩展错误类型。
        self.inner
            .WriteAuthMoreData(data)
            .map_err(|error| ExtensionError::new(error.to_string()))
    }

    fn ReadPacket(&mut self) -> Result<Vec<u8>, ExtensionError> {
        self.inner
            .ReadPacket()
            .map_err(|error| ExtensionError::new(error.to_string()))
    }

    fn Flush(&mut self) -> Result<(), ExtensionError> {
        // Flush 需要带上适配器持有的上下文。
        self.inner
            .Flush(&self.context)
            .map_err(|error| ExtensionError::new(error.to_string()))
    }
}

/// 用户认证回调：根据请求校验身份，失败返回扩展错误。
pub type AuthenticateUserFunc =
    Arc<dyn Fn(AuthenticateRequest) -> Result<(), ExtensionError> + Send + Sync>;
/// 由明文密码生成可持久化的认证串，并返回是否成功。
pub type GenerateAuthStringFunc = Arc<dyn Fn(String) -> (String, bool) + Send + Sync>;
/// 校验已存储认证串格式是否合法。
pub type ValidateAuthStringFunc = Arc<dyn Fn(String) -> bool + Send + Sync>;
/// 静态权限（如 SELECT/INSERT）校验回调。
pub type VerifyPrivilegeFunc = Arc<dyn Fn(VerifyStaticPrivRequest) -> bool + Send + Sync>;
/// 动态权限（按名称字符串描述的特权）校验回调。
pub type VerifyDynamicPrivilegeFunc = Arc<dyn Fn(VerifyDynamicPrivRequest) -> bool + Send + Sync>;

/// 自定义鉴权插件描述：名称、可选客户端插件要求与各阶段回调。
#[derive(Default)]
pub struct AuthPlugin {
    /// 插件注册名，须唯一且不得与默认鉴权插件冲突。
    pub Name: String,
    /// 要求客户端侧配合使用的插件名（可为空）。
    pub RequiredClientSidePlugin: String,
    /// 登录时校验用户身份的回调（必填）。
    pub AuthenticateUser: Option<AuthenticateUserFunc>,
    /// 生成持久化认证串的回调（必填）。
    pub GenerateAuthString: Option<GenerateAuthStringFunc>,
    /// 校验认证串格式的回调（必填）。
    pub ValidateAuthString: Option<ValidateAuthStringFunc>,
    /// 可选：静态权限校验钩子。
    pub VerifyPrivilege: Option<VerifyPrivilegeFunc>,
    /// 可选：动态权限校验钩子。
    pub VerifyDynamicPrivilege: Option<VerifyDynamicPrivilegeFunc>,
}

/// 一次认证请求携带的用户、盐值、输入串与连接态。
pub struct AuthenticateRequest {
    /// 待认证用户名。
    pub User: String,
    /// 服务端已存储的认证串（哈希/编码后的凭证）。
    pub StoredAuthString: String,
    /// 客户端本次提交的认证数据。
    pub InputAuthString: Vec<u8>,
    /// 握手盐值（salt），用于挑战-应答类协议。
    pub Salt: Vec<u8>,
    /// 可选 TLS 连接状态，供插件读取证书等信息。
    pub ConnState: Option<Arc<TlsConnectionState>>,
    /// 与客户端继续交换认证数据的连接。
    pub AuthConn: Box<dyn AuthConn>,
}

/// 静态权限校验请求：用户/主机/库表列与权限位。
#[derive(Clone)]
pub struct VerifyStaticPrivRequest {
    /// 用户名。
    pub User: String,
    /// 客户端主机。
    pub Host: String,
    /// 目标数据库名。
    pub DB: String,
    /// 目标表名。
    pub Table: String,
    /// 目标列名。
    pub Column: String,
    /// MySQL 静态权限位类型。
    pub StaticPriv: mysql::PrivilegeType,
    /// 可选 TLS 连接状态。
    pub ConnState: Option<Arc<TlsConnectionState>>,
    /// 当前会话激活的角色（Role）身份列表。
    pub ActiveRoles: Vec<Arc<auth_identity::RoleIdentity>>,
}

impl Default for VerifyStaticPrivRequest {
    fn default() -> Self {
        Self {
            User: String::new(),
            Host: String::new(),
            DB: String::new(),
            Table: String::new(),
            Column: String::new(),
            StaticPriv: mysql::PrivilegeType(0),
            ConnState: None,
            ActiveRoles: Vec::new(),
        }
    }
}

/// 动态权限校验请求：按权限名字符串判断是否具备特权。
#[derive(Clone, Default)]
pub struct VerifyDynamicPrivRequest {
    /// 用户名。
    pub User: String,
    /// 客户端主机。
    pub Host: String,
    /// 动态权限名称。
    pub DynamicPriv: String,
    /// 可选 TLS 连接状态。
    pub ConnState: Option<Arc<TlsConnectionState>>,
    /// 当前激活角色列表。
    pub ActiveRoles: Vec<Arc<auth_identity::RoleIdentity>>,
    /// 是否同时要求 GRANT OPTION（可转授该权限）。
    pub WithGrant: bool,
}

/// 批量校验鉴权插件：名称非空、唯一、非保留名，且三个核心回调均已设置。
pub fn validate_auth_plugins(plugins: &[Arc<AuthPlugin>]) -> Result<(), ExtensionError> {
    let mut names = HashSet::new();
    for plugin in plugins {
        // 名称必须非空。
        if plugin.Name.is_empty() {
            return Err(ExtensionError::new(format!(
                "auth plugin name cannot be empty for {}",
                plugin.Name
            )));
        }
        // 同一批次内禁止重复注册同名插件。
        if !names.insert(plugin.Name.as_str()) {
            return Err(ExtensionError::new(format!(
                "auth plugin name {} has already been registered",
                plugin.Name
            )));
        }
        // 禁止占用 MySQL 默认鉴权插件保留名。
        if mysql::DefaultAuthPlugins.contains(&plugin.Name.as_str()) {
            return Err(ExtensionError::new(format!(
                "auth plugin name {} is a reserved name for default auth plugins",
                plugin.Name
            )));
        }
        if plugin.AuthenticateUser.is_none() {
            return Err(ExtensionError::new(format!(
                "auth plugin AuthenticateUser function cannot be nil for {}",
                plugin.Name
            )));
        }
        if plugin.GenerateAuthString.is_none() {
            return Err(ExtensionError::new(format!(
                "auth plugin GenerateAuthString function cannot be nil for {}",
                plugin.Name
            )));
        }
        if plugin.ValidateAuthString.is_none() {
            return Err(ExtensionError::new(format!(
                "auth plugin ValidateAuthString function cannot be nil for {}",
                plugin.Name
            )));
        }
    }
    Ok(())
}

/// 校验 Manifest 中声明的鉴权插件列表（无插件时直接通过）。
pub(crate) fn validateAuthPlugin(manifest: &Manifest) -> Result<(), ExtensionError> {
    match &manifest.authPlugins {
        Some(plugins) => validate_auth_plugins(plugins),
        None => Ok(()),
    }
}
