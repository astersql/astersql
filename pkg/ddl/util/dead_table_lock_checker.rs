// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 失效表锁（dead table lock）检测。
//
// 表锁（table lock）由持有会话的 TiDB 节点登记。当节点宕机后不再向 etcd
// 发布 schema version，其持有的表锁成为“死锁”——此处指持有者已失联的锁，
// 而非事务死锁。本模块对照 etcd 上存活节点集合，找出这类失效锁。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::{
    CancellationToken, DDLAllSchemaVersions, DdlUtilError, EtcdClient, SessionInfo, TableInfo,
    TableLockTpInfo,
};

/// 查询 etcd 存活节点的默认重试次数。
pub const DEFAULT_RETRY_COUNT: usize = 5;
/// 重试间隔默认值。
pub const DEFAULT_RETRY_INTERVAL: Duration = Duration::from_millis(200);
/// 默认超时（预留，与 Go 常量对齐）。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(1);

/// 单个库下带锁表的集合。
#[derive(Clone, Debug, Default)]
pub struct DatabaseTables {
    /// 该库中的表元信息列表。
    pub table_infos: Vec<TableInfo>,
}

/// 仅元数据视角的信息模式（info schema）抽象：列出持有表锁的表。
pub trait MetaOnlyInfoSchema {
    /// 返回所有带表锁的库表分组。
    fn list_tables_with_locks(&self) -> Vec<DatabaseTables>;
}

/// 内存实现的 MetaOnlyInfoSchema，用于测试与本地校验。
#[derive(Clone, Debug, Default)]
pub struct InMemoryInfoSchema {
    /// 内存中的库表列表。
    pub databases: Vec<DatabaseTables>,
}

/// 过滤出 `lock` 非空的表，按库归组返回。
impl MetaOnlyInfoSchema for InMemoryInfoSchema {
    fn list_tables_with_locks(&self) -> Vec<DatabaseTables> {
        self.databases
            .iter()
            .filter_map(|database| {
                // 仅保留持有表锁的表；空库不进入结果。
                let table_infos = database
                    .table_infos
                    .iter()
                    .filter(|table| table.lock.is_some())
                    .cloned()
                    .collect::<Vec<_>>();
                (!table_infos.is_empty()).then_some(DatabaseTables { table_infos })
            })
            .collect()
    }
}

/// 失效表锁检查器：对照 etcd 存活节点，收集持有者已离线的表锁。
/// Finds table locks whose owning TiDB server no longer publishes a schema version.
pub struct DeadTableLockChecker {
    /// 可选 etcd 客户端；为 `None` 时跳过检测。
    etcd_client: Option<Arc<EtcdClient>>,
    /// etcd 查询重试次数。
    retry_count: usize,
    /// 重试间隔。
    retry_interval: Duration,
}

/// 使用默认重试策略构造检查器。
pub fn NewDeadTableLockChecker(etcd_client: Option<Arc<EtcdClient>>) -> DeadTableLockChecker {
    DeadTableLockChecker {
        etcd_client,
        retry_count: DEFAULT_RETRY_COUNT,
        retry_interval: DEFAULT_RETRY_INTERVAL,
    }
}

impl DeadTableLockChecker {
    /// 覆盖重试次数与间隔。
    pub fn with_retry_policy(mut self, retry_count: usize, retry_interval: Duration) -> Self {
        self.retry_count = retry_count;
        self.retry_interval = retry_interval;
        self
    }

    /// 从 etcd 前缀 `DDLAllSchemaVersions` 解析仍在发布版本的 server_id 集合。
    fn get_alive_servers(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<HashSet<String>, DdlUtilError> {
        let client = self
            .etcd_client
            .as_ref()
            .expect("caller checks the optional etcd client");
        let mut last_error = DdlUtilError::Etcd("get was not attempted".to_owned());
        // 带取消检查的重试：成功则解析 key 后缀为 server_id。
        for _ in 0..self.retry_count {
            cancellation.check()?;
            match client.get(DDLAllSchemaVersions, true) {
                Ok(values) => {
                    let server_prefix = format!("{DDLAllSchemaVersions}/");
                    return Ok(values
                        .into_iter()
                        .map(|value| {
                            // key 形如 /tidb/ddl/all_schema_versions/<server_id>。
                            value
                                .key
                                .strip_prefix(&server_prefix)
                                .unwrap_or(&value.key)
                                .to_owned()
                        })
                        .collect());
                }
                Err(error) => {
                    last_error = error;
                    if !self.retry_interval.is_zero() {
                        thread::sleep(self.retry_interval);
                    }
                }
            }
        }
        Err(last_error)
    }

    /// 返回“会话 → 其持有的失效表锁列表”映射。
    pub fn GetDeadLockedTables(
        &self,
        cancellation: &CancellationToken,
        info_schema: &dyn MetaOnlyInfoSchema,
    ) -> Result<HashMap<SessionInfo, Vec<TableLockTpInfo>>, DdlUtilError> {
        // 无 etcd 时无法判定存活节点，返回空结果。
        if self.etcd_client.is_none() {
            return Ok(HashMap::new());
        }
        let alive_servers = self.get_alive_servers(cancellation)?;
        let mut dead_locks: HashMap<SessionInfo, Vec<TableLockTpInfo>> = HashMap::new();

        for database in info_schema.list_tables_with_locks() {
            for table in database.table_infos {
                let Some(lock) = &table.lock else {
                    continue;
                };
                // 持有会话的 server_id 不在存活集合中，则记为失效锁。
                for session in &lock.sessions {
                    if !alive_servers.contains(&session.server_id) {
                        dead_locks
                            .entry(session.clone())
                            .or_default()
                            .push(TableLockTpInfo {
                                schema_id: table.db_id,
                                table_id: table.id,
                                lock_type: lock.lock_type.clone(),
                            });
                    }
                }
            }
        }
        Ok(dead_locks)
    }
}
