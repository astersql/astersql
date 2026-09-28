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

// LDAP SASL 认证：在 TiDB 连接与 LDAP 服务端之间转发多轮挑战响应。
//
// SASL（Simple Authentication and Security Layer）绑定可能需要多轮
// `AuthMoreData`/`AuthNextFactor` 交换；本模块循环调用 `SaslSession::server_bind_step`，
// 并通过 `AuthConn` 把服务端凭证写回客户端，直到 LDAP 返回成功（result_code == 0）。

#![allow(non_snake_case, non_upper_case_globals)]

use std::sync::{Arc, LazyLock, RwLock};

use anyhow::Result;

use crate::ldap_common::LdapAuthImpl;

/// TiDB 连接侧写回 AuthMoreData / 读下一包的能力。
/// The TiDB connection operations used by the LDAP SASL challenge loop.
pub trait AuthConn {
    fn write_auth_more_data(&mut self, data: &[u8]) -> Result<()>;
    fn flush(&mut self) -> Result<()>;
    fn read_packet(&mut self) -> Result<Vec<u8>>;
}

/// 单轮 LDAP SASL Bind 交换；由协议适配器提供实际 LDAP 线协议调用。
/// One LDAP SASL bind exchange. A protocol adapter supplies the LDAP wire operation.
pub trait SaslSession {
    fn server_bind_step(
        &mut self,
        client_cred: &[u8],
        dn: &str,
        method: &str,
    ) -> Result<(u32, Vec<u8>)>;
}

/// LDAP SASL 认证实现：持有共享 LDAP 配置与当前 SASL 方法名。
pub struct LdapSaslAuthImpl {
    /// 共享的 LDAP 连接/搜索实现。
    pub ldap: Arc<LdapAuthImpl>,
    /// 当前选用的 SASL 方法名（如 SCRAM-SHA-256）。
    sasl_auth_method: RwLock<String>,
}

impl LdapSaslAuthImpl {
    /// 使用给定 LDAP 实现与初始 SASL 方法构造。
    pub fn new(ldap: Arc<LdapAuthImpl>, method: impl Into<String>) -> Self {
        Self {
            ldap,
            sasl_auth_method: RwLock::new(method.into()),
        }
    }

    /// 在已知 DN 上执行 SASL 挑战循环直至成功。
    pub fn auth_with_session<S: SaslSession, C: AuthConn>(
        &self,
        dn: &str,
        mut client_cred: Vec<u8>,
        session: &mut S,
        auth_conn: &mut C,
    ) -> Result<()> {
        // 多轮挑战：每轮把 server_cred 写回客户端；成功码 0 时仍发送最后一轮凭证（对齐 Go）。
        let method = self.GetSASLAuthMethod();
        loop {
            let (result_code, server_cred) = session.server_bind_step(&client_cred, dn, &method)?;
            // Go deliberately sends the last server credential even when LDAP_SUCCESS is returned.
            auth_conn.write_auth_more_data(&server_cred)?;
            auth_conn.flush()?;
            if result_code == 0 {
                return Ok(());
            }
            client_cred = auth_conn.read_packet()?;
        }
    }

    /// 对外入口：必要时先搜索/规范化 DN，再进入 SASL 循环。
    pub fn AuthLDAPSASL<S: SaslSession, C: AuthConn>(
        &self,
        user_name: &str,
        dn: &str,
        client_cred: Vec<u8>,
        session: &mut S,
        auth_conn: &mut C,
    ) -> Result<()> {
        // DN 为空则按用户名搜索；否则按 `+suffix` 规则规范化。
        let dn = if dn.is_empty() {
            self.ldap.search_user(user_name)?
        } else {
            self.ldap.canonicalize_dn(user_name, dn)
        };
        self.auth_with_session(&dn, client_cred, session, auth_conn)
    }

    /// 设置当前 SASL 认证方法名。
    pub fn SetSASLAuthMethod(&self, method: impl Into<String>) {
        *self
            .sasl_auth_method
            .write()
            .expect("LDAP SASL lock poisoned") = method.into();
    }

    /// 读取当前 SASL 认证方法名。
    pub fn GetSASLAuthMethod(&self) -> String {
        self.sasl_auth_method
            .read()
            .expect("LDAP SASL lock poisoned")
            .clone()
    }
}

/// 默认空方法名 + 默认 LDAP 配置。
impl Default for LdapSaslAuthImpl {
    fn default() -> Self {
        Self::new(Arc::new(LdapAuthImpl::default()), "")
    }
}

/// 进程级默认 LDAP SASL 认证单例。
pub static LDAPSASLAuthImpl: LazyLock<LdapSaslAuthImpl> = LazyLock::new(LdapSaslAuthImpl::default);
