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

// LDAP 公共层：配置、连接池、TLS/StartTLS 与用户 DN 搜索。
//
// `LdapAuthImpl` 以读写锁保护配置与 r2d2 连接池；主机/端口/TLS/容量变更会重建池。
// DN 规范化支持完整 DN 与 `+base` 前缀语法（拼接 search_attr=user）。

#![allow(non_snake_case)]

use std::fmt;
use std::fs;
use std::sync::RwLock;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use ldap3::{LdapConn, LdapConnSettings, Scope, SearchEntry};
use native_tls::{Certificate, Protocol, TlsConnector};
use r2d2::{ManageConnection, Pool, PooledConnection};

/// LDAP 连接与操作超时（10 秒）。
pub const LDAP_TIMEOUT: Duration = Duration::from_secs(10);
/// 从连接池取连接的最大重试次数。
pub const GET_CONNECTION_MAX_RETRY: usize = 10;
/// 取连接失败后的重试间隔。
pub const GET_CONNECTION_RETRY_INTERVAL: Duration = Duration::from_millis(500);

/// Build the LDAP search filter exactly as the Go implementation does.
pub(crate) fn search_filter(search_attr: &str, user_name: &str) -> String {
    format!("({search_attr}={user_name})")
}

#[derive(Clone, Default)]
/// LDAP 服务端与绑定/搜索相关配置。
pub struct LdapConfig {
    /// 搜索用户的 Base DN。
    pub bind_base_dn: String,
    /// 用于搜索的管理账户 DN。
    pub bind_root_dn: String,
    /// 管理账户密码。
    pub bind_root_pwd: String,
    /// 搜索用户名时使用的属性（如 `cn`/`uid`）。
    pub search_attr: String,
    /// LDAP 服务器主机名或 IP。
    pub ldap_server_host: String,
    /// LDAP 服务器端口。
    pub ldap_server_port: u16,
    /// 是否启用 TLS（StartTLS 或 LDAPS）。
    pub enable_tls: bool,
    /// CA 证书文件路径。
    pub ca_path: String,
    /// 已加载的 CA PEM 字节（可选）。
    pub ca_pem: Option<Vec<u8>>,
    /// 连接池初始/最小空闲容量。
    pub init_capacity: u32,
    /// 连接池最大容量。
    pub max_capacity: u32,
}

/// 运行时状态：配置、连接池与重建世代号。
struct LdapState {
    config: LdapConfig,
    pool: Option<Pool<LdapConnectionManager>>,
    pool_generation: u64,
}

impl Default for LdapState {
    fn default() -> Self {
        Self {
            config: LdapConfig::default(),
            pool: None,
            pool_generation: 0,
        }
    }
}

#[derive(Clone)]
/// r2d2 连接管理器：按配置建立并校验 LDAP 连接。
pub struct LdapConnectionManager {
    pub config: LdapConfig,
}

#[derive(Debug)]
/// 连接池错误包装，满足 r2d2 `Error` 约束。
pub struct LdapPoolError(anyhow::Error);

impl fmt::Display for LdapPoolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for LdapPoolError {}

impl LdapConnectionManager {
    /// 构建最低 TLS1.2 的连接器，可选挂载自定义 CA。
    fn tls_connector(&self) -> Result<TlsConnector> {
        let mut builder = TlsConnector::builder();
        builder.min_protocol_version(Some(Protocol::Tlsv12));
        if let Some(pem) = &self.config.ca_pem {
            builder.add_root_certificate(
                Certificate::from_pem(pem).context("fail to parse ca certificate")?,
            );
        }
        builder.build().context("build LDAP TLS connector")
    }

    /// 格式化 `host:port`；IPv6 主机名加方括号。
    fn address(&self) -> String {
        if self.config.ldap_server_host.contains(':') {
            format!(
                "[{}]:{}",
                self.config.ldap_server_host, self.config.ldap_server_port
            )
        } else {
            format!(
                "{}:{}",
                self.config.ldap_server_host, self.config.ldap_server_port
            )
        }
    }

    /// 按 URL 建立 LDAP 连接（可选 StartTLS）。
    pub fn connect_url(&self, url: &str, starttls: bool) -> Result<LdapConn> {
        let settings = LdapConnSettings::new()
            .set_conn_timeout(LDAP_TIMEOUT)
            .set_starttls(starttls)
            .set_connector(self.tls_connector()?);
        let mut conn = LdapConn::with_settings(settings, url)
            .with_context(|| format!("connect to LDAP at {url}"))?;
        conn.with_timeout(LDAP_TIMEOUT);
        Ok(conn)
    }

    /// 按配置连接：明文，或先 StartTLS 再回退 LDAPS。
    pub fn connect_ldap(&self) -> Result<LdapConn> {
        let address = self.address();
        // 未启用 TLS 走明文；启用时优先 StartTLS，失败再尝试 ldaps://。
        if !self.config.enable_tls {
            return self.connect_url(&format!("ldap://{address}"), false);
        }

        self.connect_url(&format!("ldap://{address}"), true)
            .or_else(|starttls_err| {
                self.connect_url(&format!("ldaps://{address}"), false).map_err(|tls_err| {
                anyhow!("create ldap connection: {tls_err}; StartTLS failed first: {starttls_err}")
            })
            })
    }
}

impl ManageConnection for LdapConnectionManager {
    type Connection = LdapConn;
    type Error = LdapPoolError;

    /// 池创建连接：调用 `connect_ldap`。
    fn connect(&self) -> std::result::Result<Self::Connection, Self::Error> {
        self.connect_ldap().map_err(LdapPoolError)
    }

    /// 用 root DN 做 simple_bind 校验连接可用性。
    fn is_valid(&self, conn: &mut Self::Connection) -> std::result::Result<(), Self::Error> {
        let result: Result<()> = (|| {
            conn.simple_bind(&self.config.bind_root_dn, &self.config.bind_root_pwd)
                .context("bind root dn to validate LDAP connection")?
                .success()
                .context("bind root dn to validate LDAP connection")?;
            Ok(())
        })();
        result.map_err(LdapPoolError)
    }

    /// 不主动判定连接损坏（交由校验逻辑）。
    fn has_broken(&self, _conn: &mut Self::Connection) -> bool {
        false
    }
}

/// LDAP 认证实现门面：配置 CRUD、连接池与用户搜索。
pub struct LdapAuthImpl {
    state: RwLock<LdapState>,
}

impl Default for LdapAuthImpl {
    fn default() -> Self {
        Self {
            state: RwLock::new(LdapState::default()),
        }
    }
}

impl LdapAuthImpl {
    fn get_connection_from_pool(
        pool: &Pool<LdapConnectionManager>,
    ) -> Result<PooledConnection<LdapConnectionManager>> {
        let mut last_error = None;
        for attempt in 0..GET_CONNECTION_MAX_RETRY {
            match pool.get() {
                Ok(conn) => return Ok(conn),
                Err(err) => {
                    last_error = Some(err);
                    if attempt + 1 < GET_CONNECTION_MAX_RETRY {
                        std::thread::sleep(GET_CONNECTION_RETRY_INTERVAL);
                    }
                }
            }
        }
        Err(anyhow!(
            "fail to bind to anonymous user: {}",
            last_error.expect("retry loop ran")
        ))
    }

    /// 在容量合法时重建连接池并递增世代号。
    fn rebuild_pool(state: &mut LdapState) {
        let config = &state.config;
        if config.init_capacity == 0 || config.max_capacity < config.init_capacity {
            return;
        }
        let manager = LdapConnectionManager {
            config: config.clone(),
        };
        state.pool = Some(
            Pool::builder()
                .max_size(config.max_capacity)
                .min_idle(Some(config.init_capacity))
                .connection_timeout(LDAP_TIMEOUT)
                .test_on_check_out(true)
                .build_unchecked(manager),
        );
        state.pool_generation += 1;
    }

    /// 当前连接池重建世代（用于测试断言）。
    pub fn pool_generation(&self) -> u64 {
        self.state
            .read()
            .expect("LDAP state lock poisoned")
            .pool_generation
    }

    /// 规范化 DN：`+suffix` 前缀时拼成 `attr=user,suffix`。
    pub fn canonicalize_dn(&self, user_name: &str, dn: &str) -> String {
        if let Some(suffix) = dn.strip_prefix('+') {
            let attr = self.GetSearchAttr();
            return format!("{attr}={user_name},{suffix}");
        }
        dn.to_owned()
    }

    /// 以 root 身份按 search_attr 搜索用户 DN。
    pub fn search_user(&self, user_name: &str) -> Result<String> {
        let (base, attr, root_dn, root_pwd) = {
            let state = self.state.read().expect("LDAP state lock poisoned");
            (
                state.config.bind_base_dn.clone(),
                state.config.search_attr.clone(),
                state.config.bind_root_dn.clone(),
                state.config.bind_root_pwd.clone(),
            )
        };
        let mut conn = self.get_connection()?;
        conn.simple_bind(&root_dn, &root_pwd)
            .context("bind root dn to search user")?
            .success()
            .context("bind root dn to search user")?;
        let filter = search_filter(&attr, user_name);
        let (entries, _) = conn
            .search(&base, Scope::Subtree, &filter, vec!["dn"])
            .context("search LDAP user")?
            .success()
            .context("search LDAP user")?;
        entries
            .into_iter()
            .next()
            .map(|entry| SearchEntry::construct(entry).dn)
            .ok_or_else(|| anyhow!("LDAP user not found"))
    }

    /// 从池中获取连接，失败时按间隔重试。
    pub fn get_connection(&self) -> Result<PooledConnection<LdapConnectionManager>> {
        let pool = self
            .state
            .read()
            .expect("LDAP state lock poisoned")
            .pool
            .clone()
            .ok_or_else(|| anyhow!("LDAP connection pool is not initialized"))?;

        Self::get_connection_from_pool(&pool)
    }

    /// Execute the complete Simple Bind flow under one configuration read lock.
    /// This matches Go's `RLock` around search/canonicalization and final bind.
    pub(crate) fn auth_simple(&self, user_name: &str, dn: &str, password: &[u8]) -> Result<()> {
        let state = self.state.read().expect("LDAP state lock poisoned");
        let pool = state.pool.as_ref();

        let bind_dn = if dn.is_empty() {
            let pool = pool.ok_or_else(|| anyhow!("LDAP connection pool is not initialized"))?;
            let mut conn = Self::get_connection_from_pool(pool)?;
            conn.simple_bind(&state.config.bind_root_dn, &state.config.bind_root_pwd)
                .context("bind root dn to search user")?
                .success()
                .context("bind root dn to search user")?;
            let filter = search_filter(&state.config.search_attr, user_name);
            let (entries, _) = conn
                .search(
                    &state.config.bind_base_dn,
                    Scope::Subtree,
                    &filter,
                    vec!["dn"],
                )
                .context("search LDAP user")?
                .success()
                .context("search LDAP user")?;
            entries
                .into_iter()
                .next()
                .map(|entry| SearchEntry::construct(entry).dn)
                .ok_or_else(|| anyhow!("LDAP user not found"))?
        } else if let Some(suffix) = dn.strip_prefix('+') {
            format!("{}={user_name},{suffix}", state.config.search_attr)
        } else {
            dn.to_owned()
        };

        let pool = pool.ok_or_else(|| anyhow!("LDAP connection pool is not initialized"))?;
        let mut conn = Self::get_connection_from_pool(pool).context("create LDAP connection")?;
        conn.simple_bind_bytes(&bind_dn, password)
            .context("bind LDAP")?
            .success()
            .context("bind LDAP")?;
        Ok(())
    }

    /// 设置搜索 Base DN。
    pub fn SetBindBaseDN(&self, value: impl Into<String>) {
        self.state
            .write()
            .expect("LDAP state lock poisoned")
            .config
            .bind_base_dn = value.into();
    }
    /// 设置管理账户 DN。
    pub fn SetBindRootDN(&self, value: impl Into<String>) {
        self.state
            .write()
            .expect("LDAP state lock poisoned")
            .config
            .bind_root_dn = value.into();
    }
    /// 设置管理账户密码。
    pub fn SetBindRootPW(&self, value: impl Into<String>) {
        self.state
            .write()
            .expect("LDAP state lock poisoned")
            .config
            .bind_root_pwd = value.into();
    }
    /// 设置用户搜索属性名。
    pub fn SetSearchAttr(&self, value: impl Into<String>) {
        self.state
            .write()
            .expect("LDAP state lock poisoned")
            .config
            .search_attr = value.into();
    }
    /// 设置服务器主机；变更时重建连接池。
    pub fn SetLDAPServerHost(&self, value: impl Into<String>) {
        let value = value.into();
        let mut state = self.state.write().expect("LDAP state lock poisoned");
        if value != state.config.ldap_server_host {
            state.config.ldap_server_host = value;
            Self::rebuild_pool(&mut state);
        }
    }
    /// 设置服务器端口；变更时重建连接池。
    pub fn SetLDAPServerPort(&self, value: u16) {
        let mut state = self.state.write().expect("LDAP state lock poisoned");
        if value != state.config.ldap_server_port {
            state.config.ldap_server_port = value;
            Self::rebuild_pool(&mut state);
        }
    }
    /// 设置是否启用 TLS；变更时重建连接池。
    pub fn SetEnableTLS(&self, value: bool) {
        let mut state = self.state.write().expect("LDAP state lock poisoned");
        if value != state.config.enable_tls {
            state.config.enable_tls = value;
            Self::rebuild_pool(&mut state);
        }
    }
    /// 加载或清空 CA 证书路径。
    pub fn SetCAPath(&self, path: impl Into<String>) -> Result<()> {
        let path = path.into();
        let mut state = self.state.write().expect("LDAP state lock poisoned");
        if path == state.config.ca_path {
            return Ok(());
        }
        state.config.ca_path = path.clone();
        state.config.ca_pem = None;
        state.config.ca_pem = if path.is_empty() {
            None
        } else {
            let pem = fs::read(&path).with_context(|| format!("read ca certificate at {path}"))?;
            Certificate::from_pem(&pem).context("fail to parse ca certificate")?;
            Some(pem)
        };
        Ok(())
    }
    /// 设置池初始容量；变更时尝试重建。
    pub fn SetInitCapacity(&self, value: u32) {
        let mut state = self.state.write().expect("LDAP state lock poisoned");
        if value != state.config.init_capacity {
            state.config.init_capacity = value;
            Self::rebuild_pool(&mut state);
        }
    }
    /// 设置池最大容量；变更时尝试重建。
    pub fn SetMaxCapacity(&self, value: u32) {
        let mut state = self.state.write().expect("LDAP state lock poisoned");
        if value != state.config.max_capacity {
            state.config.max_capacity = value;
            Self::rebuild_pool(&mut state);
        }
    }

    /// 读取搜索 Base DN。
    pub fn GetBindBaseDN(&self) -> String {
        self.state
            .read()
            .expect("LDAP state lock poisoned")
            .config
            .bind_base_dn
            .clone()
    }
    /// 读取管理账户 DN。
    pub fn GetBindRootDN(&self) -> String {
        self.state
            .read()
            .expect("LDAP state lock poisoned")
            .config
            .bind_root_dn
            .clone()
    }
    /// 读取管理账户密码。
    pub fn GetBindRootPW(&self) -> String {
        self.state
            .read()
            .expect("LDAP state lock poisoned")
            .config
            .bind_root_pwd
            .clone()
    }
    /// 读取用户搜索属性名。
    pub fn GetSearchAttr(&self) -> String {
        self.state
            .read()
            .expect("LDAP state lock poisoned")
            .config
            .search_attr
            .clone()
    }
    /// 读取服务器主机。
    pub fn GetLDAPServerHost(&self) -> String {
        self.state
            .read()
            .expect("LDAP state lock poisoned")
            .config
            .ldap_server_host
            .clone()
    }
    /// 读取服务器端口。
    pub fn GetLDAPServerPort(&self) -> u16 {
        self.state
            .read()
            .expect("LDAP state lock poisoned")
            .config
            .ldap_server_port
    }
    /// 读取是否启用 TLS。
    pub fn GetEnableTLS(&self) -> bool {
        self.state
            .read()
            .expect("LDAP state lock poisoned")
            .config
            .enable_tls
    }
    /// 读取 CA 证书路径。
    pub fn GetCAPath(&self) -> String {
        self.state
            .read()
            .expect("LDAP state lock poisoned")
            .config
            .ca_path
            .clone()
    }
    /// 读取池初始容量。
    pub fn GetInitCapacity(&self) -> u32 {
        self.state
            .read()
            .expect("LDAP state lock poisoned")
            .config
            .init_capacity
    }
    /// 读取池最大容量。
    pub fn GetMaxCapacity(&self) -> u32 {
        self.state
            .read()
            .expect("LDAP state lock poisoned")
            .config
            .max_capacity
    }
}
