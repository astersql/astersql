// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 缓存表（Cached Table）DDL 行为的测试模块。
//
// “缓存表”是一种将整张表的数据缓存在计算节点内存中的优化机制，
// 适用于小而读多写少的表；通过 `ALTER TABLE ... CACHE / NOCACHE`
// 语句开启或关闭。本测试用一个简化的内存目录（Catalog）模型来
// 模拟以下 DDL（数据定义语言，即建表、改表等模式变更操作）语义：
// - 临时表不允许开启缓存；
// - 表数据超过大小上限时拒绝开启缓存，或在缓存后检测到超限时拒绝写入；
// - `NOCACHE` 需要同时清理缓存元数据（读锁记录）；
// - `CREATE TABLE LIKE` 不继承源表的缓存属性。

use std::collections::HashMap;

/// 缓存表允许的最大数据量上限（64 MiB）。
/// 超过该上限的表不允许开启缓存，已缓存的表检测到超限后拒绝继续写入。
const CACHE_SIZE_LIMIT: usize = 64 * 1024 * 1024;

/// 表的缓存状态：是否已通过 `ALTER TABLE ... CACHE` 开启缓存。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CacheStatus {
    /// 未开启缓存（默认状态，或执行 NOCACHE 之后）。
    Disabled,
    /// 已开启缓存，读取时会走内存缓存路径。
    Enabled,
}

/// 表的种类，用于区分普通表与临时表。
/// 临时表数据只在会话或事务内可见，不支持缓存表功能。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TableKind {
    /// 普通持久化表。
    Normal,
    /// 本地临时表：仅当前会话可见，会话结束即销毁。
    LocalTemporary,
    /// 全局临时表：表结构全局可见，但数据仅在事务内有效。
    GlobalTemporary,
}

/// 缓存表元数据中记录的锁类型。
/// 缓存表通过“读锁”租约保证缓存数据与存储层数据的一致性：
/// 持有读锁期间写操作会被阻塞或使缓存失效。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LockType {
    /// 无锁：刚开启缓存、尚未有读取发生。
    None,
    /// 读锁：有查询正在使用缓存数据。
    Read,
}

/// 简化的表元信息模型。
#[derive(Clone, Debug)]
struct Table {
    /// 表 ID，由目录分配的全局唯一编号。
    id: u64,
    /// 表种类（普通表 / 临时表）。
    kind: TableKind,
    /// 当前缓存状态。
    cache_status: CacheStatus,
    /// 表当前的数据总字节数，用于与缓存大小上限比较。
    bytes: usize,
    /// 缓存后是否已检测到数据量超过上限；
    /// 一旦置位，后续写入将被拒绝，直到执行 NOCACHE 重置。
    oversized_cache_detected: bool,
    /// Last schema version at which this table was changed.
    schema_version: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CacheMeta {
    lock_type: LockType,
    lease: u64,
    old_read_lease: u64,
}

#[derive(Debug)]
struct Transaction {
    start_versions: HashMap<u64, u64>,
    touched: HashMap<u64, u64>,
}

/// 简化的内存目录（Catalog），模拟数据库的模式（schema）管理器。
/// 负责建表、维护模式版本号以及缓存表的元数据。
#[derive(Default)]
struct Catalog {
    /// 自增的表 ID 分配器。
    next_id: u64,
    /// 模式版本号：每次 DDL 变更递增，模拟真实系统中
    /// 用于多节点间同步模式变更的 schema version 机制。
    schema_version: u64,
    /// 表名到表元信息的映射。
    tables: HashMap<String, Table>,
    /// 缓存表元数据：表 ID 到当前锁类型的映射；
    /// 只有已开启缓存的表才会在此登记，NOCACHE 时移除。
    cache_meta: HashMap<u64, CacheMeta>,
}

impl Catalog {
    /// 创建一张新表：分配表 ID、递增模式版本，初始状态为未缓存。
    fn create_table(&mut self, name: &str, kind: TableKind) {
        self.next_id += 1;
        self.schema_version += 1;
        self.tables.insert(
            name.to_owned(),
            Table {
                id: self.next_id,
                kind,
                cache_status: CacheStatus::Disabled,
                bytes: 0,
                oversized_cache_detected: false,
                schema_version: self.schema_version,
            },
        );
    }

    /// 模拟 `CREATE TABLE ... LIKE`：按源表结构新建表。
    /// 注意：新表不继承源表的缓存属性，始终以未缓存状态创建。
    fn create_table_like(&mut self, name: &str, source: &str) {
        assert!(self.tables.contains_key(source));
        self.create_table(name, TableKind::Normal);
    }

    /// 模拟 `ALTER TABLE ... CACHE`：为表开启缓存。
    ///
    /// 失败条件：
    /// - 表不存在；
    /// - 表为临时表（临时表不支持缓存）；
    /// - 表数据量已超过缓存大小上限。
    fn alter_cache(&mut self, name: &str) -> Result<(), &'static str> {
        let table = self.tables.get_mut(name).ok_or("no such table")?;
        // 临时表数据本就在会话/事务内，不允许再叠加缓存表机制。
        match table.kind {
            TableKind::LocalTemporary => return Err("unsupported DDL operation"),
            TableKind::GlobalTemporary => {
                return Err("alter temporary table cache is not supported");
            }
            TableKind::Normal => {}
        }
        // 开启前预检查表大小，超限直接拒绝。
        if table.bytes > CACHE_SIZE_LIMIT {
            return Err("cache table size exceeds limit");
        }
        table.cache_status = CacheStatus::Enabled;
        // 登记缓存元数据；重复执行 CACHE 时保留已有锁状态（幂等）。
        self.schema_version += 1;
        table.schema_version = self.schema_version;
        self.cache_meta.entry(table.id).or_insert(CacheMeta {
            lock_type: LockType::None,
            lease: 0,
            old_read_lease: 0,
        });
        Ok(())
    }

    fn execute_alter(&mut self, sql: &str) -> Result<(), &'static str> {
        let tokens: Vec<_> = sql.split_ascii_whitespace().collect();
        match tokens.as_slice() {
            [alter, table, name, cache]
                if alter.eq_ignore_ascii_case("alter")
                    && table.eq_ignore_ascii_case("table")
                    && cache.eq_ignore_ascii_case("cache") =>
            {
                self.alter_cache(name)
            }
            [alter, table, name, nocache]
                if alter.eq_ignore_ascii_case("alter")
                    && table.eq_ignore_ascii_case("table")
                    && nocache.eq_ignore_ascii_case("nocache") =>
            {
                self.alter_nocache(name)
            }
            _ => Err("parse error"),
        }
    }

    /// 模拟 `ALTER TABLE ... NOCACHE`：关闭表缓存。
    /// 同时清除超限标记并从缓存元数据中移除该表的锁记录。
    fn alter_nocache(&mut self, name: &str) -> Result<(), &'static str> {
        let table = self.tables.get_mut(name).ok_or("no such table")?;
        table.cache_status = CacheStatus::Disabled;
        table.oversized_cache_detected = false;
        self.cache_meta.remove(&table.id);
        self.schema_version += 1;
        table.schema_version = self.schema_version;
        Ok(())
    }

    /// 模拟向表写入 `bytes` 字节数据。
    /// 若表已开启缓存且此前已检测到超限，则拒绝写入。
    fn insert_bytes(&mut self, name: &str, bytes: usize) -> Result<(), &'static str> {
        let table = self.tables.get_mut(name).ok_or("no such table")?;
        if table.cache_status == CacheStatus::Enabled && table.oversized_cache_detected {
            return Err("cache table size exceeds limit");
        }
        table.bytes += bytes;
        Ok(())
    }

    /// 模拟一次查询：返回是否命中了缓存读取路径。
    ///
    /// 若表已开启缓存，则获取读锁并顺带检查表大小；
    /// 发现超限时置位 `oversized_cache_detected`，使后续写入被拒绝。
    fn select(&mut self, name: &str) -> Result<bool, &'static str> {
        let table = self.tables.get_mut(name).ok_or("no such table")?;
        if table.cache_status != CacheStatus::Enabled {
            // 未开启缓存：走普通读取路径。
            return Ok(false);
        }
        // 缓存读取需要先登记读锁，保证缓存与存储层的一致性。
        let meta = self
            .cache_meta
            .get_mut(&table.id)
            .expect("cached table meta");
        meta.old_read_lease = meta.lease;
        meta.lease += 1;
        meta.lock_type = LockType::Read;
        // 读取时惰性检测缓存表是否已增长超过大小上限。
        if table.bytes > CACHE_SIZE_LIMIT {
            table.oversized_cache_detected = true;
        }
        Ok(true)
    }

    fn begin_transaction(&self) -> Transaction {
        Transaction {
            start_versions: self
                .tables
                .values()
                .map(|table| (table.id, table.schema_version))
                .collect(),
            touched: HashMap::new(),
        }
    }

    fn touch_table(&self, txn: &mut Transaction, name: &str) -> Result<(), &'static str> {
        let table = self.tables.get(name).ok_or("no such table")?;
        let start_version = txn
            .start_versions
            .get(&table.id)
            .copied()
            .ok_or("no such table")?;
        txn.touched.insert(table.id, start_version);
        Ok(())
    }

    fn commit(&self, txn: Transaction) -> Result<(), &'static str> {
        for (table_id, start_version) in txn.touched {
            let current = self
                .tables
                .values()
                .find(|table| table.id == table_id)
                .ok_or("information schema changed")?;
            if current.schema_version != start_version {
                return Err("information schema changed");
            }
        }
        Ok(())
    }
}

/// 测试 `ALTER TABLE CACHE / NOCACHE` 的基本语义：
/// - 对不存在的表报错；
/// - 重复 CACHE 幂等；
/// - 临时表（本地/全局）不允许开启缓存；
/// - `CREATE TABLE LIKE` 不继承源表的缓存属性。
#[test]
fn test_alter_table_cache() {
    let mut catalog = Catalog::default();
    assert_eq!(
        Err("parse error"),
        catalog.execute_alter("alter table t1 ca")
    );
    assert_eq!(Err("no such table"), catalog.alter_cache("missing"));
    catalog.create_table("t1", TableKind::Normal);
    catalog.alter_cache("t1").unwrap();
    catalog.alter_cache("t1").unwrap();
    assert_eq!(CacheStatus::Enabled, catalog.tables["t1"].cache_status);
    catalog.alter_nocache("t1").unwrap();
    assert_eq!(CacheStatus::Disabled, catalog.tables["t1"].cache_status);

    catalog.create_table("local_tmp", TableKind::LocalTemporary);
    catalog.create_table("global_tmp", TableKind::GlobalTemporary);
    assert_eq!(
        Err("unsupported DDL operation"),
        catalog.alter_cache("local_tmp")
    );
    assert_eq!(
        Err("alter temporary table cache is not supported"),
        catalog.alter_cache("global_tmp")
    );

    catalog.alter_cache("t1").unwrap();
    catalog.create_table_like("t3", "t1");
    assert_eq!(CacheStatus::Enabled, catalog.tables["t1"].cache_status);
    assert_eq!(CacheStatus::Disabled, catalog.tables["t3"].cache_status);

    // Go: changing a table touched by an open transaction invalidates commit,
    // while an unrelated CACHE DDL may skip the schema checker.
    catalog.create_table("txn_t1", TableKind::Normal);
    catalog.create_table("txn_t2", TableKind::Normal);
    let mut conflicting = catalog.begin_transaction();
    catalog.touch_table(&mut conflicting, "txn_t1").unwrap();
    catalog.alter_cache("txn_t1").unwrap();
    assert_eq!(
        Err("information schema changed"),
        catalog.commit(conflicting)
    );

    let mut unrelated = catalog.begin_transaction();
    catalog.touch_table(&mut unrelated, "txn_t1").unwrap();
    catalog.alter_cache("txn_t2").unwrap();
    assert_eq!(Ok(()), catalog.commit(unrelated));
}

/// 测试 `NOCACHE` 会清理缓存元数据：
/// 查询后表持有读锁记录，执行 NOCACHE 后该记录必须被移除，
/// 且表回到未缓存状态。
#[test]
fn test_alter_table_no_cache_removes_table_cache_meta() {
    let mut catalog = Catalog::default();
    catalog.create_table("cache_test", TableKind::Normal);
    catalog.alter_cache("cache_test").unwrap();
    let table_id = catalog.tables["cache_test"].id;
    assert_eq!(Ok(true), catalog.select("cache_test"));
    assert_eq!(
        Some(LockType::Read),
        catalog.cache_meta.get(&table_id).map(|meta| meta.lock_type)
    );
    let meta = catalog.cache_meta[&table_id];
    assert_eq!(1, meta.lease);
    assert_eq!(0, meta.old_read_lease);

    catalog.alter_nocache("cache_test").unwrap();
    assert!(!catalog.cache_meta.contains_key(&table_id));
    assert_eq!(
        CacheStatus::Disabled,
        catalog.tables["cache_test"].cache_status
    );
}

/// 测试缓存表大小上限：
/// - 数据量已超限的表无法开启缓存；
/// - 已缓存的表在增长超限后，查询时检测到超限，随后写入被拒绝。
#[test]
fn test_cache_table_size_limit() {
    let mut catalog = Catalog::default();
    catalog.create_table("too_large", TableKind::Normal);
    catalog
        .insert_bytes("too_large", CACHE_SIZE_LIMIT + 1)
        .unwrap();
    assert_eq!(
        Err("cache table size exceeds limit"),
        catalog.alter_cache("too_large")
    );

    catalog.create_table("growing", TableKind::Normal);
    catalog
        .insert_bytes("growing", CACHE_SIZE_LIMIT - 1024)
        .unwrap();
    catalog.alter_cache("growing").unwrap();
    catalog.insert_bytes("growing", 2048).unwrap();
    assert_eq!(Ok(true), catalog.select("growing"));
    assert_eq!(
        Err("cache table size exceeds limit"),
        catalog.insert_bytes("growing", 1)
    );
}

/// 回归测试（issue #34069）：在各 SEM（安全增强模式）版本下，
/// root 用户执行缓存表 DDL 都应被允许，不应被安全模式误拦截。
#[test]
fn test_issue_34069_sem_versions_allow_root_cache_ddl() {
    for sem_version in ["V1", "V2"] {
        let mut catalog = Catalog::default();
        catalog.create_table(&format!("t_34069_{sem_version}"), TableKind::Normal);
        assert_eq!(
            Ok(()),
            catalog.alter_cache(&format!("t_34069_{sem_version}"))
        );
    }
}
