// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// DDL 运行时统计信息导出。
//
// 将当前 DDL owner 的 server_id 与 schema_version（schema 版本号，
// 每次 schema 变更后递增，供各节点感知元数据变化）暴露为键值对，
// 供监控与运维接口查询。

use std::collections::BTreeMap;

use astersql_sessionctx_vardef::{ScopeFlag, ScopeGlobal, ScopeSession};

pub(crate) const SERVER_ID: &str = "server_id";
pub(crate) const DDL_SCHEMA_VERSION: &str = "ddl_schema_version";

/// Rust 中保留 Go `map[string]any` 的标量类型，避免将 schema version 字符串化。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StatusValue {
    String(String),
    I64(i64),
}

/// `ddl.sessPool` 与 `GetDDLInfoWithNewTxn` 的最小边界，便于保留错误与归还语义。
pub trait StatsSessionPool {
    type Session;
    type Error;

    fn get(&self) -> Result<Self::Session, Self::Error>;
    fn put(&self, session: Self::Session);
    fn schema_version(&self, session: &mut Self::Session) -> Result<i64, Self::Error>;
}

/// Go `ddl` 统计方法所需的状态。
pub struct DdlStatistics<P> {
    pub server_id: String,
    pub session_pool: P,
}

impl<P: StatsSessionPool> DdlStatistics<P> {
    /// 从 pool 获取会话，在新事务中读取 schema version，并无条件归还会话。
    pub fn stats(&self) -> Result<BTreeMap<String, StatusValue>, P::Error> {
        let mut session = self.session_pool.get()?;
        let schema_version = self.session_pool.schema_version(&mut session);
        self.session_pool.put(session);
        schema_version.map(|schema_version| {
            BTreeMap::from([
                (
                    SERVER_ID.into(),
                    StatusValue::String(self.server_id.clone()),
                ),
                (DDL_SCHEMA_VERSION.into(), StatusValue::I64(schema_version)),
            ])
        })
    }
}

/// DDL 统计快照：记录本节点身份与当前 schema 版本。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DdlStats {
    /// 本 DDL owner 所在节点的 server 标识。
    pub server_id: String,
    /// 当前已应用的 schema 版本号。
    pub schema_version: i64,
}
impl DdlStats {
    /// Go `variable.DefaultStatusVarScopeFlag`：同时对 global 与 session 可见。
    pub fn scope(&self, _status: &str) -> ScopeFlag {
        ScopeGlobal | ScopeSession
    }
    /// 导出可供展示的统计键值对（server_id、ddl_schema_version）。
    pub fn stats(&self) -> BTreeMap<String, StatusValue> {
        BTreeMap::from([
            (
                SERVER_ID.into(),
                StatusValue::String(self.server_id.clone()),
            ),
            (
                DDL_SCHEMA_VERSION.into(),
                StatusValue::I64(self.schema_version),
            ),
        ])
    }
}
