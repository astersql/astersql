// Copyright 2023-2023 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// LDAP Simple Bind 认证实现。
//
// Simple Bind 是 LDAP 最基础的认证方式：用完整 DN（Distinguished Name，
// 目录中唯一标识条目的路径）与明文密码向 LDAP 服务器发起绑定。
// 本模块对接 MySQL 兼容的 `authentication_ldap_simple` 插件路径，
// 负责把客户端传来的以 NUL 结尾的密码解码后完成一次 bind。

#![allow(non_snake_case, non_upper_case_globals)]

use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result, anyhow};

use crate::ldap_common::LdapAuthImpl;

/// LDAP Simple Bind 认证器，持有共享的 LDAP 连接/搜索实现。
pub struct LdapSimpleAuthImpl {
    /// 底层 LDAP 认证公共实现（连接池、用户搜索、DN 规范化等）。
    pub ldap: Arc<LdapAuthImpl>,
}

impl Default for LdapSimpleAuthImpl {
    fn default() -> Self {
        Self {
            ldap: Arc::new(LdapAuthImpl::default()),
        }
    }
}

impl LdapSimpleAuthImpl {
    /// 使用给定的 LDAP 公共实现构造 Simple Bind 认证器。
    pub fn new(ldap: Arc<LdapAuthImpl>) -> Self {
        Self { ldap }
    }

    /// 取出 MySQL 协议中以 NUL（`\0`）结尾的原始密码字节。
    ///
    /// 客户端认证报文通常在密码末尾附带一个 0 字节作为结束符；
    /// 与 Go 的 `string(password[:len-1])` 一样保留任意字节；LDAP Simple Bind
    /// 的密码字段是 OCTET STRING，并不要求 UTF-8。
    pub fn password_bytes(password: &[u8]) -> Result<&[u8]> {
        if password.last() != Some(&0) {
            return Err(anyhow!("invalid password"));
        }
        Ok(&password[..password.len() - 1])
    }

    /// 兼容只需要文本密码的调用方。
    pub fn password_string(password: &[u8]) -> Result<String> {
        String::from_utf8(Self::password_bytes(password)?.to_vec())
            .context("password is not valid UTF-8")
    }

    /// 执行 LDAP Simple Bind 认证。
    ///
    /// 流程：解码密码 → 解析/搜索用户 DN → 取连接 → `simple_bind`。
    /// 若 `dn` 为空则按用户名在目录中搜索；否则对传入 DN 做规范化（canonicalize）。
    pub fn AuthLDAPSimple(&self, user_name: &str, dn: &str, password: &[u8]) -> Result<()> {
        let password = Self::password_bytes(password)?;
        self.ldap.auth_simple(user_name, dn, password)
    }
}

/// 进程级默认 Simple Bind 认证器单例（对齐 Go 包级变量）。
pub static LDAPSimpleAuthImpl: LazyLock<LdapSimpleAuthImpl> =
    LazyLock::new(LdapSimpleAuthImpl::default);
