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

// AutoID 本地缓存分配器与批大小计算。
//
// 本模块实现 TiDB/AsterSQL 的 AutoID（自动标识）核心：为 `_tidb_rowid`、
// `AUTO_INCREMENT`、`AUTO_RANDOM`、SEQUENCE 从元数据存储预留一批 ID，缓存在
// 本地 `[base, end]` 区间，耗尽后再向存储事务（[`IdStore`]）申请。
// 同时提供 increment/offset 批大小、序列寻值、有符号整数可比编码，以及
// `AUTO_RANDOM` 的分片位（shard bits）布局工具。

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::errors::{
    AUTO_RANDOM_NON_POSITIVE, AutoIdError, Result, autoinc_read_failed,
    invalid_increment_and_offset,
};

/// 内存系统库 schema ID 的高位标志（第 62 位）。
pub const SYSTEM_SCHEMA_ID_FLAG: i64 = 1_i64 << 62;
/// `information_schema` 固定库 ID。
pub const INFORMATION_SCHEMA_DB_ID: i64 = SYSTEM_SCHEMA_ID_FLAG | 1;
/// `performance_schema` 固定库 ID。
pub const PERFORMANCE_SCHEMA_DB_ID: i64 = SYSTEM_SCHEMA_ID_FLAG | 10_000;
/// `metrics_schema` 固定库 ID。
pub const METRIC_SCHEMA_DB_ID: i64 = SYSTEM_SCHEMA_ID_FLAG | 20_000;
/// 预留系统表 ID 基址。
pub const RESERVED_TABLES_BASE_ID: i64 = SYSTEM_SCHEMA_ID_FLAG | 5_000;

/// 判断 schema_id 是否为带 SYSTEM_SCHEMA_ID_FLAG 的内存系统库。
pub fn is_mem_schema_id(schema_id: i64) -> bool {
    schema_id & SYSTEM_SCHEMA_ID_FLAG != 0
}

/// 动态步长下限：本地缓存预留至少这么多 ID。
const MIN_STEP: i64 = 30_000;
/// 动态步长上限。
const MAX_STEP: i64 = 2_000_000;
/// 期望一批 ID 的消耗时间窗口，用于按消耗速率调节 step。
const DEFAULT_CONSUME_TIME: Duration = Duration::from_secs(10);
/// MySQL 风格 auto_increment increment/offset 合法下限。
const MIN_INCREMENT: i64 = 1;
/// MySQL 风格 auto_increment increment/offset 合法上限。
const MAX_INCREMENT: i64 = 65_535;

/// row id / AUTO_RANDOM 总位数（64）。
pub const ROW_ID_BIT_LENGTH: u64 = 64;
/// AUTO_RANDOM 默认分片位数。
pub const AUTO_RANDOM_SHARD_BITS_DEFAULT: u64 = 5;
/// AUTO_RANDOM 默认 range bits（可用位宽）。
pub const AUTO_RANDOM_RANGE_BITS_DEFAULT: u64 = 64;
/// 分片位数上限。
pub const AUTO_RANDOM_SHARD_BITS_MAX: i32 = 15;
/// range bits 上限。
pub const AUTO_RANDOM_RANGE_BITS_MAX: i32 = 64;
/// range bits 下限。
pub const AUTO_RANDOM_RANGE_BITS_MIN: i32 = 32;
/// 增量位数下限（保证足够 ID 空间）。
pub const AUTO_RANDOM_INC_BITS_MIN: i32 = 27;
/// DDL 未显式指定时的哨兵长度（-1）。
pub const UNSPECIFIED_LENGTH: i32 = -1;

/// 规范化 AUTO_RANDOM 分片位数：未指定用默认值，并校验正数与上限。
pub fn auto_random_shard_bits_normalize(shard: i32, column: &str) -> Result<u64> {
    if shard == UNSPECIFIED_LENGTH {
        return Ok(AUTO_RANDOM_SHARD_BITS_DEFAULT);
    }
    if shard <= 0 {
        return Err(AutoIdError::InvalidAutoRandom(
            AUTO_RANDOM_NON_POSITIVE.to_owned(),
        ));
    }
    if shard > AUTO_RANDOM_SHARD_BITS_MAX {
        return Err(AutoIdError::InvalidAutoRandom(format!(
            "max allowed auto_random shard bits is {}, but got {} on column `{}`",
            AUTO_RANDOM_SHARD_BITS_MAX, shard, column
        )));
    }
    Ok(shard as u64)
}

/// 规范化 AUTO_RANDOM range bits，校验落在 [MIN, MAX]。
pub fn auto_random_range_bits_normalize(range_bits: i32) -> Result<u64> {
    if range_bits == UNSPECIFIED_LENGTH {
        return Ok(AUTO_RANDOM_RANGE_BITS_DEFAULT);
    }
    if !(AUTO_RANDOM_RANGE_BITS_MIN..=AUTO_RANDOM_RANGE_BITS_MAX).contains(&range_bits) {
        return Err(AutoIdError::InvalidAutoRandom(format!(
            "auto_random range bits must be between {} and {}, but got {}",
            AUTO_RANDOM_RANGE_BITS_MIN, AUTO_RANDOM_RANGE_BITS_MAX, range_bits
        )));
    }
    Ok(range_bits as u64)
}

/// 可取消上下文：分配/退避等待时检查取消标志（对应 Go context）。
#[derive(Clone)]
pub struct Context {
    inner: Arc<ContextInner>,
}

/// Context 内部共享状态：取消标志与条件变量。
struct ContextInner {
    canceled: AtomicBool,
    signal: Mutex<()>,
    wake: Condvar,
}

impl Default for Context {
    fn default() -> Self {
        Self::background()
    }
}

impl Context {
    /// 创建未取消的后台上下文。
    pub fn background() -> Self {
        Self {
            inner: Arc::new(ContextInner {
                canceled: AtomicBool::new(false),
                signal: Mutex::new(()),
                wake: Condvar::new(),
            }),
        }
    }

    /// 标记取消并唤醒所有等待者。
    pub fn cancel(&self) {
        self.inner.canceled.store(true, Ordering::SeqCst);
        let _guard = self.inner.signal.lock().unwrap();
        self.inner.wake.notify_all();
    }

    /// 是否已取消。
    pub fn is_canceled(&self) -> bool {
        self.inner.canceled.load(Ordering::SeqCst)
    }

    /// 若已取消则返回 [`AutoIdError::Canceled`]。
    pub fn check(&self) -> Result<()> {
        if self.is_canceled() {
            Err(AutoIdError::Canceled)
        } else {
            Ok(())
        }
    }

    /// 可取消的限时等待（用于退避）；超时或被唤醒后再次 check。
    pub(crate) fn wait(&self, duration: Duration) -> Result<()> {
        self.check()?;
        let guard = self.inner.signal.lock().unwrap();
        let (_guard, _) = self.inner.wake.wait_timeout(guard, duration).unwrap();
        self.check()
    }
}

/// 分配器种类：隐式 row id、独立自增、AUTO_RANDOM、SEQUENCE。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum AllocatorType {
    /// 隐式 `_tidb_rowid` 或与自增共用的默认分配器。
    RowId,
    /// 与 row id 分离的 `AUTO_INCREMENT` 分配器。
    AutoIncrement,
    /// `AUTO_RANDOM` 主键分配器。
    AutoRandom,
    /// SQL SEQUENCE 对象分配器。
    Sequence,
}

impl AllocatorType {
    /// 返回与 Go 一致的类型名字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RowId => "_tidb_rowid",
            Self::AutoIncrement => "auto_increment",
            Self::AutoRandom => "auto_random",
            Self::Sequence => "sequence",
        }
    }
}

impl fmt::Display for AllocatorType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// AutoID 分配器接口：分配、rebase、迁移库表身份、查询水位。
///
/// `alloc` 返回 `(min, max]`：调用方可使用 `(min, max]` 内的 ID（通常取 `min+1..=max`）。
pub trait Allocator: Send + Sync {
    fn alloc(&self, ctx: &Context, n: u64, increment: i64, offset: i64) -> Result<(i64, i64)>;
    fn alloc_seq_cache(&self) -> Result<(i64, i64, i64)>;
    fn rebase(&self, ctx: &Context, new_base: i64, alloc_ids: bool) -> Result<()>;
    fn force_rebase(&self, new_base: i64) -> Result<()>;
    fn rebase_seq(&self, new_base: i64) -> Result<(i64, bool)>;
    fn transfer(&self, database_id: i64, table_id: i64) -> Result<()>;
    fn base(&self) -> i64;
    fn end(&self) -> i64;
    fn next_global_auto_id(&self) -> Result<i64>;
    fn get_type(&self) -> AllocatorType;
}

/// 一张表上可能并存的多种分配器集合。
#[derive(Clone, Default)]
pub struct Allocators {
    /// 为 true 时 AutoIncrement 与 RowId 使用独立分配器。
    pub separate_auto_increment: bool,
    pub allocators: Vec<Arc<dyn Allocator>>,
}

impl Allocators {
    /// 构造分配器集合。
    pub fn new(separate_auto_increment: bool, allocators: Vec<Arc<dyn Allocator>>) -> Self {
        Self {
            separate_auto_increment,
            allocators,
        }
    }

    /// 追加一个分配器（建造者模式）。
    pub fn append(mut self, allocator: Arc<dyn Allocator>) -> Self {
        self.allocators.push(allocator);
        self
    }

    /// 按类型查找；未分离自增时 AutoIncrement 回落到 RowId。
    pub fn get(&self, mut allocator_type: AllocatorType) -> Option<Arc<dyn Allocator>> {
        if !self.separate_auto_increment && allocator_type == AllocatorType::AutoIncrement {
            allocator_type = AllocatorType::RowId;
        }
        self.allocators
            .iter()
            .find(|allocator| allocator.get_type() == allocator_type)
            .cloned()
    }

    /// 按谓词过滤得到新集合。
    pub fn filter(&self, predicate: impl Fn(&Arc<dyn Allocator>) -> bool) -> Self {
        Self::new(
            self.separate_auto_increment,
            self.allocators
                .iter()
                .filter(|allocator| predicate(allocator))
                .cloned()
                .collect(),
        )
    }

    pub fn len(&self) -> usize {
        self.allocators.len()
    }

    pub fn is_empty(&self) -> bool {
        self.allocators.is_empty()
    }
}

/// 元数据中 AutoID 键的种类。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AutoIdKeyKind {
    /// `_tidb_rowid` / 默认水位。
    RowId,
    /// 独立自增水位；携带表 meta version。
    IncrementId(u16),
    /// AUTO_RANDOM 水位。
    RandomId,
    /// SEQUENCE 当前值。
    SequenceValue,
    /// SEQUENCE 已循环轮次。
    SequenceCycle,
}

/// 定位某库表某类 AutoID 水位的键。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AutoIdKey {
    pub database_id: i64,
    pub table_id: i64,
    pub kind: AutoIdKeyKind,
}

/// 单次元数据事务上的读写接口。
pub trait IdTransaction {
    fn get(&self, key: AutoIdKey) -> Result<i64>;
    fn put(&mut self, key: AutoIdKey, value: i64) -> Result<()>;
    fn inc(&mut self, key: AutoIdKey, step: i64) -> Result<i64>;
    fn copy_to(&mut self, from: AutoIdKey, to: AutoIdKey) -> Result<()>;
}

/// AutoID 持久化存储：在事务中执行水位更新。
pub trait IdStore: Send + Sync {
    fn run_in_transaction(
        &self,
        operation: &mut dyn FnMut(&mut dyn IdTransaction) -> Result<()>,
    ) -> Result<()>;
}

/// SQL SEQUENCE 对象的定义参数。
#[derive(Clone, Debug, Default)]
pub struct SequenceInfo {
    pub increment: i64,
    pub start: i64,
    pub min_value: i64,
    pub max_value: i64,
    pub cache: bool,
    pub cache_value: i64,
    pub cycle: bool,
}

/// 从 table meta 抽取的、构造分配器所需的精简表信息。
#[derive(Clone, Debug, Default)]
pub struct TableInfo {
    pub id: i64,
    pub version: u16,
    pub pk_is_handle: bool,
    pub is_common_handle: bool,
    pub has_auto_increment_column: bool,
    pub auto_increment_unsigned: bool,
    pub auto_increment_id: i64,
    pub auto_id_cache: i64,
    pub separate_auto_increment: bool,
    pub auto_random_bits: u64,
    pub auto_random_unsigned: bool,
    pub sequence: Option<SequenceInfo>,
}

/// 创建 [`DefaultAllocator`] 时的可选参数。
#[derive(Clone, Copy, Debug)]
pub enum AllocatorOption {
    /// 自定义本地预留步长；0 表示忽略。
    CustomStep(i64),
    /// 表 meta 版本，影响 IncrementId 键。
    TableInfoVersion(u16),
}

/// 分配器可变状态：本地缓存区间 `[base, end]` 与动态 step。
struct AllocatorState {
    base: i64,
    end: i64,
    database_id: i64,
    table_id: i64,
    last_alloc_time: Instant,
    step: i64,
}

/// 默认本地缓存分配器：批量向 IdStore 预留，再在内存中发放。
pub struct DefaultAllocator {
    store: Arc<dyn IdStore>,
    state: Mutex<AllocatorState>,
    table_version: u16,
    is_unsigned: bool,
    custom_step: bool,
    allocator_type: AllocatorType,
    sequence: Option<SequenceInfo>,
}

/// 全局默认预留步长（可动态调节）。
static DEFAULT_STEP: AtomicI64 = AtomicI64::new(30_000);

/// 读取全局默认 step。
pub fn get_step() -> i64 {
    DEFAULT_STEP.load(Ordering::SeqCst)
}

/// 设置全局默认 step。
pub fn set_step(step: i64) {
    DEFAULT_STEP.store(step, Ordering::SeqCst);
}

impl DefaultAllocator {
    /// 使用全局默认 step 创建分配器。
    pub fn new(
        store: Arc<dyn IdStore>,
        database_id: i64,
        table_id: i64,
        is_unsigned: bool,
        allocator_type: AllocatorType,
    ) -> Self {
        Self::with_options(
            store,
            database_id,
            table_id,
            is_unsigned,
            allocator_type,
            &[],
        )
    }

    /// 解析 [`AllocatorOption`] 后创建分配器。
    pub fn with_options(
        store: Arc<dyn IdStore>,
        database_id: i64,
        table_id: i64,
        is_unsigned: bool,
        allocator_type: AllocatorType,
        options: &[AllocatorOption],
    ) -> Self {
        let mut step = get_step();
        let mut custom_step = false;
        let mut table_version = 0;
        for option in options {
            match *option {
                AllocatorOption::CustomStep(0) => {}
                AllocatorOption::CustomStep(value) => {
                    step = value;
                    custom_step = true;
                }
                AllocatorOption::TableInfoVersion(value) => table_version = value,
            }
        }
        // Go NewAllocator: since TableInfoVersion5, AUTO_INCREMENT and hidden
        // RowID use separate allocators. AUTO_ID_CACHE 1 only selects the
        // single-point AUTO_INCREMENT allocator; RowID must retain the normal
        // local cache and its adaptive step behavior.
        if custom_step && step == 1 && table_version >= 5 && allocator_type == AllocatorType::RowId
        {
            step = get_step();
            custom_step = false;
        }
        Self {
            store,
            state: Mutex::new(AllocatorState {
                base: 0,
                end: 0,
                database_id,
                table_id,
                last_alloc_time: Instant::now(),
                step,
            }),
            table_version,
            is_unsigned,
            custom_step,
            allocator_type,
            sequence: None,
        }
    }

    /// 创建 SEQUENCE 类型分配器。
    pub fn new_sequence(
        store: Arc<dyn IdStore>,
        database_id: i64,
        table_id: i64,
        sequence: SequenceInfo,
    ) -> Self {
        let mut allocator = Self::new(store, database_id, table_id, false, AllocatorType::Sequence);
        allocator.sequence = Some(sequence);
        allocator
    }

    /// 测试注入本地 step。
    pub fn set_step_for_test(&self, step: i64) {
        self.state.lock().unwrap().step = step;
    }

    /// Corresponds to Go `TestModifyBaseAndEndInjection`.
    /// 测试注入本地缓存区间边界。
    pub fn modify_base_and_end_for_test(&self, base: i64, end: i64) {
        let mut state = self.state.lock().unwrap();
        state.base = base;
        state.end = end;
    }

    /// 按分配器类型构造对应的元数据键。
    fn key_for(&self, state: &AllocatorState) -> AutoIdKey {
        AutoIdKey {
            database_id: state.database_id,
            table_id: state.table_id,
            kind: match self.allocator_type {
                AllocatorType::RowId => AutoIdKeyKind::RowId,
                AllocatorType::AutoIncrement => AutoIdKeyKind::IncrementId(self.table_version),
                AllocatorType::AutoRandom => AutoIdKeyKind::RandomId,
                AllocatorType::Sequence => AutoIdKeyKind::SequenceValue,
            },
        }
    }

    /// SEQUENCE 循环轮次键。
    fn cycle_key(&self, state: &AllocatorState) -> AutoIdKey {
        AutoIdKey {
            database_id: state.database_id,
            table_id: state.table_id,
            kind: AutoIdKeyKind::SequenceCycle,
        }
    }

    /// 有符号 rebase：必要时向存储推进全局水位并刷新本地缓存。
    fn rebase_signed_locked(
        &self,
        ctx: &Context,
        state: &mut AllocatorState,
        required_base: i64,
        allocate_ids: bool,
    ) -> Result<()> {
        if required_base <= state.base {
            return Ok(());
        }
        // 仍在本地缓存内，只抬高 base。
        if required_base <= state.end {
            state.base = required_base;
            return Ok(());
        }
        ctx.check()?;
        let key = self.key_for(state);
        let step = state.step;
        let mut new_base = 0;
        let mut new_end = 0;
        self.store.run_in_transaction(&mut |transaction| {
            let current_end = transaction.get(key)?;
            if allocate_ids {
                // 预留一批：从 max(全局, required) 起再加 step。
                new_base = current_end.max(required_base);
                new_end = new_base.min(i64::MAX.wrapping_sub(step)).wrapping_add(step);
            } else if current_end >= required_base {
                new_base = current_end;
                new_end = current_end;
                return Ok(());
            } else {
                // 仅抬高全局水位到 required_base，不额外预留。
                new_base = required_base;
                new_end = required_base;
            }
            transaction.inc(key, new_end.wrapping_sub(current_end))?;
            Ok(())
        })?;
        state.base = new_base;
        state.end = new_end;
        Ok(())
    }

    /// 无符号 rebase，逻辑同有符号但用 u64 比较与 wrapping。
    fn rebase_unsigned_locked(
        &self,
        ctx: &Context,
        state: &mut AllocatorState,
        required_base: u64,
        allocate_ids: bool,
    ) -> Result<()> {
        if required_base <= state.base as u64 {
            return Ok(());
        }
        if required_base <= state.end as u64 {
            state.base = required_base as i64;
            return Ok(());
        }
        ctx.check()?;
        let key = self.key_for(state);
        let step = state.step as u64;
        let mut new_base = 0_u64;
        let mut new_end = 0_u64;
        self.store.run_in_transaction(&mut |transaction| {
            let current_end = transaction.get(key)? as u64;
            if allocate_ids {
                new_base = current_end.max(required_base);
                new_end = new_base.min(u64::MAX - step) + step;
            } else if current_end >= required_base {
                new_base = current_end;
                new_end = current_end;
                return Ok(());
            } else {
                new_base = required_base;
                new_end = required_base;
            }
            transaction.inc(key, new_end.wrapping_sub(current_end) as i64)?;
            Ok(())
        })?;
        state.base = new_base as i64;
        state.end = new_end as i64;
        Ok(())
    }

    /// 有符号分配：本地不足时按动态 step 向存储预留，再推进 base。
    fn alloc_signed_locked(
        &self,
        ctx: &Context,
        state: &mut AllocatorState,
        n: u64,
        increment: i64,
        offset: i64,
    ) -> Result<(i64, i64)> {
        let offset_base = offset.wrapping_sub(1);
        if offset_base > state.base {
            self.rebase_signed_locked(ctx, state, offset_base, true)?;
        }
        let mut needed = calc_needed_batch_size(state.base, n as i64, increment, offset, false);
        if i64::MAX.wrapping_sub(state.base) <= needed {
            return Err(autoinc_read_failed("signed auto ID exhausted"));
        }
        // 本地缓存不够，事务内重新计算 needed 并预留。
        if state.base.wrapping_add(needed) > state.end {
            let mut reservation_step = state.step;
            if !self.custom_step && state.end > 0 {
                reservation_step = next_step(state.step, state.last_alloc_time.elapsed());
            }
            let key = self.key_for(state);
            let mut new_base = 0;
            let mut new_end = 0;
            ctx.check()?;
            self.store.run_in_transaction(&mut |transaction| {
                new_base = transaction.get(key)?;
                needed = calc_needed_batch_size(new_base, n as i64, increment, offset, false);
                reservation_step = reservation_step.max(needed);
                let transaction_step = reservation_step.min(i64::MAX.wrapping_sub(new_base));
                if transaction_step < needed {
                    return Err(autoinc_read_failed("signed auto ID exhausted"));
                }
                new_end = transaction.inc(key, transaction_step)?;
                Ok(())
            })?;
            if !self.custom_step {
                state.step = reservation_step;
            }
            state.last_alloc_time = Instant::now();
            if new_base == i64::MAX {
                return Err(autoinc_read_failed("signed auto ID exhausted"));
            }
            state.base = new_base;
            state.end = new_end;
        }
        let minimum = state.base;
        state.base = state.base.wrapping_add(needed);
        Ok((minimum, state.base))
    }

    /// 无符号分配，语义同有符号。
    fn alloc_unsigned_locked(
        &self,
        ctx: &Context,
        state: &mut AllocatorState,
        n: u64,
        increment: i64,
        offset: i64,
    ) -> Result<(i64, i64)> {
        let offset_base = (offset as u64).wrapping_sub(1);
        if offset_base > state.base as u64 {
            self.rebase_unsigned_locked(ctx, state, offset_base, true)?;
        }
        let mut needed = calc_needed_batch_size(state.base, n as i64, increment, offset, true);
        if u64::MAX - state.base as u64 <= needed as u64 {
            return Err(autoinc_read_failed("unsigned auto ID exhausted"));
        }
        if (state.base as u64).wrapping_add(needed as u64) > state.end as u64 {
            let mut reservation_step = state.step as u64;
            if !self.custom_step {
                reservation_step = next_step(state.step, state.last_alloc_time.elapsed()) as u64;
            }
            let key = self.key_for(state);
            let mut new_base = 0_u64;
            let mut new_end = 0_u64;
            ctx.check()?;
            self.store.run_in_transaction(&mut |transaction| {
                new_base = transaction.get(key)? as u64;
                needed = calc_needed_batch_size(new_base as i64, n as i64, increment, offset, true);
                reservation_step = reservation_step.max(needed as u64);
                let transaction_step = reservation_step.min(u64::MAX - new_base);
                if transaction_step < needed as u64 {
                    return Err(autoinc_read_failed("unsigned auto ID exhausted"));
                }
                new_end = transaction.inc(key, transaction_step as i64)? as u64;
                Ok(())
            })?;
            if !self.custom_step {
                state.step = reservation_step as i64;
            }
            state.last_alloc_time = Instant::now();
            if new_base == u64::MAX {
                return Err(autoinc_read_failed("unsigned auto ID exhausted"));
            }
            state.base = new_base as i64;
            state.end = new_end as i64;
        }
        let minimum = state.base;
        state.base = (state.base as u64).wrapping_add(needed as u64) as i64;
        Ok((minimum, state.base))
    }
}

impl Allocator for DefaultAllocator {
    fn alloc(&self, ctx: &Context, n: u64, increment: i64, offset: i64) -> Result<(i64, i64)> {
        if self.state.lock().unwrap().table_id == 0 {
            return Err(AutoIdError::InvalidTableId("Invalid tableID".into()));
        }
        if n == 0 {
            return Ok((0, 0));
        }
        if matches!(
            self.allocator_type,
            AllocatorType::AutoIncrement | AllocatorType::RowId
        ) && !valid_increment_and_offset(increment, offset)
        {
            return Err(invalid_increment_and_offset(increment, offset));
        }
        let mut state = self.state.lock().unwrap();
        if self.is_unsigned {
            self.alloc_unsigned_locked(ctx, &mut state, n, increment, offset)
        } else {
            self.alloc_signed_locked(ctx, &mut state, n, increment, offset)
        }
    }

    /// 为 SEQUENCE 预取一批缓存值；返回 `(base, end, cycle_round)`。
    fn alloc_seq_cache(&self) -> Result<(i64, i64, i64)> {
        let state = self.state.lock().unwrap();
        let sequence = self.sequence.as_ref().ok_or_else(|| {
            AutoIdError::NotImplemented("AllocSeqCache is only supported for sequence".into())
        })?;
        let value_key = self.key_for(&state);
        let cycle_key = self.cycle_key(&state);
        let increment = sequence.increment;
        let mut offset = sequence.start;
        let cache_size = if sequence.cache {
            sequence.cache_value
        } else {
            1
        };
        let mut new_base = 0;
        let mut new_end = 0;
        let mut round = 0;
        self.store.run_in_transaction(&mut |transaction| {
            if sequence.cycle {
                round = transaction.get(cycle_key)?;
                // 已循环过则从 min/max 重新起算。
                if round > 0 {
                    offset = if increment > 0 {
                        sequence.min_value
                    } else {
                        sequence.max_value
                    };
                }
            }
            new_base = transaction.get(value_key)?;
            let mut step = calc_sequence_batch_size(
                new_base,
                cache_size,
                increment,
                offset,
                sequence.min_value,
                sequence.max_value,
            );
            // 触顶且允许 CYCLE：重置水位并递增轮次。
            if matches!(step, Err(AutoIdError::AutoIncrementReadFailed(_))) {
                if !sequence.cycle {
                    return step.map(|_| ());
                }
                if increment > 0 {
                    new_base = sequence.min_value.wrapping_sub(1);
                    offset = sequence.min_value;
                } else {
                    new_base = sequence.max_value.wrapping_add(1);
                    offset = sequence.max_value;
                }
                transaction.put(value_key, new_base)?;
                round += 1;
                transaction.put(cycle_key, round)?;
                step = calc_sequence_batch_size(
                    new_base,
                    cache_size,
                    increment,
                    offset,
                    sequence.min_value,
                    sequence.max_value,
                );
            }
            let step = step?;
            let delta = if increment > 0 {
                step
            } else {
                step.wrapping_neg()
            };
            new_end = transaction.inc(value_key, delta)?;
            Ok(())
        })?;
        Ok((new_base, new_end, round))
    }

    fn rebase(&self, ctx: &Context, new_base: i64, allocate_ids: bool) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if self.is_unsigned {
            self.rebase_unsigned_locked(ctx, &mut state, new_base as u64, allocate_ids)
        } else {
            self.rebase_signed_locked(ctx, &mut state, new_base, allocate_ids)
        }
    }

    /// 强制把全局与本地水位都设到 required_base（允许下调）。
    fn force_rebase(&self, required_base: i64) -> Result<()> {
        if required_base == -1 {
            return Err(autoinc_read_failed(
                "Cannot force rebase the next global ID to '0'",
            ));
        }
        let mut state = self.state.lock().unwrap();
        let key = self.key_for(&state);
        self.store.run_in_transaction(&mut |transaction| {
            let current_end = transaction.get(key)?;
            let step = if self.is_unsigned {
                (required_base as u64).wrapping_sub(current_end as u64) as i64
            } else {
                required_base.wrapping_sub(current_end)
            };
            transaction.inc(key, step)?;
            Ok(())
        })?;
        state.base = required_base;
        state.end = required_base;
        Ok(())
    }

    /// SEQUENCE rebase：若全局已满足要求则返回 `(0, true)`。
    fn rebase_seq(&self, required_base: i64) -> Result<(i64, bool)> {
        let state = self.state.lock().unwrap();
        let sequence = self.sequence.as_ref().ok_or_else(|| {
            AutoIdError::NotImplemented("RebaseSeq is only supported for sequence".into())
        })?;
        let key = self.key_for(&state);
        let mut already_satisfied = false;
        self.store.run_in_transaction(&mut |transaction| {
            let current_end = transaction.get(key)?;
            if (sequence.increment > 0 && current_end >= required_base)
                || (sequence.increment <= 0 && current_end <= required_base)
            {
                already_satisfied = true;
                return Ok(());
            }
            transaction.inc(key, required_base.wrapping_sub(current_end))?;
            Ok(())
        })?;
        if already_satisfied {
            Ok((0, true))
        } else {
            Ok((required_base, false))
        }
    }

    /// 将水位键从旧库表身份复制到新身份（如 RENAME TABLE）。
    fn transfer(&self, database_id: i64, table_id: i64) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if state.database_id == database_id && state.table_id == table_id {
            return Ok(());
        }
        let from = self.key_for(&state);
        let to = AutoIdKey {
            database_id,
            table_id,
            kind: from.kind,
        };
        self.store
            .run_in_transaction(&mut |transaction| transaction.copy_to(from, to))?;
        state.database_id = database_id;
        state.table_id = table_id;
        Ok(())
    }

    fn base(&self) -> i64 {
        self.state.lock().unwrap().base
    }

    fn end(&self) -> i64 {
        self.state.lock().unwrap().end
    }

    fn next_global_auto_id(&self) -> Result<i64> {
        let state = self.state.lock().unwrap();
        let key = self.key_for(&state);
        let mut value = 0;
        self.store.run_in_transaction(&mut |transaction| {
            value = transaction.get(key)?;
            Ok(())
        })?;
        if self.is_unsigned {
            Ok((value as u64).wrapping_add(1) as i64)
        } else {
            Ok(value.wrapping_add(1))
        }
    }

    fn get_type(&self) -> AllocatorType {
        self.allocator_type
    }
}

/// 创建分配器时的依赖：提供 IdStore，可选提供单点分配器。
pub trait Requirement {
    fn store(&self) -> Arc<dyn IdStore>;

    fn single_point_allocator(
        &self,
        _database_id: i64,
        _table_id: i64,
        _is_unsigned: bool,
    ) -> Option<Arc<dyn Allocator>> {
        None
    }
}

/// 按选项创建分配器；`auto_id_cache=1` 且版本足够时优先单点分配器。
pub fn new_allocator(
    requirement: &dyn Requirement,
    database_id: i64,
    table_id: i64,
    is_unsigned: bool,
    allocator_type: AllocatorType,
    options: &[AllocatorOption],
) -> Arc<dyn Allocator> {
    let allocator = DefaultAllocator::with_options(
        requirement.store(),
        database_id,
        table_id,
        is_unsigned,
        allocator_type,
        options,
    );
    let state = allocator.state.lock().unwrap();
    // step==1 的 AutoIncrement：走远程单点服务。
    let use_single_point = allocator.custom_step
        && state.step == 1
        && allocator.table_version >= 5
        && allocator_type == AllocatorType::AutoIncrement;
    drop(state);
    if use_single_point {
        if let Some(single_point) =
            requirement.single_point_allocator(database_id, table_id, is_unsigned)
        {
            return single_point;
        }
    }
    Arc::new(allocator)
}

/// 根据表信息挂载 RowId / AutoIncrement / AutoRandom / Sequence 分配器。
pub fn new_allocators_from_table_info(
    requirement: &dyn Requirement,
    database_id: i64,
    table: &TableInfo,
) -> Allocators {
    let options = [
        AllocatorOption::CustomStep(table.auto_id_cache),
        AllocatorOption::TableInfoVersion(table.version),
    ];
    let mut allocators = Vec::new();
    let has_row_id = !table.pk_is_handle && !table.is_common_handle;
    if has_row_id || (table.has_auto_increment_column && !table.separate_auto_increment) {
        allocators.push(new_allocator(
            requirement,
            database_id,
            table.id,
            table.auto_increment_unsigned,
            AllocatorType::RowId,
            &options,
        ));
    }
    if table.has_auto_increment_column && table.separate_auto_increment {
        allocators.push(new_allocator(
            requirement,
            database_id,
            table.id,
            table.auto_increment_unsigned,
            AllocatorType::AutoIncrement,
            &options,
        ));
    }
    if table.auto_random_bits > 0 {
        allocators.push(new_allocator(
            requirement,
            database_id,
            table.id,
            table.auto_random_unsigned,
            AllocatorType::AutoRandom,
            &options,
        ));
    }
    if let Some(sequence) = &table.sequence {
        allocators.push(Arc::new(DefaultAllocator::new_sequence(
            requirement.store(),
            database_id,
            table.id,
            sequence.clone(),
        )));
    }
    Allocators::new(table.separate_auto_increment, allocators)
}

/// 校验 MySQL 风格 auto_increment increment/offset 是否在合法范围。
pub fn valid_increment_and_offset(increment: i64, offset: i64) -> bool {
    (MIN_INCREMENT..=MAX_INCREMENT).contains(&increment)
        && (MIN_INCREMENT..=MAX_INCREMENT).contains(&offset)
}

/// 按上一批消耗耗时调节下一步长，夹在 [MIN_STEP, MAX_STEP]。
pub fn next_step(current_step: i64, consume_duration: Duration) -> i64 {
    if consume_duration.is_zero() {
        return MAX_STEP;
    }
    let consume_rate = DEFAULT_CONSUME_TIME.as_secs_f64() / consume_duration.as_secs_f64();
    ((current_step as f64 * consume_rate) as i64).clamp(MIN_STEP, MAX_STEP)
}

/// 计算为发放 n 个满足 `id ≡ offset (mod increment)` 的 ID，base 需要前进多少。
pub fn calc_needed_batch_size(
    base: i64,
    n: i64,
    increment: i64,
    offset: i64,
    is_unsigned: bool,
) -> i64 {
    if increment == 1 {
        return n;
    }
    if is_unsigned {
        let first = seek_to_first_auto_id_unsigned(base as u64, increment as u64, offset as u64);
        return first
            .wrapping_add((n as u64).wrapping_sub(1).wrapping_mul(increment as u64))
            .wrapping_sub(base as u64) as i64;
    }
    seek_to_first_auto_id_signed(base, increment, offset)
        .wrapping_add(n.wrapping_sub(1).wrapping_mul(increment))
        .wrapping_sub(base)
}

/// 计算 SEQUENCE 一次缓存批的步长；触顶返回错误。
pub fn calc_sequence_batch_size(
    base: i64,
    size: i64,
    increment: i64,
    offset: i64,
    min_value: i64,
    max_value: i64,
) -> Result<i64> {
    if increment > 0 {
        if increment == 1 {
            if base >= max_value {
                return Err(autoinc_read_failed("sequence has reached its maximum"));
            }
            return Ok(max_value.wrapping_sub(base).min(size));
        }
        let (first, found) =
            seek_to_first_sequence_value(base, increment, offset, min_value, max_value);
        if !found {
            return Err(autoinc_read_failed("sequence has reached its maximum"));
        }
        if max_value.wrapping_sub(first) < size.wrapping_sub(1).wrapping_mul(increment) {
            Ok(max_value.wrapping_sub(base))
        } else {
            Ok(first
                .wrapping_sub(base)
                .wrapping_add(size.wrapping_sub(1).wrapping_mul(increment)))
        }
    } else {
        if increment == -1 {
            if base <= min_value {
                return Err(autoinc_read_failed("sequence has reached its minimum"));
            }
            return Ok(base.wrapping_sub(min_value).min(size));
        }
        let (first, found) =
            seek_to_first_sequence_value(base, increment, offset, min_value, max_value);
        if !found {
            return Err(autoinc_read_failed("sequence has reached its minimum"));
        }
        let decrement = increment.wrapping_neg();
        if first.wrapping_sub(min_value) < size.wrapping_sub(1).wrapping_mul(decrement) {
            Ok(base.wrapping_sub(min_value))
        } else {
            Ok(base
                .wrapping_sub(first)
                .wrapping_add(size.wrapping_sub(1).wrapping_mul(decrement)))
        }
    }
}

/// 在有符号全序上寻找下一个满足序列同余条件的值；找不到返回 `(0, false)`。
///
/// 通过异或符号位把 i64 映射为可比的 u64，避免有符号溢出干扰模运算。
pub fn seek_to_first_sequence_value(
    base: i64,
    increment: i64,
    offset: i64,
    min_value: i64,
    max_value: i64,
) -> (i64, bool) {
    if increment > 0 {
        if base >= max_value {
            return (0, false);
        }
        let unsigned_max = encode_int_to_cmp_uint(max_value);
        let unsigned_base = encode_int_to_cmp_uint(base);
        let unsigned_offset = encode_int_to_cmp_uint(offset);
        let unsigned_increment = increment as u64;
        // 剩余区间不足一个 increment 时线性扫描。
        if unsigned_max - unsigned_base < unsigned_increment {
            for value in unsigned_base + 1..=unsigned_max {
                if value.wrapping_sub(unsigned_offset) % unsigned_increment == 0 {
                    return (decode_cmp_uint_to_int(value), true);
                }
            }
            return (0, false);
        }
        let quotient = unsigned_base
            .wrapping_add(unsigned_increment)
            .wrapping_sub(unsigned_offset)
            / unsigned_increment;
        return (
            decode_cmp_uint_to_int(
                quotient
                    .wrapping_mul(unsigned_increment)
                    .wrapping_add(unsigned_offset),
            ),
            true,
        );
    }
    if base <= min_value {
        return (0, false);
    }
    let unsigned_min = encode_int_to_cmp_uint(min_value);
    let unsigned_base = encode_int_to_cmp_uint(base);
    let unsigned_offset = encode_int_to_cmp_uint(offset);
    let unsigned_increment = increment.wrapping_neg() as u64;
    if unsigned_base - unsigned_min < unsigned_increment {
        for value in (unsigned_min..unsigned_base).rev() {
            if unsigned_offset.wrapping_sub(value) % unsigned_increment == 0 {
                return (decode_cmp_uint_to_int(value), true);
            }
        }
        return (0, false);
    }
    let quotient = unsigned_offset
        .wrapping_sub(unsigned_base)
        .wrapping_add(unsigned_increment)
        / unsigned_increment;
    (
        decode_cmp_uint_to_int(
            unsigned_offset.wrapping_sub(quotient.wrapping_mul(unsigned_increment)),
        ),
        true,
    )
}

/// 有符号：寻找大于 base 且满足 `id ≡ offset (mod increment)` 的第一个 ID。
pub fn seek_to_first_auto_id_signed(base: i64, increment: i64, offset: i64) -> i64 {
    let quotient = base
        .wrapping_add(increment)
        .wrapping_sub(offset)
        .wrapping_div(increment);
    quotient.wrapping_mul(increment).wrapping_add(offset)
}

/// 无符号版本的 [`seek_to_first_auto_id_signed`]。
pub fn seek_to_first_auto_id_unsigned(base: u64, increment: u64, offset: u64) -> u64 {
    let quotient = base.wrapping_add(increment).wrapping_sub(offset) / increment;
    quotient.wrapping_mul(increment).wrapping_add(offset)
}

/// 符号位掩码：用于 i64↔可比 u64 映射。
const SIGN_MASK: u64 = 0x8000_0000_0000_0000;

/// 将有符号整数编码为保持全序的无符号整数。
pub fn encode_int_to_cmp_uint(value: i64) -> u64 {
    value as u64 ^ SIGN_MASK
}

/// [`encode_int_to_cmp_uint`] 的逆变换。
pub fn decode_cmp_uint_to_int(value: u64) -> i64 {
    (value ^ SIGN_MASK) as i64
}

/// AUTO_RANDOM ID 布局：高位分片 + 低位增量。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShardIdFormat {
    pub unsigned: bool,
    pub shard_bits: u64,
    pub incremental_bits: u64,
}

impl ShardIdFormat {
    /// 由分片位与 range bits 推导增量位数；有符号再减 1 位符号位。
    pub fn new(unsigned: bool, shard_bits: u64, range_bits: u64) -> Self {
        let mut incremental_bits = if range_bits == 0 {
            ROW_ID_BIT_LENGTH.wrapping_sub(shard_bits)
        } else {
            range_bits.wrapping_sub(shard_bits)
        };
        if !unsigned {
            incremental_bits = incremental_bits.wrapping_sub(1);
        }
        Self {
            unsigned,
            shard_bits,
            incremental_bits,
        }
    }

    /// 增量部分的位掩码。
    pub fn incremental_mask(&self) -> i64 {
        let shifted = if self.incremental_bits < i64::BITS as u64 {
            1_i64 << self.incremental_bits
        } else {
            0
        };
        shifted.wrapping_sub(1)
    }

    /// 增量部分可表示的最大无符号容量。
    pub fn incremental_bits_capacity(&self) -> u64 {
        self.incremental_mask() as u64
    }

    /// 组合分片值与增量 ID 为最终 row id。
    pub fn compose(&self, shard: i64, id: i64) -> i64 {
        let shard_shifted = if self.shard_bits < i64::BITS as u64 {
            1_i64 << self.shard_bits
        } else {
            0
        };
        let shard_mask = shard_shifted.wrapping_sub(1);
        let shard_segment = if self.incremental_bits < i64::BITS as u64 {
            (shard & shard_mask) << self.incremental_bits
        } else {
            0
        };
        shard_segment | id
    }
}

/// 分配器运行时统计（alloc/rebase 次数与快照/提交细节字符串）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AllocatorRuntimeStats {
    pub snapshot_stats: String,
    pub commit_stats: String,
    alloc_count: i32,
    rebase_count: i32,
}

impl AllocatorRuntimeStats {
    /// 记录一次分配。
    pub fn record_alloc(&mut self) {
        self.alloc_count += 1;
    }

    /// 记录一次 rebase。
    pub fn record_rebase(&mut self) {
        self.rebase_count += 1;
    }

    /// 合并另一份统计（非空的 snapshot/commit 字符串覆盖）。
    pub fn merge(&mut self, other: &Self) {
        if !other.snapshot_stats.is_empty() {
            self.snapshot_stats = other.snapshot_stats.clone();
        }
        if !other.commit_stats.is_empty() {
            self.commit_stats = other.commit_stats.clone();
        }
    }
}

impl fmt::Display for AllocatorRuntimeStats {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.alloc_count == 0 && self.rebase_count == 0 {
            return Ok(());
        }
        let mut fields = Vec::new();
        if self.alloc_count > 0 {
            fields.push(format!("alloc_cnt: {}", self.alloc_count));
        }
        if self.rebase_count > 0 {
            fields.push(format!("rebase_cnt: {}", self.rebase_count));
        }
        if !self.snapshot_stats.is_empty() {
            fields.push(self.snapshot_stats.clone());
        }
        if !self.commit_stats.is_empty() {
            fields.push(self.commit_stats.clone());
        }
        write!(formatter, "auto_id_allocator: {{{}}}", fields.join(", "))
    }
}
