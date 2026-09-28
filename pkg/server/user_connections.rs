// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 按用户维度统计与限制 MySQL 客户端连接数。
//
// 对应 Go `user_connections.go`：在全局上限与用户级 `MAX_USER_CONNECTIONS` 之间取有效限额，
// 于建连前检查、建连后递增、断开时递减连接计数。

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

/// MySQL 用户身份：登录名/主机与鉴权后实际匹配的账号。
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct UserIdentity {
    /// 客户端登录时提交的用户名。
    pub username: String,
    /// 客户端登录时使用的主机名或地址。
    pub hostname: String,
    /// 权限系统匹配后的鉴权用户名。
    pub auth_username: String,
    /// 权限系统匹配后的鉴权主机模式（可为 `%`）。
    pub auth_hostname: String,
}

impl UserIdentity {
    /// 返回鉴权身份字符串 `auth_user@auth_host`，用作连接计数键。
    pub fn String(&self) -> String {
        if self.auth_username.is_empty() {
            format!("{}@{}", self.username, self.hostname)
        } else {
            format!("{}@{}", self.auth_username, self.auth_hostname)
        }
    }

    /// 返回客户端登录串 `user@host`，用于超限日志。
    pub fn LoginString(&self) -> String {
        format!("{}@{}", self.username, self.hostname)
    }
}

/// 用户连接计数相关错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectionError {
    /// 资源或运行时配置错误。
    Resource(String),
    /// 超过该用户允许的最大连接数。
    TooManyUserConnections(String),
    /// 连接计数 Mutex 被毒化。
    LockPoisoned,
}

impl fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resource(message) => f.write_str(message),
            Self::TooManyUserConnections(user) => {
                write!(f, "too many user connections for {user}")
            }
            Self::LockPoisoned => f.write_str("user connection counter lock poisoned"),
        }
    }
}

impl std::error::Error for ConnectionError {}

/// 连接限额与身份匹配所需的运行时依赖（全局上限、用户上限、身份解析、超限日志）。
pub trait UserConnectionRuntime: Send + Sync {
    /// 返回全局最大连接数；为 0 表示不限制。
    fn global_limit(&self) -> u32;
    /// 查询指定账号的用户级连接上限（如 `MAX_USER_CONNECTIONS`）。
    fn user_limit(&self, username: &str, hostname: &str) -> Result<u32, ConnectionError>;
    /// 将客户端提交的身份与连接主机匹配为权限系统中的真实账号。
    fn match_identity(
        &self,
        presented: &UserIdentity,
        host: &str,
    ) -> Result<UserIdentity, ConnectionError>;
    /// 记录用户连接数超限事件。
    fn log_limit_exceeded(&self, user: &str, limit: u32);
}

/// 跨连接共享的用户连接计数注册表（键为 `auth_user@auth_host`）。
#[derive(Clone, Default)]
pub struct UserConnectionRegistry {
    counts: Arc<Mutex<HashMap<String, usize>>>,
}

/// 客户端连接上下文：身份、运行时依赖与共享计数注册表。
pub struct ClientConn {
    /// 当前连接的用户身份。
    pub user: UserIdentity,
    /// 限额与身份匹配运行时。
    pub runtime: Arc<dyn UserConnectionRuntime>,
    /// 共享的用户连接计数。
    pub registry: UserConnectionRegistry,
}

/// 计算有效连接上限：用户级限额优先，否则使用全局限额；均为 0 表示不限。
pub fn calculateConnectionLimit(global_limit: u32, user_limit: u32) -> u32 {
    if user_limit > 0 {
        user_limit
    } else {
        global_limit
    }
}

/// 建连成功后递增该用户连接数；若已达上限则返回 `TooManyUserConnections`。
pub fn increaseUserConnectionsCount(cc: &ClientConn) -> Result<(), ConnectionError> {
    let target_user = cc.user.String();
    let user_limit = cc
        .runtime
        .user_limit(&cc.user.auth_username, &cc.user.auth_hostname)?;
    let limit = calculateConnectionLimit(cc.runtime.global_limit(), user_limit);
    let mut counts = cc
        .registry
        .counts
        .lock()
        .map_err(|_| ConnectionError::LockPoisoned)?;
    let count = counts.entry(target_user.clone()).or_insert(0);
    // 限额大于 0 且当前计数已达上限时拒绝再建连。
    if limit > 0 && *count >= limit as usize {
        return Err(ConnectionError::TooManyUserConnections(target_user));
    }
    *count += 1;
    Ok(())
}

/// 连接关闭时递减计数；减至 0 则从注册表移除该用户条目。
pub fn decreaseUserConnectionCount(cc: &ClientConn) -> Result<(), ConnectionError> {
    let target_user = cc.user.String();
    let mut counts = cc
        .registry
        .counts
        .lock()
        .map_err(|_| ConnectionError::LockPoisoned)?;
    if let Some(count) = counts.get_mut(&target_user) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            counts.remove(&target_user);
        }
    }
    Ok(())
}

/// 查询指定用户当前已建立的连接数。
pub fn getUserConnectionCount(
    cc: &ClientConn,
    user: &UserIdentity,
) -> Result<usize, ConnectionError> {
    let counts = cc
        .registry
        .counts
        .lock()
        .map_err(|_| ConnectionError::LockPoisoned)?;
    Ok(counts.get(&user.String()).copied().unwrap_or(0))
}

/// 建连前预检：匹配身份后若已达有效上限则打日志并返回超限错误。
pub fn checkUserConnectionCount(cc: &ClientConn, host: &str) -> Result<(), ConnectionError> {
    let auth_user = cc.runtime.match_identity(&cc.user, host)?;
    let user_limit = cc
        .runtime
        .user_limit(&auth_user.username, &auth_user.hostname)?;
    let limit = calculateConnectionLimit(cc.runtime.global_limit(), user_limit);
    // limit == 0 表示全局与用户级均不限制。
    if limit == 0 {
        return Ok(());
    }
    if getUserConnectionCount(cc, &auth_user)? >= limit as usize {
        let login = auth_user.LoginString();
        cc.runtime.log_limit_exceeded(&login, limit);
        return Err(ConnectionError::TooManyUserConnections(login));
    }
    Ok(())
}
