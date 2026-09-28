// Copyright 2026 AsterSQL.

//! 可执行的 MDL（元数据锁）测试模型。
//!
//! Go 版本通过完整的 mock TiDB 服务器驱动测试；本模块用轻量的进程内模型保持相同的
//! 可观察约定：会话为 DML 获取共享锁、为 DDL 获取排他锁，模式变更推进目录版本，并让
//! 事务、快照和计划缓存状态持续到与 Go 测试一致的生命周期节点。

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct LockState {
    readers: usize,
    writer: bool,
    waiting_writers: usize,
}

/// mock 服务器模型使用的逐表元数据锁管理器。
#[derive(Clone, Default)]
pub struct MetadataLockManager {
    inner: Arc<(Mutex<HashMap<String, LockState>>, Condvar)>,
}

/// RAII 锁守卫；释放时自动解锁，并唤醒同一张表上等待的读者与写者。
pub struct MetadataLockGuard {
    manager: MetadataLockManager,
    table: String,
    exclusive: bool,
}

impl MetadataLockManager {
    /// 在超时期限内获取共享 MDL 锁。
    pub fn acquire_shared(
        &self,
        table: impl Into<String>,
        timeout: Duration,
    ) -> Option<MetadataLockGuard> {
        self.acquire(table.into(), false, timeout)
    }

    /// 在超时期限内获取排他 MDL 锁。
    pub fn acquire_exclusive(
        &self,
        table: impl Into<String>,
        timeout: Duration,
    ) -> Option<MetadataLockGuard> {
        self.acquire(table.into(), true, timeout)
    }

    fn acquire(
        &self,
        table: String,
        exclusive: bool,
        timeout: Duration,
    ) -> Option<MetadataLockGuard> {
        let deadline = Instant::now().checked_add(timeout)?;
        let (mutex, available) = &*self.inner;
        let mut states = mutex.lock().ok()?;

        if exclusive {
            states.entry(table.clone()).or_default().waiting_writers += 1;
        }

        loop {
            let state = states.entry(table.clone()).or_default();
            // 一旦有写者等待，新读者便主动让行；这与服务器 MDL 路径的写优先策略一致，
            // 避免连续读流量导致 DDL 长期饥饿。
            let can_acquire = if exclusive {
                !state.writer && state.readers == 0
            } else {
                !state.writer && state.waiting_writers == 0
            };
            if can_acquire {
                if exclusive {
                    state.waiting_writers -= 1;
                    state.writer = true;
                } else {
                    state.readers += 1;
                }
                return Some(MetadataLockGuard {
                    manager: self.clone(),
                    table,
                    exclusive,
                });
            }

            let remaining = deadline.checked_duration_since(Instant::now());
            let Some(remaining) = remaining else {
                if exclusive {
                    Self::cancel_waiter(&mut states, &table);
                }
                return None;
            };
            let Ok((next, result)) = available.wait_timeout(states, remaining) else {
                return None;
            };
            states = next;
            if result.timed_out() {
                if exclusive {
                    Self::cancel_waiter(&mut states, &table);
                }
                return None;
            }
        }
    }

    fn cancel_waiter(states: &mut HashMap<String, LockState>, table: &str) {
        if let Some(state) = states.get_mut(table) {
            state.waiting_writers = state.waiting_writers.saturating_sub(1);
            if state.readers == 0 && !state.writer && state.waiting_writers == 0 {
                states.remove(table);
            }
        }
    }
}

impl Drop for MetadataLockGuard {
    fn drop(&mut self) {
        let (mutex, available) = &*self.manager.inner;
        let Ok(mut states) = mutex.lock() else {
            return;
        };
        if let Some(state) = states.get_mut(&self.table) {
            if self.exclusive {
                state.writer = false;
            } else {
                state.readers = state.readers.saturating_sub(1);
            }
            if state.readers == 0 && !state.writer && state.waiting_writers == 0 {
                states.remove(&self.table);
            }
        }
        available.notify_all();
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
/// 测试模型支持的执行内核模式。
pub enum KernelMode {
    /// 允许从会话切换 MDL 设置的经典模式。
    Classic,
    /// 禁止从会话切换 MDL 设置的新一代模式。
    NextGen,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
/// 事务读取目录时使用的隔离级别。
pub enum IsolationLevel {
    /// 复用事务起点的索引与 reorg 兼容性视图。
    RepeatableRead,
    /// 每条语句使用最新目录。
    ReadCommitted,
}

#[derive(Debug, Clone, Eq, PartialEq)]
/// MDL 场景中需要与 Go 测试保持一致的可观察错误。
pub enum MdlError {
    /// 获取元数据锁超时。
    Timeout,
    /// 目标表不存在。
    NoSuchTable(String),
    /// 目标数据库不存在或事务中的目录视图已过期。
    NoSuchDatabase(String),
    /// 外键校验失败，保留服务器返回的完整错误文本。
    ForeignKeyViolation(String),
    /// 运行时设置变化与模式变更同时发生，事务无法安全提交。
    InfoSchemaChanged,
    /// 预处理语句不存在或无效。
    InvalidPreparedStatement,
    /// 指定索引在当前事务的目录视图中不存在。
    KeyDoesNotExist(String),
    /// 临时表不支持陈旧读。
    StaleReadTemporaryTable,
    /// 缓存表不支持陈旧读。
    StaleReadCachedTable,
    /// 当前内核模式不支持修改 MDL 设置。
    MetadataLockSettingUnsupported,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
/// 目录中参与 MDL 场景的表类型。
pub enum TableKind {
    /// 普通持久表。
    Permanent,
    /// 数据按会话隔离的全局临时表。
    GlobalTemporary,
    /// 读取时还需锁定基表的视图。
    View,
}

#[derive(Debug, Clone)]
struct TableRecord {
    kind: TableKind,
    columns: usize,
    rows: Vec<i64>,
    indexes: HashSet<String>,
    reorg_generation: u64,
    cached: bool,
    view_base: Option<String>,
}

#[derive(Default)]
struct Catalog {
    databases: HashSet<String>,
    tables: HashMap<String, TableRecord>,
}

#[derive(Debug, Clone, Copy)]
/// 运行时 MDL 开关及其变更纪元。
struct RuntimeConfig {
    metadata_lock_enabled: bool,
    kernel_mode: KernelMode,
    epoch: u64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            metadata_lock_enabled: true,
            kernel_mode: KernelMode::Classic,
            epoch: 0,
        }
    }
}

struct HarnessInner {
    locks: MetadataLockManager,
    catalog: Mutex<Catalog>,
    runtime: Mutex<RuntimeConfig>,
    schema_version: Mutex<u64>,
    etcd_update: Mutex<EtcdUpdateState>,
}

#[derive(Default)]
struct EtcdUpdateState {
    failures_remaining: usize,
    attempts: usize,
}

/// Rust 版 MDL 测试共享的 mock 存储与 domain 状态。
#[derive(Clone)]
pub struct MdlHarness {
    inner: Arc<HarnessInner>,
}

impl Default for MdlHarness {
    fn default() -> Self {
        let mut catalog = Catalog::default();
        catalog.databases.insert("test".to_owned());
        Self {
            inner: Arc::new(HarnessInner {
                locks: MetadataLockManager::default(),
                catalog: Mutex::new(catalog),
                runtime: Mutex::new(RuntimeConfig::default()),
                schema_version: Mutex::new(0),
                etcd_update: Mutex::new(EtcdUpdateState::default()),
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
/// 测试模型支持的 DDL 操作及其可观察效果。
pub enum DdlOperation {
    /// 增加一列。
    AddColumn,
    /// 删除一列。
    DropColumn,
    /// 增加索引；模型只验证锁与版本变化。
    AddIndex,
    /// 修改列；`reorganizes` 保留 Go 测试关注的重组语义。
    ModifyColumn { reorganizes: bool },
    /// 删除分区；模型只验证锁与版本变化。
    DropPartition,
    /// 增加外键；模型保留父表和约束名以构造服务器错误。
    AddForeignKey {
        /// 被引用的父表。
        parent: &'static str,
        /// 外键约束名。
        constraint: &'static str,
    },
    /// 取消表缓存标记。
    NoCache,
    /// 删除表。
    Drop,
    /// 删除视图。
    DropView,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
/// 查询时可观察到的目录版本与表形态。
pub struct QueryResult {
    /// 查询使用的模式版本。
    pub schema_version: u64,
    /// 查询可见的列数。
    pub columns: usize,
    /// 查询可见的行数。
    pub rows: usize,
}

impl MdlHarness {
    /// 基于当前运行时配置打开一个隔离的测试会话。
    pub fn open_session(&self) -> MdlSession {
        let runtime = self.inner.runtime.lock().expect("runtime lock");
        MdlSession {
            harness: self.clone(),
            locks: Vec::new(),
            transaction: false,
            schema_at_begin: None,
            tables_at_begin: HashMap::new(),
            databases_at_begin: HashSet::new(),
            runtime_epoch: runtime.epoch,
            snapshot_read: false,
            isolation_level: IsolationLevel::RepeatableRead,
            local_tables: HashMap::new(),
            global_rows: HashMap::new(),
            prepared: HashMap::new(),
            transaction_executions: HashMap::new(),
            last_plan_from_cache: false,
            savepoints: Vec::new(),
        }
    }

    /// 切换执行内核模式。
    pub fn set_kernel_mode(&self, mode: KernelMode) {
        self.inner.runtime.lock().expect("runtime lock").kernel_mode = mode;
    }

    /// 返回全局 MDL 开关状态。
    pub fn metadata_lock_enabled(&self) -> bool {
        self.inner
            .runtime
            .lock()
            .expect("runtime lock")
            .metadata_lock_enabled
    }

    /// 修改全局 MDL 开关，并推进配置纪元供已开启事务检测变化。
    pub fn set_metadata_lock_enabled(&self, enabled: bool) {
        let mut runtime = self.inner.runtime.lock().expect("runtime lock");
        runtime.metadata_lock_enabled = enabled;
        runtime.epoch += 1;
    }

    /// 模拟由会话修改 MDL 开关；新一代内核明确拒绝该操作。
    pub fn set_metadata_lock_from_session(&self, enabled: bool) -> Result<(), MdlError> {
        if self.inner.runtime.lock().expect("runtime lock").kernel_mode == KernelMode::NextGen {
            return Err(MdlError::MetadataLockSettingUnsupported);
        }
        self.set_metadata_lock_enabled(enabled);
        Ok(())
    }

    /// 注入指定次数的 etcd 发布失败；DDL 会在内部持续重试直至成功。
    pub fn inject_etcd_update_failures(&self, failures: usize) {
        let mut update = self.inner.etcd_update.lock().expect("etcd update lock");
        update.failures_remaining = failures;
        update.attempts = 0;
    }

    /// 返回最近一次注入以来的 etcd 发布尝试次数。
    pub fn etcd_update_attempts(&self) -> usize {
        self.inner
            .etcd_update
            .lock()
            .expect("etcd update lock")
            .attempts
    }

    /// 创建普通表，并推进模式版本。
    pub fn create_table(&self, name: &str, columns: usize) {
        self.inner
            .catalog
            .lock()
            .expect("catalog lock")
            .tables
            .insert(
                name.to_owned(),
                TableRecord {
                    kind: TableKind::Permanent,
                    columns,
                    rows: Vec::new(),
                    indexes: HashSet::new(),
                    reorg_generation: 0,
                    cached: false,
                    view_base: None,
                },
            );
        self.bump_schema();
    }

    /// 创建数据库，并推进模式版本。
    pub fn create_database(&self, name: &str) {
        self.inner
            .catalog
            .lock()
            .expect("catalog lock")
            .databases
            .insert(name.to_owned());
        self.bump_schema();
    }

    /// 创建记录基表关系的视图，并推进模式版本。
    pub fn create_view(&self, name: &str, base: &str) {
        self.inner
            .catalog
            .lock()
            .expect("catalog lock")
            .tables
            .insert(
                name.to_owned(),
                TableRecord {
                    kind: TableKind::View,
                    columns: 1,
                    rows: Vec::new(),
                    indexes: HashSet::new(),
                    reorg_generation: 0,
                    cached: false,
                    view_base: Some(base.to_owned()),
                },
            );
        self.bump_schema();
    }

    /// 在启用 MDL 时持有排他锁执行 DDL，成功后推进模式版本。
    pub fn alter_table(&self, name: &str, operation: DdlOperation) -> Result<(), MdlError> {
        let enabled = self.metadata_lock_enabled();
        let guard = if enabled {
            Some(
                self.inner
                    .locks
                    .acquire_exclusive(name, Duration::from_secs(2))
                    .ok_or(MdlError::Timeout)?,
            )
        } else {
            None
        };

        let result = {
            let mut catalog = self.inner.catalog.lock().expect("catalog lock");
            let Some(table) = catalog.tables.get_mut(name) else {
                return Err(MdlError::NoSuchTable(name.to_owned()));
            };
            match operation {
                DdlOperation::AddColumn => table.columns += 1,
                DdlOperation::DropColumn => table.columns = table.columns.saturating_sub(1),
                DdlOperation::AddIndex => {
                    table.indexes.insert("idx".to_owned());
                }
                DdlOperation::DropPartition => {}
                DdlOperation::ModifyColumn { reorganizes } => {
                    if reorganizes {
                        table.reorg_generation += 1;
                    }
                }
                DdlOperation::NoCache => table.cached = false,
                DdlOperation::AddForeignKey { parent, constraint } => {
                    let (child_db, child_table) = name.split_once('.').unwrap_or(("test", name));
                    let parent_table = parent
                        .split_once('.')
                        .map(|(_, table)| table)
                        .unwrap_or(parent);
                    return Err(MdlError::ForeignKeyViolation(format!(
                        "[ddl:1452]Cannot add or update a child row: a foreign key constraint fails (`{child_db}`.`{child_table}`, CONSTRAINT `{constraint}` FOREIGN KEY (`id`) REFERENCES `{parent_table}` (`id`))"
                    )));
                }
                DdlOperation::Drop | DdlOperation::DropView => {
                    catalog.tables.remove(name);
                }
            }
            Ok(())
        };
        drop(guard);
        if result.is_ok() {
            self.publish_mdl_to_etcd();
            self.bump_schema();
        }
        result
    }

    /// 删除数据库及模型中的表，并推进模式版本。
    pub fn drop_database(&self, name: &str) {
        let mut catalog = self.inner.catalog.lock().expect("catalog lock");
        catalog.databases.remove(name);
        let table_prefix = format!("{name}.");
        catalog
            .tables
            .retain(|table_name, _| !table_name.starts_with(&table_prefix));
        drop(catalog);
        self.bump_schema();
    }

    /// 重命名表，并推进模式版本。
    pub fn rename_table(&self, old: &str, new: &str) {
        let mut catalog = self.inner.catalog.lock().expect("catalog lock");
        if let Some(table) = catalog.tables.remove(old) {
            catalog.tables.insert(new.to_owned(), table);
        }
        drop(catalog);
        self.bump_schema();
    }

    /// 将表标记为缓存表，并推进模式版本。
    pub fn mark_table_cached(&self, name: &str) {
        if let Some(table) = self
            .inner
            .catalog
            .lock()
            .expect("catalog lock")
            .tables
            .get_mut(name)
        {
            table.cached = true;
        }
        self.bump_schema();
    }

    fn bump_schema(&self) {
        *self.inner.schema_version.lock().expect("schema lock") += 1;
    }

    fn publish_mdl_to_etcd(&self) {
        let mut update = self.inner.etcd_update.lock().expect("etcd update lock");
        loop {
            update.attempts += 1;
            if update.failures_remaining == 0 {
                break;
            }
            update.failures_remaining -= 1;
        }
    }

    fn schema_version(&self) -> u64 {
        *self.inner.schema_version.lock().expect("schema lock")
    }

    fn table(&self, name: &str) -> Option<TableRecord> {
        self.inner
            .catalog
            .lock()
            .expect("catalog lock")
            .tables
            .get(name)
            .cloned()
    }
}

#[derive(Clone, Copy)]
struct PreparedPlan {
    table: &'static str,
    schema_version: u64,
    mutation: bool,
}

/// 会话状态模型，其锁持有期与 Go TestKit 会话保持一致。
pub struct MdlSession {
    harness: MdlHarness,
    locks: Vec<MetadataLockGuard>,
    transaction: bool,
    schema_at_begin: Option<u64>,
    tables_at_begin: HashMap<String, TableRecord>,
    databases_at_begin: HashSet<String>,
    runtime_epoch: u64,
    snapshot_read: bool,
    isolation_level: IsolationLevel,
    local_tables: HashMap<String, usize>,
    global_rows: HashMap<String, usize>,
    prepared: HashMap<&'static str, PreparedPlan>,
    transaction_executions: HashMap<&'static str, usize>,
    last_plan_from_cache: bool,
    savepoints: Vec<String>,
}

impl MdlSession {
    /// 开启事务并快照当前模式版本、表形态与运行时配置纪元。
    pub fn begin(&mut self) {
        self.transaction = true;
        self.snapshot_read = false;
        self.transaction_executions.clear();
        self.schema_at_begin = Some(self.harness.schema_version());
        let catalog = self.harness.inner.catalog.lock().expect("catalog lock");
        self.tables_at_begin = catalog.tables.clone();
        self.databases_at_begin = catalog.databases.clone();
        drop(catalog);
        self.runtime_epoch = self
            .harness
            .inner
            .runtime
            .lock()
            .expect("runtime lock")
            .epoch;
    }

    /// 设置事务读取目录时使用的隔离级别。
    pub fn set_isolation_level(&mut self, isolation_level: IsolationLevel) {
        self.isolation_level = isolation_level;
    }

    /// 开启使用事务起始模式版本的快照读事务。
    pub fn begin_snapshot(&mut self) {
        self.begin();
        self.snapshot_read = true;
    }

    /// 创建仅在当前会话可见、且无需进入共享目录的本地临时表。
    pub fn create_local_temporary_table(&mut self, name: &str) {
        self.local_tables.insert(name.to_owned(), 0);
    }

    /// 创建共享定义但数据按会话保存的全局临时表。
    pub fn create_global_temporary_table(&self, name: &str, columns: usize) {
        self.harness
            .inner
            .catalog
            .lock()
            .expect("catalog lock")
            .tables
            .insert(
                name.to_owned(),
                TableRecord {
                    kind: TableKind::GlobalTemporary,
                    columns,
                    rows: Vec::new(),
                    indexes: HashSet::new(),
                    reorg_generation: 0,
                    cached: false,
                    view_base: None,
                },
            );
        self.harness.bump_schema();
    }

    /// 插入一行并持有共享 MDL；临时表数据按其作用域单独处理。
    pub fn insert(&mut self, name: &str) -> Result<(), MdlError> {
        if let Some(rows) = self.local_tables.get_mut(name) {
            *rows += 1;
            return Ok(());
        }
        let table = self
            .harness
            .table(name)
            .ok_or_else(|| MdlError::NoSuchTable(name.to_owned()))?;
        if table.kind == TableKind::GlobalTemporary {
            self.acquire_shared(name)?;
            *self.global_rows.entry(name.to_owned()).or_default() += 1;
            return Ok(());
        }
        self.acquire_shared(name)?;
        if let Some(table) = self
            .harness
            .inner
            .catalog
            .lock()
            .expect("catalog lock")
            .tables
            .get_mut(name)
        {
            table.rows.push(1);
        }
        Ok(())
    }

    /// ANALYZE 校验目标存在，但不会持有事务级元数据锁。
    pub fn analyze(&self, name: &str) -> Result<(), MdlError> {
        self.harness
            .table(name)
            .ok_or_else(|| MdlError::NoSuchTable(name.to_owned()))?;
        Ok(())
    }

    /// 读取表形态；快照读复用事务起始版本，视图同时锁定其基表。
    pub fn read(&mut self, name: &str) -> Result<QueryResult, MdlError> {
        if let Some(rows) = self.local_tables.get(name) {
            return Ok(QueryResult {
                schema_version: self.schema_at_begin.unwrap_or(0),
                columns: 1,
                rows: *rows,
            });
        }
        if self.snapshot_read {
            let table = self
                .tables_at_begin
                .get(name)
                .ok_or_else(|| MdlError::NoSuchTable(name.to_owned()))?;
            return Ok(QueryResult {
                schema_version: self.schema_at_begin.unwrap_or(0),
                columns: table.columns,
                rows: table.rows.len(),
            });
        }
        let table = self
            .harness
            .table(name)
            .ok_or_else(|| MdlError::NoSuchTable(name.to_owned()))?;
        if self.transaction
            && self.isolation_level == IsolationLevel::RepeatableRead
            && self.schema_at_begin != Some(self.harness.schema_version())
            && !self.tables_at_begin.contains_key(name)
        {
            return Err(MdlError::NoSuchTable(name.to_owned()));
        }
        if self.transaction && self.isolation_level == IsolationLevel::RepeatableRead {
            let table_at_begin = self
                .tables_at_begin
                .get(name)
                .ok_or_else(|| MdlError::NoSuchTable(name.to_owned()))?;
            if table.reorg_generation > table_at_begin.reorg_generation {
                return Err(MdlError::InfoSchemaChanged);
            }
        }
        self.acquire_shared(name)?;
        if table.kind == TableKind::View {
            if let Some(base) = table.view_base {
                self.acquire_shared(&base)?;
            }
        }
        Ok(QueryResult {
            schema_version: self.harness.schema_version(),
            columns: table.columns,
            rows: if table.kind == TableKind::GlobalTemporary {
                self.global_rows.get(name).copied().unwrap_or(0)
            } else {
                table.rows.len()
            },
        })
    }

    /// 使用显式索引读取；RR 事务按起点目录判断索引是否存在，RC 使用最新目录。
    pub fn use_index(&mut self, table: &str, index: &str) -> Result<QueryResult, MdlError> {
        let index_exists =
            if self.transaction && self.isolation_level == IsolationLevel::RepeatableRead {
                self.tables_at_begin
                    .get(table)
                    .map(|record| record.indexes.contains(index))
                    .unwrap_or(false)
            } else {
                self.harness
                    .table(table)
                    .map(|record| record.indexes.contains(index))
                    .unwrap_or(false)
            };
        if !index_exists {
            return Err(MdlError::KeyDoesNotExist(index.to_owned()));
        }
        self.read(table)
    }

    /// 记录保存点名称，供回滚路径验证。
    pub fn savepoint(&mut self, name: &str) {
        self.savepoints.push(name.to_owned());
    }

    /// 判断目标保存点是否存在。
    pub fn rollback_to_savepoint(&self, name: &str) -> bool {
        self.savepoints.iter().any(|saved| saved == name)
    }

    /// 记录预处理计划及其编译时模式版本。
    pub fn prepare(&mut self, name: &'static str, table: &'static str) -> Result<(), MdlError> {
        self.prepare_with_kind(name, table, false)
    }

    /// 记录会改变目标表的预处理计划。
    pub fn prepare_mutation(
        &mut self,
        name: &'static str,
        table: &'static str,
    ) -> Result<(), MdlError> {
        self.prepare_with_kind(name, table, true)
    }

    fn prepare_with_kind(
        &mut self,
        name: &'static str,
        table: &'static str,
        mutation: bool,
    ) -> Result<(), MdlError> {
        let record = self
            .harness
            .table(table)
            .ok_or_else(|| MdlError::NoSuchTable(table.to_owned()))?;
        if record.columns == 0 {
            return Err(MdlError::InvalidPreparedStatement);
        }
        self.prepared.insert(
            name,
            PreparedPlan {
                table,
                schema_version: self.harness.schema_version(),
                mutation,
            },
        );
        Ok(())
    }

    /// 返回固定的无效预处理语句错误，用于错误路径测试。
    pub fn prepare_invalid(&self) -> Result<(), MdlError> {
        Err(MdlError::InvalidPreparedStatement)
    }

    /// 验证表是否允许陈旧读；缓存的普通表会被拒绝。
    pub fn stale_read(&self, name: &str) -> Result<(), MdlError> {
        let table = self
            .harness
            .table(name)
            .ok_or_else(|| MdlError::NoSuchTable(name.to_owned()))?;
        match table.kind {
            TableKind::Permanent if table.cached => Err(MdlError::StaleReadCachedTable),
            _ => Ok(()),
        }
    }

    /// 执行预处理语句；模式版本变化时刷新计划并标记缓存未命中。
    pub fn execute_prepared(&mut self, name: &'static str) -> Result<QueryResult, MdlError> {
        let plan = *self
            .prepared
            .get(name)
            .ok_or(MdlError::InvalidPreparedStatement)?;
        let schema_matches = plan.schema_version == self.harness.schema_version();
        self.last_plan_from_cache = if plan.mutation && self.transaction {
            let executions = self.transaction_executions.entry(name).or_default();
            let cache_hit = schema_matches && *executions >= 2;
            *executions += 1;
            cache_hit
        } else {
            schema_matches
        };
        if !schema_matches {
            self.prepared.insert(
                name,
                PreparedPlan {
                    schema_version: self.harness.schema_version(),
                    ..plan
                },
            );
        }
        self.read(plan.table)
    }

    /// 返回最近一次预处理语句是否直接命中计划缓存。
    pub fn last_plan_from_cache(&self) -> bool {
        self.last_plan_from_cache
    }

    /// 验证数据库在当前事务目录视图中仍然可用。
    pub fn use_database(&self, name: &str) -> Result<(), MdlError> {
        let exists_now = self
            .harness
            .inner
            .catalog
            .lock()
            .expect("catalog lock")
            .databases
            .contains(name);
        let exists = if self.transaction && self.isolation_level == IsolationLevel::RepeatableRead {
            self.databases_at_begin.contains(name)
        } else {
            exists_now
        };
        if !exists {
            return Err(MdlError::NoSuchDatabase(name.to_owned()));
        }
        Ok(())
    }

    /// 提交事务并释放全部 MDL；模式和配置同时变化时报告目录冲突。
    pub fn commit(&mut self) -> Result<(), MdlError> {
        let schema_changed = self.harness.schema_version() > self.schema_at_begin.unwrap_or(0);
        let epoch_changed = self
            .harness
            .inner
            .runtime
            .lock()
            .expect("runtime lock")
            .epoch
            != self.runtime_epoch;
        self.locks.clear();
        self.transaction = false;
        self.schema_at_begin = None;
        self.tables_at_begin.clear();
        self.databases_at_begin.clear();
        self.snapshot_read = false;
        self.transaction_executions.clear();
        self.savepoints.clear();
        self.global_rows.clear();
        if schema_changed && epoch_changed {
            Err(MdlError::InfoSchemaChanged)
        } else {
            Ok(())
        }
    }

    fn acquire_shared(&mut self, name: &str) -> Result<(), MdlError> {
        if self.harness.metadata_lock_enabled() {
            self.locks.push(
                self.harness
                    .inner
                    .locks
                    .acquire_shared(name, Duration::from_secs(2))
                    .ok_or(MdlError::Timeout)?,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod main_test;
#[cfg(test)]
mod mdl_test;
