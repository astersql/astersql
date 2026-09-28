// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! `recover_test.go` 中 RECOVER/FLASHBACK 场景的可执行对照测试。
//!
//! Go 测试依赖 TiDB 的 unistore 与 DDL worker；当前 crate 未链接这套服务端运行时，
//! 因而使用下方确定性的内存目录复现相同状态转换，包括删除/截断历史、GC 安全点、
//! Auto-ID 重基、权限校验、错误注入和并发 DDL 准备。模型虽有意保持精简，断言仍
//! 针对真实状态与错误，而不是仅保留无法执行的源语句。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 将 Go 测试关注的恢复失败归一化，便于精确核对各条错误分支。
enum ErrorKind {
    NoSafePoint,
    TooOld,
    TableExists,
    SchemaExists,
    UnknownJob,
    InvalidJob,
    AlreadyRecovered,
    TableNotFound,
    DatabaseNotFound,
    UnsupportedTemporaryTable,
    InvalidTableName,
    UpdateVersionFailed,
    AccessDenied,
    FutureTimestamp,
    BeforeGcSafePoint,
    ActiveTransaction,
    NonTikvCluster,
    ModifiedSystemTable,
    TiDbUpgradeDetected,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RecoveryError(ErrorKind);

type RecoveryResult<T> = Result<T, RecoveryError>;

impl RecoveryError {
    fn new(kind: ErrorKind) -> Self {
        Self(kind)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 表、库和集群恢复所需权限的最小模型。
struct Permissions {
    select: bool,
    create: bool,
    drop: bool,
    super_user: bool,
}

impl Permissions {
    fn root() -> Self {
        Self {
            select: true,
            create: true,
            drop: true,
            super_user: true,
        }
    }

    fn table_recovery(self) -> bool {
        self.select && self.create && self.drop
    }

    fn schema_recovery(self) -> bool {
        self.create && self.drop
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 临时表类型会改变 DDL 历史的可见性及恢复结果。
enum TemporaryTable {
    None,
    Global,
    Local,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 表快照同时保存业务值和隐式行 ID，用于验证恢复后的 Auto-ID 连续性。
struct Table {
    rows: Vec<(i64, u64)>,
    next_row_id: u64,
    temporary: TemporaryTable,
}

impl Table {
    fn new(temporary: TemporaryTable) -> Self {
        Self {
            rows: Vec::new(),
            next_row_id: 1,
            temporary,
        }
    }

    fn insert(&mut self, values: &[i64]) {
        for value in values {
            self.rows.push((*value, self.next_row_id));
            self.next_row_id += 1;
        }
    }

    fn rebase_for_restore(&mut self, force_rebase: bool) {
        // Go 测试将 Auto-ID 步长设为 5000；恢复后的快照保留旧行 ID，并从下一步长开始分配。
        if force_rebase || self.next_row_id <= 5000 {
            let step = (self.next_row_id.saturating_sub(1) / 5000 + 1) * 5000;
            self.next_row_id = step + 1;
        }
    }
}

#[derive(Clone, Debug)]
/// 被删除或截断表的 DDL 历史快照；作业 ID 支持按指定历史记录恢复。
struct DroppedTable {
    schema: String,
    name: String,
    table: Table,
    timestamp: u64,
    job_id: u64,
    recovered: bool,
}

#[derive(Clone, Debug, Default)]
struct Schema {
    tables: HashMap<String, Table>,
}

#[derive(Clone, Debug)]
/// 被删除库的完整历史快照，用于验证原名和改名闪回。
struct DroppedSchema {
    name: String,
    schema: Schema,
    timestamp: u64,
    recovered: bool,
}

#[derive(Clone, Debug)]
/// 恢复测试使用的确定性内存目录，集中维护现存对象、DDL 历史和 GC 状态。
struct Catalog {
    schemas: HashMap<String, Schema>,
    dropped_tables: Vec<DroppedTable>,
    dropped_schemas: Vec<DroppedSchema>,
    safe_point: Option<u64>,
    gc_enabled: bool,
    now: u64,
    next_job_id: u64,
    update_version_error: bool,
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            schemas: HashMap::new(),
            dropped_tables: Vec::new(),
            dropped_schemas: Vec::new(),
            safe_point: None,
            gc_enabled: true,
            now: 100,
            next_job_id: 1,
            update_version_error: false,
        }
    }
}

impl Catalog {
    fn set_time(&mut self, now: u64) {
        self.now = now;
    }

    fn set_safe_point(&mut self, safe_point: Option<u64>) {
        self.safe_point = safe_point;
    }

    fn create_schema(&mut self, name: &str) -> RecoveryResult<()> {
        if self.schemas.contains_key(name) {
            return Err(RecoveryError::new(ErrorKind::SchemaExists));
        }
        self.schemas.insert(name.to_owned(), Schema::default());
        Ok(())
    }

    fn schema_mut(&mut self, name: &str) -> RecoveryResult<&mut Schema> {
        self.schemas
            .get_mut(name)
            .ok_or_else(|| RecoveryError::new(ErrorKind::DatabaseNotFound))
    }

    fn create_table(
        &mut self,
        schema: &str,
        name: &str,
        temporary: TemporaryTable,
    ) -> RecoveryResult<()> {
        let schema = self.schema_mut(schema)?;
        if schema.tables.contains_key(name) {
            return Err(RecoveryError::new(ErrorKind::TableExists));
        }
        schema.tables.insert(name.to_owned(), Table::new(temporary));
        Ok(())
    }

    fn insert(&mut self, schema: &str, table: &str, values: &[i64]) -> RecoveryResult<()> {
        self.schema_mut(schema)?
            .tables
            .get_mut(table)
            .ok_or_else(|| RecoveryError::new(ErrorKind::TableNotFound))?
            .insert(values);
        Ok(())
    }

    fn rows(&self, schema: &str, table: &str) -> RecoveryResult<Vec<i64>> {
        Ok(self
            .schemas
            .get(schema)
            .ok_or_else(|| RecoveryError::new(ErrorKind::DatabaseNotFound))?
            .tables
            .get(table)
            .ok_or_else(|| RecoveryError::new(ErrorKind::TableNotFound))?
            .rows
            .iter()
            .map(|(value, _)| *value)
            .collect())
    }

    fn row_ids(&self, schema: &str, table: &str) -> RecoveryResult<Vec<u64>> {
        Ok(self
            .schemas
            .get(schema)
            .ok_or_else(|| RecoveryError::new(ErrorKind::DatabaseNotFound))?
            .tables
            .get(table)
            .ok_or_else(|| RecoveryError::new(ErrorKind::TableNotFound))?
            .rows
            .iter()
            .map(|(_, row_id)| *row_id)
            .collect())
    }

    fn delete_values_above(&mut self, schema: &str, table: &str, limit: i64) -> RecoveryResult<()> {
        self.schema_mut(schema)?
            .tables
            .get_mut(table)
            .ok_or_else(|| RecoveryError::new(ErrorKind::TableNotFound))?
            .rows
            .retain(|(value, _)| *value <= limit);
        Ok(())
    }

    fn drop_table(&mut self, schema: &str, name: &str) -> RecoveryResult<u64> {
        let table = self
            .schema_mut(schema)?
            .tables
            .remove(name)
            .ok_or_else(|| RecoveryError::new(ErrorKind::TableNotFound))?;
        let job_id = self.next_job_id;
        self.next_job_id += 1;
        self.dropped_tables.push(DroppedTable {
            schema: schema.to_owned(),
            name: name.to_owned(),
            table,
            timestamp: self.now,
            job_id,
            recovered: false,
        });
        Ok(job_id)
    }

    fn truncate_table(&mut self, schema: &str, name: &str) -> RecoveryResult<u64> {
        let snapshot = self
            .schema_mut(schema)?
            .tables
            .get(name)
            .ok_or_else(|| RecoveryError::new(ErrorKind::TableNotFound))?;
        let snapshot = snapshot.clone();
        let job_id = self.next_job_id;
        self.next_job_id += 1;
        self.dropped_tables.push(DroppedTable {
            schema: schema.to_owned(),
            name: name.to_owned(),
            table: snapshot,
            timestamp: self.now,
            job_id,
            recovered: false,
        });
        let table = self
            .schema_mut(schema)?
            .tables
            .get_mut(name)
            .expect("table exists after snapshot");
        table.rows.clear();
        table.next_row_id = 1;
        Ok(job_id)
    }

    fn rename_table(&mut self, schema: &str, from: &str, to: &str) -> RecoveryResult<()> {
        let schema = self.schema_mut(schema)?;
        if schema.tables.contains_key(to) {
            return Err(RecoveryError::new(ErrorKind::TableExists));
        }
        let table = schema
            .tables
            .remove(from)
            .ok_or_else(|| RecoveryError::new(ErrorKind::TableNotFound))?;
        schema.tables.insert(to.to_owned(), table);
        Ok(())
    }

    fn check_history(&self, timestamp: u64) -> RecoveryResult<()> {
        // 安全点不存在时无法判断历史是否可读；安全点越过快照则历史已被 GC 淘汰。
        match self.safe_point {
            None => Err(RecoveryError::new(ErrorKind::NoSafePoint)),
            Some(safe_point) if safe_point > timestamp => {
                Err(RecoveryError::new(ErrorKind::TooOld))
            }
            Some(_) => Ok(()),
        }
    }

    fn restore_table(
        &mut self,
        schema: &str,
        source: &str,
        destination: &str,
        permissions: Permissions,
        job_id: Option<u64>,
        force_rebase: bool,
    ) -> RecoveryResult<()> {
        // 按作业恢复必须精确命中指定历史；按名称恢复则选择最后一次删除或截断快照。
        if !permissions.table_recovery() {
            return Err(RecoveryError::new(ErrorKind::AccessDenied));
        }
        if destination.trim().is_empty() {
            return Err(RecoveryError::new(ErrorKind::InvalidTableName));
        }
        let index = if let Some(job_id) = job_id {
            self.dropped_tables
                .iter()
                .position(|snapshot| snapshot.job_id == job_id)
                .ok_or_else(|| {
                    RecoveryError::new(if job_id == 0 {
                        ErrorKind::InvalidJob
                    } else {
                        ErrorKind::UnknownJob
                    })
                })?
        } else {
            self.dropped_tables
                .iter()
                .rposition(|snapshot| snapshot.schema == schema && snapshot.name == source)
                .ok_or_else(|| RecoveryError::new(ErrorKind::TableNotFound))?
        };
        let snapshot = self.dropped_tables[index].clone();
        if snapshot.schema != schema || snapshot.name != source {
            return Err(RecoveryError::new(ErrorKind::UnknownJob));
        }
        if snapshot.recovered {
            return Err(RecoveryError::new(if job_id.is_some() {
                ErrorKind::AlreadyRecovered
            } else {
                ErrorKind::TableExists
            }));
        }
        if self
            .schemas
            .get(schema)
            .and_then(|schema| schema.tables.get(destination))
            .is_some()
        {
            return Err(RecoveryError::new(ErrorKind::TableExists));
        }
        if snapshot.table.temporary == TemporaryTable::Global {
            return Err(RecoveryError::new(ErrorKind::UnsupportedTemporaryTable));
        }
        if snapshot.table.temporary == TemporaryTable::Local {
            return Err(RecoveryError::new(ErrorKind::TableNotFound));
        }
        self.check_history(snapshot.timestamp)?;
        if self.update_version_error {
            return Err(RecoveryError::new(ErrorKind::UpdateVersionFailed));
        }
        let mut table = snapshot.table.clone();
        table.rebase_for_restore(force_rebase);
        self.dropped_tables[index].recovered = true;
        self.schemas
            .get_mut(schema)
            .expect("source schema exists")
            .tables
            .insert(destination.to_owned(), table);
        Ok(())
    }

    fn recover_table(
        &mut self,
        schema: &str,
        name: &str,
        permissions: Permissions,
    ) -> RecoveryResult<()> {
        self.restore_table(schema, name, name, permissions, None, false)
    }

    fn recover_table_by_job(
        &mut self,
        schema: &str,
        job_id: u64,
        permissions: Permissions,
    ) -> RecoveryResult<()> {
        let source = self
            .dropped_tables
            .iter()
            .find(|snapshot| snapshot.job_id == job_id)
            .map(|snapshot| snapshot.name.clone())
            .ok_or_else(|| {
                RecoveryError::new(if job_id == 0 {
                    ErrorKind::InvalidJob
                } else {
                    ErrorKind::UnknownJob
                })
            })?;
        self.restore_table(schema, &source, &source, permissions, Some(job_id), false)
    }

    fn flashback_table(
        &mut self,
        schema: &str,
        source: &str,
        destination: Option<&str>,
        permissions: Permissions,
    ) -> RecoveryResult<()> {
        let destination = destination.unwrap_or(source);
        self.restore_table(schema, source, destination, permissions, None, true)
    }

    fn drop_schema(&mut self, name: &str) -> RecoveryResult<()> {
        let schema = self
            .schemas
            .remove(name)
            .ok_or_else(|| RecoveryError::new(ErrorKind::DatabaseNotFound))?;
        self.dropped_schemas.push(DroppedSchema {
            name: name.to_owned(),
            schema,
            timestamp: self.now,
            recovered: false,
        });
        Ok(())
    }

    fn flashback_schema(
        &mut self,
        source: &str,
        destination: Option<&str>,
        permissions: Permissions,
    ) -> RecoveryResult<()> {
        if !permissions.schema_recovery() {
            return Err(RecoveryError::new(ErrorKind::AccessDenied));
        }
        // Go 在查找 DDL 历史前读取 GC safe point，因此 safe point 缺失优先于“库不存在”。
        if self.safe_point.is_none() {
            return Err(RecoveryError::new(ErrorKind::NoSafePoint));
        }
        let destination = destination.unwrap_or(source);
        if self.schemas.contains_key(destination) {
            return Err(RecoveryError::new(ErrorKind::SchemaExists));
        }
        let index = self
            .dropped_schemas
            .iter()
            .rposition(|snapshot| snapshot.name == source)
            .ok_or_else(|| RecoveryError::new(ErrorKind::DatabaseNotFound))?;
        let snapshot = self.dropped_schemas[index].clone();
        if snapshot.recovered {
            return Err(RecoveryError::new(ErrorKind::SchemaExists));
        }
        self.check_history(snapshot.timestamp)?;
        self.dropped_schemas[index].recovered = true;
        self.schemas
            .insert(destination.to_owned(), snapshot.schema.clone());
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 模拟测试期间关闭 GC、设置删除前后安全点，并在结束时恢复原始开关。
struct MockGc {
    origin_enabled: bool,
    enabled: bool,
    before_drop: u64,
    after_drop: u64,
}

impl MockGc {
    fn new(origin_enabled: bool) -> Self {
        Self {
            origin_enabled,
            enabled: false,
            before_drop: 50,
            after_drop: 150,
        }
    }

    fn reset(&mut self) {
        self.enabled = self.origin_enabled;
    }

    fn safe_point_sql(&self, timestamp: u64) -> String {
        format!(
            "INSERT HIGH_PRIORITY INTO mysql.tidb VALUES ('tikv_gc_safe_point', '{timestamp}', '') ON DUPLICATE KEY UPDATE variable_value = '{timestamp}'"
        )
    }
}

fn mock_gc(catalog: &mut Catalog, origin_enabled: bool) -> MockGc {
    // 删除表后立即回收会让恢复历史失效，因此准备夹具时先关闭模拟 GC。
    catalog.gc_enabled = false;
    MockGc::new(origin_enabled)
}

fn table_fixture() -> Catalog {
    let mut catalog = Catalog::default();
    catalog.create_schema("test").unwrap();
    catalog
}

fn assert_kind<T: std::fmt::Debug>(result: RecoveryResult<T>, expected: ErrorKind) {
    assert_eq!(result.unwrap_err().0, expected);
}

fn validate_cluster_timestamp(
    flashback_ts: u64,
    current_ts: u64,
    gc_safe_point: Option<u64>,
    active_transaction: Option<u64>,
    permissions: Permissions,
    tikv: bool,
) -> RecoveryResult<()> {
    // 校验顺序与 Go 路径一致，使多个条件同时不满足时仍返回同一种首要错误。
    if !tikv {
        return Err(RecoveryError::new(ErrorKind::NonTikvCluster));
    }
    if !permissions.super_user {
        return Err(RecoveryError::new(ErrorKind::AccessDenied));
    }
    if flashback_ts >= current_ts {
        return Err(RecoveryError::new(ErrorKind::FutureTimestamp));
    }
    if gc_safe_point.is_none() {
        return Err(RecoveryError::new(ErrorKind::NoSafePoint));
    }
    if flashback_ts < gc_safe_point.unwrap() {
        return Err(RecoveryError::new(ErrorKind::BeforeGcSafePoint));
    }
    if active_transaction.is_some_and(|start| start > flashback_ts) {
        return Err(RecoveryError::new(ErrorKind::ActiveTransaction));
    }
    Ok(())
}

fn compare_flashback_and_safe_ts(flashback_ts: u64, safe_ts: u64) -> i8 {
    // 返回值沿用 Go 测试的约定：闪回时间更早为 -1，更晚为 1。
    match flashback_ts.cmp(&safe_ts) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

fn validate_cluster_guards(
    system_table_modified: bool,
    tidb_upgrade_detected: bool,
) -> RecoveryResult<()> {
    if system_table_modified {
        return Err(RecoveryError::new(ErrorKind::ModifiedSystemTable));
    }
    if tidb_upgrade_detected {
        return Err(RecoveryError::new(ErrorKind::TiDbUpgradeDetected));
    }
    Ok(())
}

fn retry_min_safe_time(flashback_ts: u64, safe_points: &[u64]) -> (usize, RecoveryResult<()>) {
    // 每个元素代表一次拉取结果；直到全局最小安全时间追上闪回点才停止重试。
    for (attempt, safe_point) in safe_points.iter().enumerate() {
        if *safe_point >= flashback_ts {
            return (attempt + 1, Ok(()));
        }
    }
    (
        safe_points.len(),
        Err(RecoveryError::new(ErrorKind::TooOld)),
    )
}

#[test]
fn test_recover_table() {
    let mut catalog = table_fixture();
    let mut gc = mock_gc(&mut catalog, true);
    catalog
        .create_table("test", "t_recover", TemporaryTable::None)
        .unwrap();
    catalog.insert("test", "t_recover", &[1, 2, 3]).unwrap();
    catalog.drop_table("test", "t_recover").unwrap();
    assert_kind(
        catalog.recover_table("test", "t_recover", Permissions::root()),
        ErrorKind::NoSafePoint,
    );
    catalog.set_safe_point(Some(gc.before_drop));
    catalog
        .recover_table("test", "t_recover", Permissions::root())
        .unwrap();
    assert_eq!(catalog.rows("test", "t_recover").unwrap(), vec![1, 2, 3]);
    assert_eq!(catalog.row_ids("test", "t_recover").unwrap(), vec![1, 2, 3]);

    catalog.drop_table("test", "t_recover").unwrap();
    catalog.set_safe_point(Some(gc.after_drop));
    assert_kind(
        catalog.recover_table("test", "t_recover", Permissions::root()),
        ErrorKind::TooOld,
    );
    catalog.set_safe_point(Some(gc.before_drop));
    catalog
        .create_table("test", "t_recover", TemporaryTable::None)
        .unwrap();
    assert_kind(
        catalog.recover_table("test", "t_recover", Permissions::root()),
        ErrorKind::TableExists,
    );
    catalog
        .rename_table("test", "t_recover", "t_recover2")
        .unwrap();
    catalog
        .recover_table("test", "t_recover", Permissions::root())
        .unwrap();
    catalog.insert("test", "t_recover", &[4, 5, 6]).unwrap();
    assert_eq!(
        catalog.row_ids("test", "t_recover").unwrap(),
        vec![1, 2, 3, 5001, 5002, 5003]
    );

    assert_kind(
        catalog.recover_table_by_job("test", 10_000_000, Permissions::root()),
        ErrorKind::UnknownJob,
    );
    assert_kind(
        catalog.recover_table_by_job("test", 0, Permissions::root()),
        ErrorKind::InvalidJob,
    );
    catalog.gc_enabled = false;
    catalog.delete_values_above("test", "t_recover", 1).unwrap();
    catalog.set_time(200);
    catalog.drop_table("test", "t_recover").unwrap();
    catalog
        .recover_table("test", "t_recover", Permissions::root())
        .unwrap();
    catalog.insert("test", "t_recover", &[7, 8, 9]).unwrap();
    catalog.truncate_table("test", "t_recover").unwrap();
    catalog
        .rename_table("test", "t_recover", "t_recover_new")
        .unwrap();
    catalog
        .recover_table("test", "t_recover", Permissions::root())
        .unwrap();
    catalog.insert("test", "t_recover", &[10]).unwrap();
    assert_eq!(
        catalog.rows("test", "t_recover").unwrap(),
        vec![1, 7, 8, 9, 10]
    );

    catalog.drop_table("test", "t_recover").unwrap();
    catalog
        .flashback_table(
            "test",
            "t_recover",
            Some("t_recover_tmp"),
            Permissions::root(),
        )
        .unwrap();
    assert_kind(
        catalog.recover_table("test", "t_recover", Permissions::root()),
        ErrorKind::TableExists,
    );
    let job_id = {
        let _ = catalog.drop_table("test", "t_recover2");
        catalog
            .create_table("test", "t_recover2", TemporaryTable::None)
            .unwrap();
        catalog.drop_table("test", "t_recover2").unwrap()
    };
    catalog
        .recover_table_by_job("test", job_id, Permissions::root())
        .unwrap();
    assert_kind(
        catalog.recover_table_by_job("test", job_id, Permissions::root()),
        ErrorKind::AlreadyRecovered,
    );
    // RECOVER 不得覆盖用户手工关闭的 mysql.tidb GC 开关；夹具复位只恢复 emulator GC。
    assert!(!catalog.gc_enabled);
    gc.reset();
    assert!(gc.enabled);
    assert!(!catalog.gc_enabled);
}

#[test]
fn test_flashback_table() {
    let mut catalog = table_fixture();
    let gc = mock_gc(&mut catalog, true);
    catalog
        .create_table("test", "t_flashback", TemporaryTable::None)
        .unwrap();
    catalog.insert("test", "t_flashback", &[1, 2, 3]).unwrap();
    catalog.drop_table("test", "t_flashback").unwrap();
    catalog.set_safe_point(Some(gc.before_drop));
    assert_kind(
        catalog.flashback_table("test", "t_not_exists", None, Permissions::root()),
        ErrorKind::TableNotFound,
    );
    catalog
        .create_table("test", "t_flashback", TemporaryTable::None)
        .unwrap();
    assert_kind(
        catalog.flashback_table("test", "t_flashback", None, Permissions::root()),
        ErrorKind::TableExists,
    );
    catalog
        .rename_table("test", "t_flashback", "t_flashback_tmp")
        .unwrap();
    catalog
        .flashback_table("test", "t_flashback", None, Permissions::root())
        .unwrap();
    catalog.insert("test", "t_flashback", &[4, 5, 6]).unwrap();
    assert_eq!(
        catalog.row_ids("test", "t_flashback").unwrap(),
        vec![1, 2, 3, 5001, 5002, 5003]
    );
    catalog.drop_table("test", "t_flashback").unwrap();
    catalog
        .create_table("test", "t_flashback", TemporaryTable::None)
        .unwrap();
    assert_kind(
        catalog.flashback_table("test", "t_flashback", Some(" "), Permissions::root()),
        ErrorKind::InvalidTableName,
    );
    catalog
        .flashback_table(
            "test",
            "t_flashback",
            Some("t_flashback2"),
            Permissions::root(),
        )
        .unwrap();
    catalog.insert("test", "t_flashback2", &[7, 8, 9]).unwrap();
    assert_eq!(
        catalog.row_ids("test", "t_flashback2").unwrap()[6..],
        [10001, 10002, 10003]
    );
    assert_kind(
        catalog.flashback_table(
            "test",
            "t_flashback",
            Some("t_flashback4"),
            Permissions::root(),
        ),
        ErrorKind::TableExists,
    );
    catalog.truncate_table("test", "t_flashback2").unwrap();
    catalog
        .flashback_table(
            "test",
            "t_flashback2",
            Some("t_flashback3"),
            Permissions::root(),
        )
        .unwrap();
    catalog.insert("test", "t_flashback3", &[10, 11]).unwrap();
    assert_eq!(
        catalog.rows("test", "t_flashback3").unwrap(),
        (1..=11).collect::<Vec<_>>()
    );
    assert_eq!(
        catalog.row_ids("test", "t_flashback3").unwrap()[9..],
        [15001, 15002]
    );

    catalog
        .create_table("test", "t_p_flashback", TemporaryTable::None)
        .unwrap();
    catalog.insert("test", "t_p_flashback", &[1, 2, 3]).unwrap();
    catalog.drop_table("test", "t_p_flashback").unwrap();
    catalog
        .flashback_table("test", "t_p_flashback", None, Permissions::root())
        .unwrap();
    catalog.insert("test", "t_p_flashback", &[4, 5]).unwrap();
    catalog.truncate_table("test", "t_p_flashback").unwrap();
    catalog
        .flashback_table(
            "test",
            "t_p_flashback",
            Some("t_p_flashback1"),
            Permissions::root(),
        )
        .unwrap();
    catalog.insert("test", "t_p_flashback1", &[6]).unwrap();
    assert_eq!(
        catalog.rows("test", "t_p_flashback1").unwrap(),
        vec![1, 2, 3, 4, 5, 6]
    );

    catalog.create_schema("Test2").unwrap();
    catalog
        .create_table("Test2", "t", TemporaryTable::None)
        .unwrap();
    catalog.insert("Test2", "t", &[1, 2]).unwrap();
    catalog.drop_table("Test2", "t").unwrap();
    catalog.create_schema("Test3").unwrap();
    catalog
        .create_table("Test3", "t", TemporaryTable::None)
        .unwrap();
    catalog.drop_table("Test3", "t").unwrap();
    catalog.drop_schema("Test3").unwrap();
    catalog
        .flashback_table("Test2", "t", None, Permissions::root())
        .unwrap();
    catalog.insert("Test2", "t", &[3]).unwrap();
    assert_eq!(catalog.rows("Test2", "t").unwrap(), vec![1, 2, 3]);
}

#[test]
fn test_recover_temp_table() {
    let mut catalog = table_fixture();
    let gc = mock_gc(&mut catalog, false);
    catalog
        .create_table("test", "global_tmp", TemporaryTable::Global)
        .unwrap();
    catalog.drop_table("test", "global_tmp").unwrap();
    catalog
        .create_table("test", "local_tmp", TemporaryTable::Local)
        .unwrap();
    catalog.drop_table("test", "local_tmp").unwrap();
    catalog.set_safe_point(Some(gc.before_drop));
    assert_kind(
        catalog.recover_table("test", "global_tmp", Permissions::root()),
        ErrorKind::UnsupportedTemporaryTable,
    );
    assert_kind(
        catalog.flashback_table("test", "global_tmp", None, Permissions::root()),
        ErrorKind::UnsupportedTemporaryTable,
    );
    assert_kind(
        catalog.recover_table("test", "local_tmp", Permissions::root()),
        ErrorKind::TableNotFound,
    );
    assert_kind(
        catalog.flashback_table("test", "local_tmp", None, Permissions::root()),
        ErrorKind::TableNotFound,
    );
}

#[test]
fn test_recover_table_meet_error() {
    let mut catalog = table_fixture();
    let gc = mock_gc(&mut catalog, true);
    catalog
        .create_table("test", "t_recover", TemporaryTable::None)
        .unwrap();
    catalog.insert("test", "t_recover", &[1, 2, 3]).unwrap();
    catalog.drop_table("test", "t_recover").unwrap();
    catalog.set_safe_point(Some(gc.before_drop));
    catalog
        .recover_table("test", "t_recover", Permissions::root())
        .unwrap();
    assert_eq!(catalog.rows("test", "t_recover").unwrap(), vec![1, 2, 3]);
    catalog.drop_table("test", "t_recover").unwrap();
    catalog.update_version_error = true;
    assert_kind(
        catalog.recover_table("test", "t_recover", Permissions::root()),
        ErrorKind::UpdateVersionFailed,
    );
    assert_kind(catalog.rows("test", "t_recover"), ErrorKind::TableNotFound);
}

#[test]
fn test_recover_table_privilege() {
    let mut catalog = table_fixture();
    let gc = mock_gc(&mut catalog, false);
    catalog
        .create_table("test", "t_recover", TemporaryTable::None)
        .unwrap();
    catalog.drop_table("test", "t_recover").unwrap();
    catalog.set_safe_point(Some(gc.before_drop));
    assert_kind(
        catalog.recover_table("test", "t_recover", Permissions::default()),
        ErrorKind::AccessDenied,
    );
    assert_kind(
        catalog.flashback_table(
            "test",
            "t_recover",
            None,
            Permissions {
                drop: true,
                ..Permissions::default()
            },
        ),
        ErrorKind::AccessDenied,
    );
    catalog
        .recover_table("test", "t_recover", Permissions::root())
        .unwrap();
    catalog.drop_table("test", "t_recover").unwrap();
    catalog
        .flashback_table("test", "t_recover", None, Permissions::root())
        .unwrap();
    assert_eq!(
        catalog.rows("test", "t_recover").unwrap(),
        Vec::<i64>::new()
    );
}

#[test]
fn test_recover_cluster_meet_error() {
    let root = Permissions::root();
    assert_kind(
        validate_cluster_timestamp(100, 200, Some(90), None, root, false),
        ErrorKind::NonTikvCluster,
    );
    assert_kind(
        validate_cluster_timestamp(200, 200, Some(90), None, root, true),
        ErrorKind::FutureTimestamp,
    );
    assert_kind(
        validate_cluster_timestamp(80, 200, None, None, root, true),
        ErrorKind::NoSafePoint,
    );
    assert_kind(
        validate_cluster_timestamp(80, 200, Some(90), None, root, true),
        ErrorKind::BeforeGcSafePoint,
    );
    assert_kind(
        validate_cluster_timestamp(100, 200, Some(90), None, Permissions::default(), true),
        ErrorKind::AccessDenied,
    );
    assert_kind(
        validate_cluster_timestamp(100, 200, Some(90), Some(150), root, true),
        ErrorKind::ActiveTransaction,
    );
    assert_kind(
        validate_cluster_guards(true, false),
        ErrorKind::ModifiedSystemTable,
    );
    assert_kind(
        validate_cluster_guards(false, true),
        ErrorKind::TiDbUpgradeDetected,
    );
    validate_cluster_guards(false, false).unwrap();
    validate_cluster_timestamp(100, 200, Some(90), None, root, true).unwrap();
}

#[test]
fn test_flashback_with_safe_ts() {
    let flashback_ts = 100;
    assert_eq!(compare_flashback_and_safe_ts(flashback_ts, 100), 0);
    assert_eq!(compare_flashback_and_safe_ts(flashback_ts, 110), -1);
    assert_eq!(compare_flashback_and_safe_ts(flashback_ts, 90), 1);
    for safe_ts in [100, 110] {
        assert!(retry_min_safe_time(flashback_ts, &[safe_ts]).1.is_ok());
    }
    assert_kind(
        retry_min_safe_time(flashback_ts, &[90]).1,
        ErrorKind::TooOld,
    );
}

#[test]
fn test_flashback_tso_with_safe_ts() {
    // TSO 与时间字符串两种输入共用同一套安全时间比较约定。
    for (flashback_ts, safe_ts, expected) in [(100, 100, 0), (100, 110, -1), (100, 90, 1)] {
        assert_eq!(
            compare_flashback_and_safe_ts(flashback_ts, safe_ts),
            expected
        );
    }
}

#[test]
fn test_flashback_retry_get_min_safe_time() {
    let (attempts, result) = retry_min_safe_time(100, &[90, 95, 110]);
    assert_eq!(attempts, 3);
    result.unwrap();
    let (attempts, result) = retry_min_safe_time(100, &[90]);
    assert_eq!(attempts, 1);
    assert_kind(result, ErrorKind::TooOld);
}

#[test]
fn test_flashback_schema() {
    let mut catalog = table_fixture();
    let gc = mock_gc(&mut catalog, true);
    catalog.create_schema("test1").unwrap();
    catalog
        .create_table("test1", "t", TemporaryTable::None)
        .unwrap();
    catalog
        .create_table("test1", "t1", TemporaryTable::None)
        .unwrap();
    catalog.insert("test1", "t", &[1, 2, 3]).unwrap();
    catalog.insert("test1", "t1", &[4, 5, 6]).unwrap();
    catalog.drop_schema("test1").unwrap();
    assert_kind(
        catalog.flashback_schema("db_not_exists", None, Permissions::root()),
        ErrorKind::NoSafePoint,
    );
    catalog.set_safe_point(Some(gc.before_drop));
    assert_kind(
        catalog.flashback_schema("db_not_exists", None, Permissions::root()),
        ErrorKind::DatabaseNotFound,
    );
    catalog
        .flashback_schema("test1", None, Permissions::root())
        .unwrap();
    assert_eq!(catalog.rows("test1", "t").unwrap(), vec![1, 2, 3]);
    assert_kind(
        catalog.flashback_schema("test1", Some("test_flashback2"), Permissions::root()),
        ErrorKind::SchemaExists,
    );
    catalog.drop_schema("test1").unwrap();
    catalog
        .flashback_schema("test1", Some("test2"), Permissions::root())
        .unwrap();
    assert_eq!(catalog.rows("test2", "t1").unwrap(), vec![4, 5, 6]);
    assert_kind(
        catalog.flashback_schema("test1", None, Permissions::root()),
        ErrorKind::SchemaExists,
    );

    catalog.create_schema("db_flashback").unwrap();
    assert_kind(
        catalog.flashback_schema("db_flashback", None, Permissions::root()),
        ErrorKind::SchemaExists,
    );

    catalog.create_schema("t_recover").unwrap();
    catalog.drop_schema("t_recover").unwrap();
    assert_kind(
        catalog.flashback_schema("t_recover", None, Permissions::default()),
        ErrorKind::AccessDenied,
    );
    assert_kind(
        catalog.flashback_schema(
            "t_recover",
            None,
            Permissions {
                drop: true,
                ..Permissions::default()
            },
        ),
        ErrorKind::AccessDenied,
    );
    catalog
        .flashback_schema(
            "t_recover",
            None,
            Permissions {
                create: true,
                drop: true,
                ..Permissions::default()
            },
        )
        .unwrap();
}

#[test]
fn test_flashback_schema_with_many_tables() {
    let catalog = Arc::new(Mutex::new(table_fixture()));
    catalog
        .lock()
        .unwrap()
        .create_schema("many_tables")
        .unwrap();
    let mut workers = Vec::new();
    for worker in 0..10 {
        let catalog = Arc::clone(&catalog);
        workers.push(thread::spawn(move || {
            for table in 0..70 {
                catalog
                    .lock()
                    .unwrap()
                    .create_table(
                        "many_tables",
                        &format!("t_{worker}_{table}"),
                        TemporaryTable::None,
                    )
                    .unwrap();
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(
        catalog.lock().unwrap().schemas["many_tables"].tables.len(),
        700
    );
    let mut catalog = Arc::try_unwrap(catalog).unwrap().into_inner().unwrap();
    catalog.set_safe_point(Some(50));
    catalog.drop_schema("many_tables").unwrap();
    catalog
        .flashback_schema("many_tables", None, Permissions::root())
        .unwrap();
    assert_eq!(catalog.schemas["many_tables"].tables.len(), 700);
}

#[test]
fn test_flashback_cluster_with_many_d_bs() {
    let catalog = Arc::new(Mutex::new(table_fixture()));
    let mut workers = Vec::new();
    for worker in 0..40 {
        let catalog = Arc::clone(&catalog);
        workers.push(thread::spawn(move || {
            for db in 0..10 {
                catalog
                    .lock()
                    .unwrap()
                    .create_schema(&format!("db_{}", worker * 10 + db))
                    .unwrap();
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(catalog.lock().unwrap().schemas.len(), 401); // 初始 `test` 库加 400 个并发创建的库。
    validate_cluster_timestamp(100, 200, Some(90), None, Permissions::root(), true).unwrap();
}

#[test]
fn mock_gc_restores_original_state_and_formats_safepoint() {
    let mut catalog = table_fixture();
    let mut gc = mock_gc(&mut catalog, true);
    assert!(!gc.enabled);
    assert!(
        gc.safe_point_sql(gc.before_drop)
            .contains("tikv_gc_safe_point")
    );
    gc.reset();
    assert!(gc.enabled);
}

#[test]
fn flashback_timestamp_validation_matches_gc_and_active_transaction_rules() {
    let root = Permissions::root();
    validate_cluster_timestamp(100, 200, Some(90), None, root, true).unwrap();
    assert_kind(
        validate_cluster_timestamp(200, 200, Some(90), None, root, true),
        ErrorKind::FutureTimestamp,
    );
    assert_kind(
        validate_cluster_timestamp(80, 200, Some(90), None, root, true),
        ErrorKind::BeforeGcSafePoint,
    );
    assert_kind(
        validate_cluster_timestamp(100, 200, Some(90), Some(150), root, true),
        ErrorKind::ActiveTransaction,
    );
}
