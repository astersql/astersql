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

// 外键（Foreign Key）检查与级联（Cascade）执行框架。
//
// 在 DML 路径上：
// - `FKCheckExec`：校验子表引用存在 / 父表删除时无残留引用；
// - `FKCascadeExec`：按 ON DELETE/UPDATE 生成并执行级联 DELETE/UPDATE。
//
// 外键：子表列必须匹配父表被引用列；级联在父行变更时自动改子行。
// 悲观事务下可将待锁 key 延迟到统一加锁阶段。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use astersql_errors as errors;

/// 外键路径统一错误结果别名。
pub type FKResult<T = ()> = Result<T, errors::SharedError>;
/// 编码后的 TiKV key。
pub type Key = Vec<u8>;

/// 单次级联语句最多处理的外键值组数，避免超大 IN 列表。
const MAX_HANDLE_FK_VALUE_IN_ONE_CASCADE: usize = 1024;
/// 运行时统计类型：外键检查。
pub const TP_FK_CHECK_RUNTIME_STATS: i32 = 1;
/// 运行时统计类型：外键级联。
pub const TP_FK_CASCADE_RUNTIME_STATS: i32 = 2;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
/// 简化的列值表示，用于外键值抽取与编码。
pub enum Datum {
    Null,
    Int64(i64),
    Uint64(u64),
    Bytes(Vec<u8>),
    String(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 列名及其在行中的偏移。
pub struct ColumnInfo {
    pub name: String,
    pub offset: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 用于定位引用的索引元数据。
pub struct IndexInfo {
    pub id: i64,
    pub name: String,
    pub columns: Vec<String>,
    pub primary: bool,
    pub unique: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 外键检查/级联涉及的表元数据。
/// `common_handle`：非整型主键时用多列编码的句柄。
pub struct TableInfo {
    pub id: i64,
    pub schema: String,
    pub name: String,
    pub columns: Vec<ColumnInfo>,
    pub record_prefix: Key,
    pub common_handle: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 检查方向：子表引用父表，或父表被引用约束。
pub enum FKDirection {
    ChildReference,
    ParentReference,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 一条外键检查规格：列、目标表/索引、存在性语义与失败信息。
pub struct FKCheckSpec {
    pub id: i64,
    pub direction: FKDirection,
    pub columns: Vec<String>,
    pub table: TableInfo,
    pub index: Option<IndexInfo>,
    pub idx_is_primary_key: bool,
    pub idx_is_exclusive: bool,
    pub check_exist: bool,
    pub failed_error: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// SQL 引用动作：CASCADE / SET NULL / RESTRICT / NO ACTION。
pub enum ReferOption {
    Cascade,
    SetNull,
    Restrict,
    NoAction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 级联触发类型：删除或更新父行。
pub enum FKCascadeType {
    OnDelete,
    OnUpdate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 外键上声明的 ON DELETE / ON UPDATE 动作。
pub struct FKInfo {
    pub on_delete: ReferOption,
    pub on_update: ReferOption,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 一条级联规格：子表、外键列与被引用列映射。
pub struct FKCascadeSpec {
    pub id: i64,
    pub cascade_type: FKCascadeType,
    pub referred_schema: String,
    pub child_table: TableInfo,
    pub foreign_key: FKInfo,
    pub foreign_key_columns: Vec<ColumnInfo>,
    pub foreign_key_index: Option<IndexInfo>,
    pub referred_columns: Vec<String>,
}

/// 执行器可挂载的外键检查与级联列表。
pub trait WithForeignKeyTrigger {
    fn GetFKChecks(&self) -> &[FKCheckExec];
    fn GetFKCascades(&self) -> &[FKCascadeExec];
    fn HasFKCascades(&self) -> bool;
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 外键检查阶段耗时与 key 计数。
pub struct FKCheckRuntimeStats {
    pub Total: Duration,
    pub Check: Duration,
    pub Lock: Duration,
    pub Keys: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 外键级联阶段耗时与 key 计数。
pub struct FKCascadeRuntimeStats {
    pub Total: Duration,
    pub Keys: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 检查或级联统计的统一包装。
pub enum FKRuntimeStats {
    Check(FKCheckRuntimeStats),
    Cascade(FKCascadeRuntimeStats),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 事务 Get 结果：键是否存在。
pub enum KeyState {
    Found,
    NotFound,
}

/// 外键检查所需的事务读写与前缀扫描能力。
pub trait FKTransaction: Send {
    fn IsPessimistic(&self) -> bool;
    fn BatchGet(&mut self, keys: &[Key]) -> FKResult;
    fn Get(&mut self, key: &[u8]) -> FKResult<KeyState>;
    fn ScanMemPrefix(&mut self, prefix: &[u8]) -> FKResult<Vec<(Key, Vec<u8>)>>;
    fn ScanSnapshotPrefix(&mut self, prefix: &[u8]) -> FKResult<Vec<(Key, Vec<u8>)>>;
    fn SetSnapshotScanBatchSize(&mut self, size: usize);
}

/// 外键检查运行时：编码 key、加锁、注册统计与警告。
pub trait FKCheckRuntime: Send + Sync {
    fn Transaction(&self, activate: bool) -> FKResult<Box<dyn FKTransaction>>;
    fn EncodeIndexKey(&self, index: &IndexInfo, values: &[Datum]) -> FKResult<(Key, bool)>;
    fn EncodeCommonHandle(&self, values: &[Datum]) -> FKResult<Key>;
    fn EncodeRecordKey(&self, record_prefix: &[u8], handle: &[u8]) -> Key;
    fn DecodeIndexHandle(
        &self,
        key: &[u8],
        value: &[u8],
        index_column_count: usize,
    ) -> FKResult<Key>;
    fn AddUnchangedKeysForLock(&self, keys: &[Key]);
    fn ForUpdateFlag(&self) -> u32;
    fn SetForUpdateFlag(&self, value: u32);
    fn LockKeys(&self, keys: &[Key]) -> FKResult;
    fn RuntimeStatsEnabled(&self) -> bool;
    fn RegisterCheckStats(&self, id: i64, stats: &FKCheckRuntimeStats);
    fn AppendWarning(&self, error: errors::SharedError);
}

/// 单条外键检查执行器：收集待查 key，批量校验并可加锁。
pub struct FKCheckExec {
    pub FKCheck: FKCheckSpec,
    pub fkValueHelper: fkValueHelper,
    pub toBeCheckedKeys: Vec<Key>,
    pub toBeCheckedPrefixKeys: Vec<Key>,
    pub toBeLockedKeys: Vec<Key>,
    pub checkRowsCache: HashMap<Key, bool>,
    pub stats: Option<FKCheckRuntimeStats>,
    runtime: Arc<dyn FKCheckRuntime>,
}

/// 按表 ID 批量构建外键检查执行器。
pub fn buildTblID2FKCheckExecs(
    runtime: Arc<dyn FKCheckRuntime>,
    tables: &HashMap<i64, TableInfo>,
    checks: &HashMap<i64, Vec<FKCheckSpec>>,
) -> FKResult<HashMap<i64, Vec<FKCheckExec>>> {
    let mut result = HashMap::new();
    for (table_id, table) in tables {
        let built = buildFKCheckExecs(
            Arc::clone(&runtime),
            table,
            checks.get(table_id).map_or(&[], Vec::as_slice),
        )?;
        if !built.is_empty() {
            result.insert(*table_id, built);
        }
    }
    Ok(result)
}

/// 为单表构建全部外键检查执行器。
pub fn buildFKCheckExecs(
    runtime: Arc<dyn FKCheckRuntime>,
    table: &TableInfo,
    checks: &[FKCheckSpec],
) -> FKResult<Vec<FKCheckExec>> {
    checks
        .iter()
        .map(|check| buildFKCheckExec(Arc::clone(&runtime), table, check.clone()))
        .collect()
}

/// 构建单个外键检查执行器并解析列偏移。
pub fn buildFKCheckExec(
    runtime: Arc<dyn FKCheckRuntime>,
    table: &TableInfo,
    mut check: FKCheckSpec,
) -> FKResult<FKCheckExec> {
    let offsets = getFKColumnsOffsets(table, &check.columns)?;
    check.table = table.clone();
    Ok(FKCheckExec {
        FKCheck: check,
        fkValueHelper: fkValueHelper {
            colsOffsets: offsets,
            fkValuesSet: HashSet::new(),
        },
        toBeCheckedKeys: Vec::new(),
        toBeCheckedPrefixKeys: Vec::new(),
        toBeLockedKeys: Vec::new(),
        checkRowsCache: HashMap::new(),
        stats: None,
        runtime,
    })
}

impl FKCheckExec {
    /// 插入行：仅子表引用方向需要检查新值是否在父表存在。
    pub fn insertRowNeedToCheck(&mut self, row: &[Datum]) -> FKResult {
        if self.FKCheck.direction == FKDirection::ParentReference {
            return Ok(());
        }
        self.addRowNeedToCheck(row)
    }

    /// 更新行：外键列变化时，子表查新值、父表查旧值。
    pub fn updateRowNeedToCheck(&mut self, old_row: &[Datum], new_row: &[Datum]) -> FKResult {
        let new_values = self.fkValueHelper.fetchFKValues(new_row)?;
        let old_values = self.fkValueHelper.fetchFKValues(old_row)?;
        if old_values == new_values {
            return Ok(());
        }
        match self.FKCheck.direction {
            FKDirection::ChildReference => self.addRowNeedToCheck(new_row),
            FKDirection::ParentReference => self.addRowNeedToCheck(old_row),
        }
    }

    /// 删除行：将行加入待检查集合。
    pub fn deleteRowNeedToCheck(&mut self, row: &[Datum]) -> FKResult {
        self.addRowNeedToCheck(row)
    }

    /// 抽取外键值并生成精确或前缀检查 key。
    pub fn addRowNeedToCheck(&mut self, row: &[Datum]) -> FKResult {
        let values = self.fkValueHelper.fetchFKValuesWithCheck(row)?;
        if values.is_empty() {
            return Ok(());
        }
        let (key, is_prefix) = self.buildCheckKeyFromFKValue(&values)?;
        if is_prefix {
            self.toBeCheckedPrefixKeys.push(key);
        } else {
            self.toBeCheckedKeys.push(key);
        }
        Ok(())
    }

    /// 执行批量检查：查 key / 前缀索引，必要时加锁。
    pub fn doCheck(&mut self) -> FKResult {
        let started = Instant::now();
        if self.runtime.RuntimeStatsEnabled() {
            self.stats = Some(FKCheckRuntimeStats::default());
        }
        let result = (|| {
            if self.toBeCheckedKeys.is_empty()
                && self.toBeCheckedPrefixKeys.is_empty()
                && self.toBeLockedKeys.is_empty()
            {
                return Ok(());
            }
            let mut transaction = self.runtime.Transaction(false)?;
            self.checkKeys(transaction.as_mut())?;
            self.checkIndexKeys(transaction.as_mut())?;
            if let Some(stats) = self.stats.as_mut() {
                stats.Check = started.elapsed();
            }
            if self.toBeLockedKeys.is_empty() {
                return Ok(());
            }
            // 悲观事务：延后统一加锁，避免此处立即 LockKeys。
            if transaction.IsPessimistic() {
                self.runtime.AddUnchangedKeysForLock(&self.toBeLockedKeys);
                return Ok(());
            }
            let lock_started = Instant::now();
            let for_update = self.runtime.ForUpdateFlag();
            let lock_result = self.runtime.LockKeys(&self.toBeLockedKeys);
            self.runtime.SetForUpdateFlag(for_update);
            if let Some(stats) = self.stats.as_mut() {
                stats.Lock = lock_started.elapsed();
            }
            lock_result
        })();
        if let Some(stats) = self.stats.as_mut() {
            stats.Keys = self.toBeCheckedKeys.len() + self.toBeCheckedPrefixKeys.len();
            stats.Total = started.elapsed();
            self.runtime.RegisterCheckStats(self.FKCheck.id, stats);
        }
        result
    }

    /// 由外键值编码检查 key；返回是否为前缀键。
    pub fn buildCheckKeyFromFKValue(&self, values: &[Datum]) -> FKResult<(Key, bool)> {
        if self.FKCheck.idx_is_primary_key {
            let handle = self.buildHandleFromFKValues(values)?;
            let key = self
                .runtime
                .EncodeRecordKey(&self.FKCheck.table.record_prefix, &handle);
            return Ok((key, !self.FKCheck.idx_is_exclusive));
        }
        let index = self
            .FKCheck
            .index
            .as_ref()
            .ok_or_else(|| errors::New("foreign-key index metadata is missing"))?;
        let (key, distinct) = self.runtime.EncodeIndexKey(index, values)?;
        Ok((key, !(distinct && self.FKCheck.idx_is_exclusive)))
    }

    /// 由主键外键值构建记录句柄。
    pub fn buildHandleFromFKValues(&self, values: &[Datum]) -> FKResult<Key> {
        if values.len() == 1 && self.FKCheck.index.is_none() {
            let Datum::Int64(handle) = values[0] else {
                return Err(errors::New("integer primary handle requires an i64 value"));
            };
            return Ok(handle.to_be_bytes().to_vec());
        }
        self.runtime.EncodeCommonHandle(values)
    }

    /// 预取后逐个检查精确 key。
    pub fn checkKeys(&mut self, transaction: &mut dyn FKTransaction) -> FKResult {
        if self.toBeCheckedKeys.is_empty() {
            return Ok(());
        }
        self.prefetchKeys(transaction, &self.toBeCheckedKeys)?;
        for key in self.toBeCheckedKeys.clone() {
            self.checkKey(transaction, &key)?;
        }
        Ok(())
    }

    /// 批量预取 key 到事务缓存。
    pub fn prefetchKeys(&self, transaction: &mut dyn FKTransaction, keys: &[Key]) -> FKResult {
        transaction.BatchGet(keys)
    }

    /// 按 `check_exist` 选择存在性或非存在性检查。
    pub fn checkKey(&mut self, transaction: &mut dyn FKTransaction, key: &[u8]) -> FKResult {
        if self.FKCheck.check_exist {
            self.checkKeyExist(transaction, key)
        } else {
            self.checkKeyNotExist(transaction, key)
        }
    }

    /// 键必须存在；存在则加入待锁列表。
    pub fn checkKeyExist(&mut self, transaction: &mut dyn FKTransaction, key: &[u8]) -> FKResult {
        match transaction.Get(key)? {
            KeyState::Found => {
                self.toBeLockedKeys.push(key.to_vec());
                Ok(())
            }
            KeyState::NotFound => Err(self.failedError()),
        }
    }

    /// 键必须不存在（父表删除/更新时无子引用）。
    pub fn checkKeyNotExist(&self, transaction: &mut dyn FKTransaction, key: &[u8]) -> FKResult {
        match transaction.Get(key)? {
            KeyState::Found => Err(self.failedError()),
            KeyState::NotFound => Ok(()),
        }
    }

    /// 对前缀索引键做小批量快照扫描检查。
    pub fn checkIndexKeys(&mut self, transaction: &mut dyn FKTransaction) -> FKResult {
        if self.toBeCheckedPrefixKeys.is_empty() {
            return Ok(());
        }
        transaction.SetSnapshotScanBatchSize(2);
        let keys = self.toBeCheckedPrefixKeys.clone();
        let result = keys
            .iter()
            .try_for_each(|key| self.checkPrefixKey(transaction, key));
        transaction.SetSnapshotScanBatchSize(256);
        result
    }

    /// 扫描前缀下首个有效索引项并按存在性语义判定。
    pub fn checkPrefixKey(
        &mut self,
        transaction: &mut dyn FKTransaction,
        prefix: &[u8],
    ) -> FKResult {
        let (key, value) = self.getIndexKeyValueInTable(transaction, prefix)?;
        if self.FKCheck.check_exist {
            return self.checkPrefixKeyExist(key.as_deref(), value.as_deref());
        }
        if value.as_ref().is_some_and(|value| !value.is_empty()) {
            return Err(self.failedError());
        }
        Ok(())
    }

    /// 前缀命中后解码句柄并加入记录键待锁列表。
    pub fn checkPrefixKeyExist(&mut self, key: Option<&[u8]>, value: Option<&[u8]>) -> FKResult {
        let (key, value) = match (key, value) {
            (Some(key), Some(value)) if !value.is_empty() => (key, value),
            _ => return Err(self.failedError()),
        };
        let index = self.FKCheck.index.as_ref();
        if index.is_some_and(|index| index.primary) && self.FKCheck.table.common_handle {
            self.toBeLockedKeys.push(key.to_vec());
            return Ok(());
        }
        let index = index.ok_or_else(|| errors::New("foreign-key index metadata is missing"))?;
        let handle = self
            .runtime
            .DecodeIndexHandle(key, value, index.columns.len())?;
        self.toBeLockedKeys.push(
            self.runtime
                .EncodeRecordKey(&self.FKCheck.table.record_prefix, &handle),
        );
        Ok(())
    }

    /// 先扫内存缓冲再扫快照，跳过已删除键，取前缀下首个存活索引项。
    pub fn getIndexKeyValueInTable(
        &self,
        transaction: &mut dyn FKTransaction,
        prefix: &[u8],
    ) -> FKResult<(Option<Key>, Option<Vec<u8>>)> {
        // 内存缓冲中的空 value 视为删除，快照扫描时跳过。
        let mut deleted = HashSet::new();
        for (key, value) in transaction.ScanMemPrefix(prefix)? {
            if !key.starts_with(prefix) {
                break;
            }
            if value.is_empty() {
                deleted.insert(key);
            } else {
                return Ok((Some(key), Some(value)));
            }
        }
        for (key, value) in transaction.ScanSnapshotPrefix(prefix)? {
            if !key.starts_with(prefix) {
                break;
            }
            if !deleted.contains(&key) {
                return Ok((Some(key), Some(value)));
            }
        }
        Ok((None, None))
    }

    /// 按行检查外键；失败时标记 ignore 并追加 warning（用于 INSERT IGNORE 等）。
    pub fn checkRows(
        &mut self,
        transaction: &mut dyn FKTransaction,
        rows: &mut [ToBeCheckedRow],
    ) -> FKResult {
        if rows.is_empty() {
            return Ok(());
        }
        let started = Instant::now();
        if self.runtime.RuntimeStatsEnabled() {
            self.stats = Some(FKCheckRuntimeStats::default());
        }
        let mut keys = Vec::with_capacity(rows.len());
        let mut prefetch = Vec::new();
        for row in rows.iter() {
            if row.ignored {
                keys.push(None);
                continue;
            }
            let values = self.fkValueHelper.fetchFKValues(&row.row)?;
            if self.fkValueHelper.hasNullValue(&values) {
                keys.push(None);
                continue;
            }
            let (key, is_prefix) = self.buildCheckKeyFromFKValue(&values)?;
            if !is_prefix {
                prefetch.push(key.clone());
            }
            keys.push(Some(fkCheckKey {
                k: key,
                isPrefix: is_prefix,
            }));
        }
        if !prefetch.is_empty() {
            self.prefetchKeys(transaction, &prefetch)?;
        }
        transaction.SetSnapshotScanBatchSize(2);
        let result = (|| {
            for (row, key) in rows.iter_mut().zip(keys) {
                let Some(key) = key else {
                    continue;
                };
                // 同行键已检查过则复用 ignore 结果。
                if let Some(ignore) = self.checkRowsCache.get(&key.k).copied() {
                    if ignore {
                        row.ignored = true;
                        self.runtime.AppendWarning(self.failedError());
                    }
                    continue;
                }
                let check = if key.isPrefix {
                    self.checkPrefixKey(transaction, &key.k)
                } else {
                    self.checkKey(transaction, &key.k)
                };
                let ignore = check.is_err();
                self.checkRowsCache.insert(key.k, ignore);
                if ignore {
                    row.ignored = true;
                    self.runtime.AppendWarning(self.failedError());
                }
                if let Some(stats) = self.stats.as_mut() {
                    stats.Keys += 1;
                }
            }
            Ok(())
        })();
        transaction.SetSnapshotScanBatchSize(256);
        if let Some(stats) = self.stats.as_mut() {
            stats.Total = started.elapsed();
            stats.Check = stats.Total;
            self.runtime.RegisterCheckStats(self.FKCheck.id, stats);
        }
        result
    }

    /// 构造本检查规格绑定的失败错误。
    fn failedError(&self) -> errors::SharedError {
        errors::New(self.FKCheck.failed_error.clone())
    }
}

/// 按列偏移抽取外键值，并去重避免重复检查。
pub struct fkValueHelper {
    pub colsOffsets: Vec<usize>,
    pub fkValuesSet: HashSet<Key>,
}

impl fkValueHelper {
    /// 抽取值；含 NULL 或已见过则返回空（跳过检查）。
    pub fn fetchFKValuesWithCheck(&mut self, row: &[Datum]) -> FKResult<Vec<Datum>> {
        let values = self.fetchFKValues(row)?;
        if self.hasNullValue(&values) {
            return Ok(Vec::new());
        }
        let key = encodeDatums(&values);
        if !self.fkValuesSet.insert(key) {
            return Ok(Vec::new());
        }
        Ok(values)
    }

    /// 按偏移拷贝外键列值。
    pub fn fetchFKValues(&self, row: &[Datum]) -> FKResult<Vec<Datum>> {
        self.colsOffsets
            .iter()
            .map(|offset| {
                row.get(*offset)
                    .cloned()
                    .ok_or_else(|| errors::New(format!("column offset {offset} is out of bounds")))
            })
            .collect()
    }

    /// 外键值中是否含 NULL（含 NULL 时通常不做匹配检查）。
    pub fn hasNullValue(&self, values: &[Datum]) -> bool {
        values.iter().any(|value| matches!(value, Datum::Null))
    }
}

/// 按列名（忽略大小写）解析在表中的偏移。
pub fn getFKColumnsOffsets(table: &TableInfo, columns: &[String]) -> FKResult<Vec<usize>> {
    columns
        .iter()
        .map(|name| {
            table
                .columns
                .iter()
                .find(|column| column.name.eq_ignore_ascii_case(name))
                .map(|column| column.offset)
                .ok_or_else(|| errors::New(format!("unknown column '{name}'")))
        })
        .collect()
}

/// 待检查键及其是否为前缀扫描键。
pub struct fkCheckKey {
    pub k: Key,
    pub isPrefix: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// INSERT IGNORE 路径上待检查行及是否已忽略。
pub struct ToBeCheckedRow {
    pub row: Vec<Datum>,
    pub ignored: bool,
}

/// 对单行跑全部检查；任一失败则标记 ignored 并返回 true。
pub fn checkFKIgnoreErr(
    runtime: &dyn FKCheckRuntime,
    checks: &mut [FKCheckExec],
    row: &[Datum],
) -> FKResult<bool> {
    let mut transaction = runtime.Transaction(true)?;
    let mut rows = [ToBeCheckedRow {
        row: row.to_vec(),
        ignored: false,
    }];
    for check in checks {
        check.checkRows(transaction.as_mut(), &mut rows)?;
    }
    Ok(rows[0].ignored)
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 级联更新：新值与对应的多组旧值。
pub struct UpdatedValuesCouple {
    pub NewValues: Vec<Datum>,
    pub OldValuesList: Vec<Vec<Datum>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 级联语句中的表引用（可带 USE INDEX）。
pub struct TableRefsClause {
    pub schema: String,
    pub table: String,
    pub use_index: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 级联 WHERE：单列 IN 或多列行 IN。
pub enum WhereCondition {
    SingleColumnIn {
        column: String,
        values: Vec<Datum>,
    },
    MultiColumnIn {
        columns: Vec<String>,
        rows: Vec<Vec<Datum>>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 级联 DELETE 的简化 AST。
pub struct DeleteStmt {
    pub table_refs: TableRefsClause,
    pub where_condition: WhereCondition,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 级联 UPDATE 的列赋值。
pub struct Assignment {
    pub column: String,
    pub value: Datum,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 级联 UPDATE 的简化 AST。
pub struct UpdateStmt {
    pub table_refs: TableRefsClause,
    pub where_condition: WhereCondition,
    pub assignments: Vec<Assignment>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 级联可生成的语句类型。
pub enum CascadeStatement {
    Delete(DeleteStmt),
    Update(UpdateStmt),
}

/// 优化后的级联计划占位。
pub trait CascadePlan: Send {}
/// 可执行的级联执行器。
pub trait CascadeExecutor: Send {
    fn Execute(&mut self) -> FKResult;
}

/// 级联运行时：优化语句、构建执行器、注册统计。
pub trait FKCascadeRuntime: Send + Sync {
    fn Optimize(&self, statement: &CascadeStatement) -> FKResult<Box<dyn CascadePlan>>;
    fn BuildExecutor(&self, plan: &dyn CascadePlan) -> FKResult<Box<dyn CascadeExecutor>>;
    fn RuntimeStatsEnabled(&self) -> bool;
    fn RegisterCascadeStats(&self, id: i64, stats: &FKCascadeRuntimeStats);
}

/// 单条外键级联执行器：收集变更值并生成级联计划。
pub struct FKCascadeExec {
    pub plan: FKCascadeSpec,
    pub fkValueHelper: fkValueHelper,
    pub fkValues: Vec<Vec<Datum>>,
    pub fkUpdatedValuesMap: BTreeMap<Key, UpdatedValuesCouple>,
    pub stats: Option<FKCascadeRuntimeStats>,
    pub CascadePlans: Vec<Box<dyn CascadePlan>>,
    runtime: Arc<dyn FKCascadeRuntime>,
}

/// 按表构建级联执行器的工厂。
pub struct executorBuilder {
    runtime: Arc<dyn FKCascadeRuntime>,
}

impl executorBuilder {
    /// 绑定级联运行时。
    pub fn new(runtime: Arc<dyn FKCascadeRuntime>) -> Self {
        Self { runtime }
    }

    /// 按表 ID 批量构建级联执行器。
    pub fn buildTblID2FKCascadeExecs(
        &self,
        tables: &HashMap<i64, TableInfo>,
        cascades: &HashMap<i64, Vec<FKCascadeSpec>>,
    ) -> FKResult<HashMap<i64, Vec<FKCascadeExec>>> {
        let mut result = HashMap::new();
        for (table_id, table) in tables {
            let built =
                self.buildFKCascadeExecs(table, cascades.get(table_id).map_or(&[], Vec::as_slice))?;
            if !built.is_empty() {
                result.insert(*table_id, built);
            }
        }
        Ok(result)
    }

    /// 为单表构建全部级联执行器。
    pub fn buildFKCascadeExecs(
        &self,
        table: &TableInfo,
        cascades: &[FKCascadeSpec],
    ) -> FKResult<Vec<FKCascadeExec>> {
        cascades
            .iter()
            .map(|cascade| self.buildFKCascadeExec(table, cascade.clone()))
            .collect()
    }

    /// 构建单个级联执行器（列偏移取自被引用列）。
    pub fn buildFKCascadeExec(
        &self,
        table: &TableInfo,
        mut cascade: FKCascadeSpec,
    ) -> FKResult<FKCascadeExec> {
        let offsets = getFKColumnsOffsets(table, &cascade.referred_columns)?;
        cascade.child_table = cascade.child_table.clone();
        Ok(FKCascadeExec {
            plan: cascade,
            fkValueHelper: fkValueHelper {
                colsOffsets: offsets,
                fkValuesSet: HashSet::new(),
            },
            fkValues: Vec::new(),
            fkUpdatedValuesMap: BTreeMap::new(),
            stats: None,
            CascadePlans: Vec::new(),
            runtime: Arc::clone(&self.runtime),
        })
    }
}

impl FKCascadeExec {
    /// 父行删除：记录需匹配的外键旧值。
    pub fn onDeleteRow(&mut self, row: &[Datum]) -> FKResult {
        let values = self.fkValueHelper.fetchFKValuesWithCheck(row)?;
        if !values.is_empty() {
            self.fkValues.push(values);
        }
        Ok(())
    }

    /// 父行更新：SET NULL 只记旧值；CASCADE 按新值聚合旧值列表。
    pub fn onUpdateRow(&mut self, old_row: &[Datum], new_row: &[Datum]) -> FKResult {
        let old_values = self.fkValueHelper.fetchFKValuesWithCheck(old_row)?;
        if old_values.is_empty() {
            return Ok(());
        }
        if self.plan.foreign_key.on_update == ReferOption::SetNull {
            self.fkValues.push(old_values);
            return Ok(());
        }
        let new_values = self.fkValueHelper.fetchFKValues(new_row)?;
        let key = encodeDatums(&new_values);
        self.fkUpdatedValuesMap
            .entry(key)
            .or_insert_with(|| UpdatedValuesCouple {
                NewValues: new_values,
                OldValuesList: Vec::new(),
            })
            .OldValuesList
            .push(old_values);
        Ok(())
    }

    /// 生成级联计划并构建执行器；无待处理值时返回 None。
    pub fn buildExecutor(&mut self) -> FKResult<Option<Box<dyn CascadeExecutor>>> {
        let started = Instant::now();
        if self.runtime.RuntimeStatsEnabled() && self.stats.is_none() {
            self.stats = Some(FKCascadeRuntimeStats::default());
        }
        let Some(plan) = self.buildFKCascadePlan()? else {
            return Ok(None);
        };
        let executor = self.runtime.BuildExecutor(plan.as_ref())?;
        self.CascadePlans.push(plan);
        if let Some(stats) = self.stats.as_mut() {
            stats.Total += started.elapsed();
            self.runtime.RegisterCascadeStats(self.plan.id, stats);
        }
        Ok(Some(executor))
    }

    /// 按 ON DELETE/UPDATE 选项生成并优化级联语句。
    pub fn buildFKCascadePlan(&mut self) -> FKResult<Option<Box<dyn CascadePlan>>> {
        if self.fkValues.is_empty() && self.fkUpdatedValuesMap.is_empty() {
            return Ok(None);
        }
        let index_name = self
            .plan
            .foreign_key_index
            .as_ref()
            .map(|index| index.name.clone())
            .unwrap_or_default();
        let schema = self.plan.referred_schema.clone();
        let table = self.plan.child_table.name.clone();
        let columns = self.plan.foreign_key_columns.clone();
        let cascade_type = self.plan.cascade_type;
        let on_delete = self.plan.foreign_key.on_delete;
        let on_update = self.plan.foreign_key.on_update;
        let statement = match cascade_type {
            FKCascadeType::OnDelete => {
                let values = self.fetchOnDeleteOrUpdateFKValues();
                match on_delete {
                    ReferOption::Cascade => CascadeStatement::Delete(GenCascadeDeleteAST(
                        &schema,
                        &table,
                        &index_name,
                        &columns,
                        values,
                    )?),
                    ReferOption::SetNull => CascadeStatement::Update(GenCascadeSetNullAST(
                        &schema,
                        &table,
                        &index_name,
                        &columns,
                        values,
                    )?),
                    _ => return Err(errors::New("invalid ON DELETE cascade option")),
                }
            }
            FKCascadeType::OnUpdate => match on_update {
                ReferOption::Cascade => {
                    let couple = self
                        .fetchUpdatedValuesCouple()
                        .ok_or_else(|| errors::New("updated foreign-key values are missing"))?;
                    if let Some(stats) = self.stats.as_mut() {
                        stats.Keys += couple.OldValuesList.len();
                    }
                    CascadeStatement::Update(GenCascadeUpdateAST(
                        &schema,
                        &table,
                        &index_name,
                        &columns,
                        &couple,
                    )?)
                }
                ReferOption::SetNull => {
                    let values = self.fetchOnDeleteOrUpdateFKValues();
                    CascadeStatement::Update(GenCascadeSetNullAST(
                        &schema,
                        &table,
                        &index_name,
                        &columns,
                        values,
                    )?)
                }
                _ => return Err(errors::New("invalid ON UPDATE cascade option")),
            },
        };
        self.runtime.Optimize(&statement).map(Some)
    }

    /// 取出至多 `MAX_HANDLE_FK_VALUE_IN_ONE_CASCADE` 组值。
    pub fn fetchOnDeleteOrUpdateFKValues(&mut self) -> Vec<Vec<Datum>> {
        // 分批取出，控制单次级联 SQL 规模。
        let take = self.fkValues.len().min(MAX_HANDLE_FK_VALUE_IN_ONE_CASCADE);
        let values = self.fkValues.drain(..take).collect::<Vec<_>>();
        if let Some(stats) = self.stats.as_mut() {
            stats.Keys += values.len();
        }
        values
    }

    /// 取出一组新值及其部分旧值列表（可能分批）。
    pub fn fetchUpdatedValuesCouple(&mut self) -> Option<UpdatedValuesCouple> {
        let key = self.fkUpdatedValuesMap.keys().next()?.clone();
        let couple = self.fkUpdatedValuesMap.get_mut(&key)?;
        if couple.OldValuesList.len() <= MAX_HANDLE_FK_VALUE_IN_ONE_CASCADE {
            return self.fkUpdatedValuesMap.remove(&key);
        }
        let old_values = couple
            .OldValuesList
            .drain(..MAX_HANDLE_FK_VALUE_IN_ONE_CASCADE)
            .collect();
        Some(UpdatedValuesCouple {
            NewValues: couple.NewValues.clone(),
            OldValuesList: old_values,
        })
    }
}

/// 生成级联 DELETE AST。
pub fn GenCascadeDeleteAST(
    schema: &str,
    table: &str,
    index: &str,
    columns: &[ColumnInfo],
    values: Vec<Vec<Datum>>,
) -> FKResult<DeleteStmt> {
    Ok(DeleteStmt {
        table_refs: genTableRefsAST(schema, table, index),
        where_condition: genWhereConditionAst(columns, values)?,
    })
}

/// 生成将外键列置 NULL 的级联 UPDATE AST。
pub fn GenCascadeSetNullAST(
    schema: &str,
    table: &str,
    index: &str,
    columns: &[ColumnInfo],
    values: Vec<Vec<Datum>>,
) -> FKResult<UpdateStmt> {
    let couple = UpdatedValuesCouple {
        NewValues: vec![Datum::Null; columns.len()],
        OldValuesList: values,
    };
    GenCascadeUpdateAST(schema, table, index, columns, &couple)
}

/// 生成将匹配旧值的子行更新为新值的 AST。
pub fn GenCascadeUpdateAST(
    schema: &str,
    table: &str,
    index: &str,
    columns: &[ColumnInfo],
    couple: &UpdatedValuesCouple,
) -> FKResult<UpdateStmt> {
    if columns.len() != couple.NewValues.len() {
        return Err(errors::New(
            "cascade update column count does not match new-value count",
        ));
    }
    Ok(UpdateStmt {
        table_refs: genTableRefsAST(schema, table, index),
        where_condition: genWhereConditionAst(columns, couple.OldValuesList.clone())?,
        assignments: columns
            .iter()
            .zip(&couple.NewValues)
            .map(|(column, value)| Assignment {
                column: column.name.clone(),
                value: value.clone(),
            })
            .collect(),
    })
}

/// 构造表引用子句。
pub fn genTableRefsAST(schema: &str, table: &str, index: &str) -> TableRefsClause {
    TableRefsClause {
        schema: schema.to_owned(),
        table: table.to_owned(),
        use_index: (!index.is_empty()).then(|| index.to_owned()),
    }
}

/// 按列数生成单列或多列 IN 条件。
pub fn genWhereConditionAst(
    columns: &[ColumnInfo],
    values: Vec<Vec<Datum>>,
) -> FKResult<WhereCondition> {
    if columns.is_empty() {
        return Err(errors::New("cascade condition has no foreign-key columns"));
    }
    if columns.len() > 1 {
        return genWhereConditionAstForMultiColumn(columns, values);
    }
    let mut single_values = Vec::with_capacity(values.len());
    for row in values {
        if row.len() != 1 {
            return Err(errors::New("single-column cascade value has invalid width"));
        }
        single_values.push(row.into_iter().next().expect("row width checked"));
    }
    Ok(WhereCondition::SingleColumnIn {
        column: columns[0].name.clone(),
        values: single_values,
    })
}

/// 多列 `(c1,c2,...) IN ((...),...)` 条件。
pub fn genWhereConditionAstForMultiColumn(
    columns: &[ColumnInfo],
    values: Vec<Vec<Datum>>,
) -> FKResult<WhereCondition> {
    if values.iter().any(|row| row.len() != columns.len()) {
        return Err(errors::New(
            "multi-column cascade value width does not match column count",
        ));
    }
    Ok(WhereCondition::MultiColumnIn {
        columns: columns.iter().map(|column| column.name.clone()).collect(),
        rows: values,
    })
}

impl FKCheckRuntimeStats {
    /// 格式化检查统计为可读字符串。
    pub fn String(&self) -> String {
        let mut result = format!("total:{}", formatDuration(self.Total));
        if !self.Check.is_zero() {
            result.push_str(&format!(", check:{}", formatDuration(self.Check)));
        }
        if !self.Lock.is_zero() {
            result.push_str(&format!(", lock:{}", formatDuration(self.Lock)));
        }
        if self.Keys > 0 {
            result.push_str(&format!(", foreign_keys:{}", self.Keys));
        }
        result
    }

    /// 克隆为统一统计枚举。
    pub fn Clone(&self) -> FKRuntimeStats {
        FKRuntimeStats::Check(std::clone::Clone::clone(self))
    }

    /// 累加另一份检查统计。
    pub fn Merge(&mut self, other: &FKRuntimeStats) {
        let FKRuntimeStats::Check(other) = other else {
            return;
        };
        self.Total += other.Total;
        self.Check += other.Check;
        self.Lock += other.Lock;
        self.Keys += other.Keys;
    }

    /// 返回检查统计类型常量。
    pub fn Tp(&self) -> i32 {
        TP_FK_CHECK_RUNTIME_STATS
    }
}

impl FKCascadeRuntimeStats {
    /// 格式化级联统计为可读字符串。
    pub fn String(&self) -> String {
        let mut result = format!("total:{}", formatDuration(self.Total));
        if self.Keys > 0 {
            result.push_str(&format!(", foreign_keys:{}", self.Keys));
        }
        result
    }

    /// 克隆为统一统计枚举。
    pub fn Clone(&self) -> FKRuntimeStats {
        FKRuntimeStats::Cascade(std::clone::Clone::clone(self))
    }

    /// 累加另一份级联统计。
    pub fn Merge(&mut self, other: &FKRuntimeStats) {
        let FKRuntimeStats::Cascade(other) = other else {
            return;
        };
        self.Total += other.Total;
        self.Keys += other.Keys;
    }

    /// 返回级联统计类型常量。
    pub fn Tp(&self) -> i32 {
        TP_FK_CASCADE_RUNTIME_STATS
    }
}

/// 将 Datum 列表编码为可哈希/可比较的去重键。
fn encodeDatums(values: &[Datum]) -> Key {
    let mut encoded = Vec::new();
    for value in values {
        let (tag, bytes) = match value {
            Datum::Null => (0_u8, Vec::new()),
            Datum::Int64(value) => (1, value.to_be_bytes().to_vec()),
            Datum::Uint64(value) => (2, value.to_be_bytes().to_vec()),
            Datum::Bytes(value) => (3, value.clone()),
            Datum::String(value) => (4, value.as_bytes().to_vec()),
        };
        encoded.push(tag);
        encoded.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        encoded.extend_from_slice(&bytes);
    }
    encoded
}

/// 将 Duration 格式化为统计展示用短字符串。
fn formatDuration(duration: Duration) -> String {
    const MICROSECOND: u128 = 1_000;
    const MILLISECOND: u128 = 1_000_000;
    const SECOND: u128 = 1_000_000_000;

    let nanos = duration.as_nanos();
    if nanos <= MICROSECOND {
        return format!("{duration:?}");
    }
    let unit = if nanos >= SECOND {
        SECOND
    } else if nanos >= MILLISECOND {
        MILLISECOND
    } else {
        MICROSECOND
    };
    // Match execdetails.FormatDuration: values below ten units keep two
    // decimals, larger values keep one, with positive values rounded halfway
    // up just like Go's math.Round.
    let precision = if nanos < 10 * unit { 100 } else { 10 };
    let quantum = unit / precision;
    let rounded = ((nanos + quantum / 2) / quantum) * quantum;
    let rounded = Duration::new((rounded / SECOND) as u64, (rounded % SECOND) as u32);
    format!("{rounded:?}")
}
