// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// InfoSchema 内部测试工具包（testkit）。
//
// 提供 mock 存储上的元数据夹具：库/表/资源组/Placement Policy 的增删改，
// 以及 AutoID 需求对象与全局 ID 生成。写操作经 `TestStore::transaction` 做快照回滚，
// 失败时恢复元数据状态。

use crate::mockstore::{MockStorage, MockTiKVStoreOption, NewMockStore};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// 在给定路径写入样例慢查询日志内容，供慢日志解析测试使用。
pub fn PrepareSlowLogfile(path: impl AsRef<Path>) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(SLOW_LOG_SAMPLE.as_bytes())?;
    file.flush()
}

/// 两段样例慢日志文本（含事务、Coprocessor、资源组等字段），与 Go 测试夹具对齐。
pub const SLOW_LOG_SAMPLE: &str = r#"# Time: 2019-02-12T19:33:56.571953+08:00
# Txn_start_ts: 406315658548871171
# User@Host: root[root] @ localhost [127.0.0.1]
# Conn_ID: 6
# Exec_retry_time: 0.12 Exec_retry_count: 57
# Query_time: 4.895492
# Parse_time: 0.4
# Compile_time: 0.2
# Rewrite_time: 0.000000003 Preproc_subqueries: 2 Preproc_subqueries_time: 0.000000002
# Optimize_time: 0.00000001
# Wait_TS: 0.000000003
# LockKeys_time: 1.71 Request_count: 1 Prewrite_time: 0.19 Wait_prewrite_binlog_time: 0.21 Commit_time: 0.01 Commit_backoff_time: 0.18 Backoff_types: [txnLock] Resolve_lock_time: 0.03 Write_keys: 15 Write_size: 480 Prewrite_region: 1 Txn_retry: 8
# Cop_time: 0.3824278 Process_time: 0.161 Request_count: 1 Total_keys: 100001 Process_keys: 100000
# Rocksdb_delete_skipped_count: 100 Rocksdb_key_skipped_count: 10 Rocksdb_block_cache_hit_count: 10 Rocksdb_block_read_count: 10 Rocksdb_block_read_byte: 100
# Wait_time: 0.101
# Backoff_time: 0.092
# DB: test
# Is_internal: false
# Digest: 42a1c8aae6f133e934d4bf0147491709a8812ea05ff8819ec522780fe657b772
# Stats: t1:1,t2:2
# Cop_proc_avg: 0.1 Cop_proc_p90: 0.2 Cop_proc_max: 0.03 Cop_proc_addr: 127.0.0.1:20160
# Cop_wait_avg: 0.05 Cop_wait_p90: 0.6 Cop_wait_max: 0.8 Cop_wait_addr: 0.0.0.0:20160
# Mem_max: 70724
# Mem_arbitration: 23333
# Disk_max: 65536
# Plan_from_cache: true
# Result_rows: 10
# Succ: true
# Plan: abcd
# Plan_digest: 60e9378c746d9a2be1c791047e008967cf252eb6de9167ad3aa6098fa2d523f4
# Prev_stmt: update t set i = 2;
# Resource_group: default
select * from t_slim;
# Time: 2021-09-08T14:39:54.506967433+08:00
# Txn_start_ts: 427578666238083075
# User@Host: root[root] @ 172.16.0.0 [172.16.0.0]
# Conn_ID: 40507
# Session_alias: alias123
# Query_time: 25.571605962
# Parse_time: 0.002923536
# Compile_time: 0.006800973
# Rewrite_time: 0.002100764
# Optimize_time: 0
# Wait_TS: 0.000015801
# Prewrite_time: 25.542014572 Commit_time: 0.002294647 Get_commit_ts_time: 0.000605473 Commit_backoff_time: 12.483 Backoff_types: [tikvRPC regionMiss tikvRPC regionMiss regionMiss] Write_keys: 624 Write_size: 172064 Prewrite_region: 60
# DB: rtdb
# Is_internal: false
# Digest: 124acb3a0bec903176baca5f9da00b4e7512a41c93b417923f26502edeb324cc
# Num_cop_tasks: 0
# Mem_max: 856544
# Mem_arbitration: 856547
# Prepared: false
# Plan_from_cache: false
# Plan_from_binding: false
# Has_more_results: false
# KV_total: 86.635049185
# PD_total: 0.015486658
# Backoff_total: 100.054
# Unpacked_bytes_sent_tikv_total: 30000
# Unpacked_bytes_received_tikv_total: 3000
# Unpacked_bytes_sent_tikv_cross_zone: 10000
# Unpacked_bytes_received_tikv_cross_zone: 1000
# Unpacked_bytes_sent_tiflash_total: 500000
# Unpacked_bytes_received_tiflash_total: 500005
# Unpacked_bytes_sent_tiflash_cross_zone: 300000
# Unpacked_bytes_received_tiflash_cross_zone: 300005
# Write_sql_response_total: 0
# Succ: true
# Resource_group: rg1
# Request_unit_read: 96.66703066666668
# Request_unit_write: 3182.424414062492
# Tidb_cpu_time: 0.01
# Tikv_cpu_time: 0.021
# Storage_from_kv: true
# Storage_from_mpp: true
INSERT INTO ...;
"#;

/// 大小写不敏感字符串：保留原文与小写形式，用于库表名比较。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CiString {
    pub original: String,
    pub lower: String,
}

impl CiString {
    /// 由原文构造，同时缓存小写形式。
    pub fn new(value: impl Into<String>) -> Self {
        let original = value.into();
        let lower = original.to_lowercase();
        Self { original, lower }
    }
}

/// Schema 对象可见性状态（对应 Go `model.SchemaState`）；Public 表示已对用户可见。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SchemaState {
    #[default]
    None,
    Public,
}

/// 列元数据的测试简化版。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ColumnInfo {
    pub id: i64,
    pub name: CiString,
    pub offset: usize,
    pub field_type: FieldType,
    pub state: SchemaState,
}

/// 列字段类型枚举的测试简化版。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FieldType {
    #[default]
    LongLong,
}

/// 表元数据的测试简化版。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableInfo {
    pub id: i64,
    pub name: CiString,
    pub columns: Vec<ColumnInfo>,
    pub state: SchemaState,
    pub revision: i64,
}

/// 库元数据的测试简化版。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DbInfo {
    pub id: i64,
    pub name: CiString,
    pub state: SchemaState,
    pub deprecated_tables: Vec<TableInfo>,
}

/// 资源组（Resource Group）元数据：用于配额与调度隔离。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceGroupInfo {
    pub id: i64,
    pub name: CiString,
}

/// Placement Policy（副本放置策略）元数据。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PolicyInfo {
    pub id: i64,
    pub name: CiString,
}

/// 对 Placement Policy 的引用信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PolicyRefInfo {
    pub id: i64,
    pub name: CiString,
}

/// 表运行时句柄的测试简化版，仅持有元数据。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Table {
    pub meta: TableInfo,
}

/// TestStore 内部可变元数据：全局 ID、库表、资源组与策略。
#[derive(Clone, Default)]
struct MetadataState {
    global_id: i64,
    databases: HashMap<i64, DbInfo>,
    tables: HashMap<(i64, i64), TableInfo>,
    resource_groups: HashMap<i64, ResourceGroupInfo>,
    policies: HashMap<i64, PolicyInfo>,
}

/// 内层存储：MockStorage + 受 Mutex 保护的元数据。
struct TestStoreInner {
    storage: MockStorage,
    metadata: Mutex<MetadataState>,
}

/// 可克隆的测试存储句柄，封装 mock TiKV 与内存元数据。
#[derive(Clone)]
pub struct TestStore(Arc<TestStoreInner>);

impl TestStore {
    /// 按 mockstore 选项创建 TestStore。
    pub fn new(options: Vec<MockTiKVStoreOption>) -> Result<Self, TestkitError> {
        let storage = NewMockStore(options).map_err(|error| TestkitError(error.to_string()))?;
        Ok(Self(Arc::new(TestStoreInner {
            storage,
            metadata: Mutex::new(MetadataState::default()),
        })))
    }

    /// 返回底层 MockStorage 引用。
    pub fn storage(&self) -> &MockStorage {
        &self.0.storage
    }

    /// 在元数据锁上执行操作；失败时回滚到进入前的快照。
    fn transaction<T>(
        &self,
        operation: impl FnOnce(&mut MetadataState) -> Result<T, TestkitError>,
    ) -> Result<T, TestkitError> {
        let mut state = self.0.metadata.lock().expect("metadata lock poisoned");
        // 先克隆快照，失败时整体写回以实现简易事务语义。
        let snapshot = state.clone();
        match operation(&mut state) {
            Ok(value) => Ok(value),
            Err(error) => {
                *state = snapshot;
                Err(error)
            }
        }
    }
}

/// testkit 操作错误，携带可读消息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestkitError(pub String);
impl std::fmt::Display for TestkitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for TestkitError {}

/// AutoID（自增 ID）分配所需的存储依赖接口。
pub trait AutoIdRequirement {
    /// 返回关联的 TestStore。
    fn store(&self) -> &TestStore;
    /// 可选的远端 AutoID 客户端；测试桩默认无。
    fn auto_id_client(&self) -> Option<()> {
        None
    }
}

/// AutoIdRequirement 的测试实现，仅持有 TestStore。
pub struct MockAutoIdRequirement {
    store: TestStore,
}

impl AutoIdRequirement for MockAutoIdRequirement {
    fn store(&self) -> &TestStore {
        &self.store
    }
}

/// 用给定 mockstore 选项创建 AutoID 需求对象。
pub fn CreateAutoIDRequirement(
    options: Vec<MockTiKVStoreOption>,
) -> Result<MockAutoIdRequirement, TestkitError> {
    Ok(MockAutoIdRequirement {
        store: TestStore::new(options)?,
    })
}

/// 用已有 TestStore 包装为 AutoID 需求对象。
pub fn CreateAutoIDRequirementWithStore(store: TestStore) -> MockAutoIdRequirement {
    MockAutoIdRequirement { store }
}

/// 分配下一个全局 ID（内部计数 +1，返回值再偏移 +100，与 Go 测试约定一致）。
pub fn GenGlobalID(store: &TestStore) -> Result<i64, TestkitError> {
    store.transaction(|state| {
        state.global_id = state
            .global_id
            .checked_add(1)
            .ok_or_else(|| TestkitError("global ID overflow".into()))?;
        Ok(state.global_id + 100)
    })
}

/// 构造 Public 状态的测试库元数据。
pub fn MockDBInfo(store: &TestStore, db_name: &str) -> Result<DbInfo, TestkitError> {
    Ok(DbInfo {
        id: GenGlobalID(store)?,
        name: CiString::new(db_name),
        state: SchemaState::Public,
        deprecated_tables: Vec::new(),
    })
}

/// 构造含单列 `a`（LongLong）的测试表元数据。
pub fn MockTableInfo(store: &TestStore, table_name: &str) -> Result<TableInfo, TestkitError> {
    let column_id = GenGlobalID(store)?;
    let table_id = GenGlobalID(store)?;
    Ok(TableInfo {
        id: table_id,
        name: CiString::new(table_name),
        columns: vec![ColumnInfo {
            id: column_id,
            name: CiString::new("a"),
            offset: 0,
            field_type: FieldType::LongLong,
            state: SchemaState::Public,
        }],
        state: SchemaState::Public,
        revision: 0,
    })
}

/// 由 TableInfo 包装为 Table 测试对象。
pub fn MockTable(_store: &TestStore, table_info: &TableInfo) -> Result<Table, TestkitError> {
    Ok(Table {
        meta: table_info.clone(),
    })
}

/// 构造测试用资源组元数据。
pub fn MockResourceGroupInfo(
    store: &TestStore,
    group_name: &str,
) -> Result<ResourceGroupInfo, TestkitError> {
    Ok(ResourceGroupInfo {
        id: GenGlobalID(store)?,
        name: CiString::new(group_name),
    })
}

/// 构造测试用 Placement Policy 元数据。
pub fn MockPolicyInfo(store: &TestStore, name: &str) -> Result<PolicyInfo, TestkitError> {
    Ok(PolicyInfo {
        id: GenGlobalID(store)?,
        name: CiString::new(name),
    })
}

/// 构造测试用 Policy 引用。
pub fn MockPolicyRefInfo(store: &TestStore, name: &str) -> Result<PolicyRefInfo, TestkitError> {
    Ok(PolicyRefInfo {
        id: GenGlobalID(store)?,
        name: CiString::new(name),
    })
}

/// 向指定库登记表；库不存在或表 ID 冲突则报错。
pub fn AddTable(store: &TestStore, db_id: i64, table: &TableInfo) -> Result<(), TestkitError> {
    store.transaction(|state| {
        if !state.databases.contains_key(&db_id) {
            return Err(TestkitError(format!("database {db_id} does not exist")));
        }
        if state
            .tables
            .insert((db_id, table.id), table.clone())
            .is_some()
        {
            return Err(TestkitError(format!("table {} already exists", table.id)));
        }
        Ok(())
    })
}

/// 更新已存在表的元数据。
pub fn UpdateTable(
    store: &TestStore,
    db_info: &DbInfo,
    table: &mut TableInfo,
) -> Result<(), TestkitError> {
    store.transaction(|state| {
        let existing = state
            .tables
            .get_mut(&(db_info.id, table.id))
            .ok_or_else(|| TestkitError(format!("table {} does not exist", table.id)))?;
        table.revision = table.revision.wrapping_add(1);
        *existing = table.clone();
        Ok(())
    })
}

/// 按库与表 ID 删除表。
pub fn DropTable(
    store: &TestStore,
    db_info: &DbInfo,
    table_id: i64,
    _table_name: &str,
) -> Result<(), TestkitError> {
    store.transaction(|state| {
        state
            .tables
            .remove(&(db_info.id, table_id))
            .map(|_| ())
            .ok_or_else(|| TestkitError(format!("table {table_id} does not exist")))
    })
}

/// 登记新库；ID 已存在则报错。
pub fn AddDB(store: &TestStore, db: &DbInfo) -> Result<(), TestkitError> {
    store.transaction(|state| {
        if state.databases.insert(db.id, db.clone()).is_some() {
            Err(TestkitError(format!("database {} already exists", db.id)))
        } else {
            Ok(())
        }
    })
}

/// 删除库及其下所有表登记。
pub fn DropDB(store: &TestStore, db: &DbInfo) -> Result<(), TestkitError> {
    store.transaction(|state| {
        state.databases.remove(&db.id);
        // 级联清理属于该库的表条目。
        state.tables.retain(|(db_id, _), _| *db_id != db.id);
        Ok(())
    })
}

/// 更新已存在库的元数据。
pub fn UpdateDB(store: &TestStore, db: &DbInfo) -> Result<(), TestkitError> {
    store.transaction(|state| {
        let existing = state
            .databases
            .get_mut(&db.id)
            .ok_or_else(|| TestkitError(format!("database {} does not exist", db.id)))?;
        *existing = db.clone();
        Ok(())
    })
}

/// 登记资源组；ID 冲突则报错。
pub fn AddResourceGroup(store: &TestStore, group: &ResourceGroupInfo) -> Result<(), TestkitError> {
    store.transaction(|state| {
        if group.id == 0 {
            return Err(TestkitError("group.ID is invalid".into()));
        }
        if state
            .resource_groups
            .insert(group.id, group.clone())
            .is_some()
        {
            Err(TestkitError(format!(
                "resource group {} already exists",
                group.id
            )))
        } else {
            Ok(())
        }
    })
}

/// 更新已存在资源组。
pub fn UpdateResourceGroup(
    store: &TestStore,
    group: &ResourceGroupInfo,
) -> Result<(), TestkitError> {
    store.transaction(|state| {
        // Go's default resource group is synthesized and may be updated
        // without an existing persisted record.
        if group.id == 1 {
            state.resource_groups.insert(group.id, group.clone());
            return Ok(());
        }
        let existing = state
            .resource_groups
            .get_mut(&group.id)
            .ok_or_else(|| TestkitError(format!("resource group {} does not exist", group.id)))?;
        *existing = group.clone();
        Ok(())
    })
}

/// 删除资源组。
pub fn DropResourceGroup(store: &TestStore, group: &ResourceGroupInfo) -> Result<(), TestkitError> {
    store.transaction(|state| {
        state.resource_groups.remove(&group.id);
        Ok(())
    })
}

/// 创建 Placement Policy；ID 冲突则报错。
pub fn CreatePolicy(store: &TestStore, policy: &PolicyInfo) -> Result<(), TestkitError> {
    store.transaction(|state| {
        if policy.id == 0 {
            return Err(TestkitError("policy.ID is invalid".into()));
        }
        if state.policies.insert(policy.id, policy.clone()).is_some() {
            Err(TestkitError(format!("policy {} already exists", policy.id)))
        } else {
            Ok(())
        }
    })
}

/// 更新已存在 Placement Policy。
pub fn UpdatePolicy(store: &TestStore, policy: &PolicyInfo) -> Result<(), TestkitError> {
    store.transaction(|state| {
        let existing = state
            .policies
            .get_mut(&policy.id)
            .ok_or_else(|| TestkitError(format!("policy {} does not exist", policy.id)))?;
        *existing = policy.clone();
        Ok(())
    })
}

/// 删除 Placement Policy。
pub fn DropPolicy(store: &TestStore, policy: &PolicyInfo) -> Result<(), TestkitError> {
    store.transaction(|state| {
        state.policies.remove(&policy.id);
        Ok(())
    })
}
