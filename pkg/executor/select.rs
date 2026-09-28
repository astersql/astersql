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

// SELECT 相关执行器与资源追踪。
//
// 对应 Go 的 `select.go`：提供全局内存/磁盘/ANALYZE 配额追踪（超限 panic）、
// `SelectLockExec`（SELECT FOR UPDATE / SHARE 悲观加锁）、`LimitExec`、`TableDualExec`、
// `SelectionExec`（过滤）、信息表扫描 `TableScanExec`、`MaxOneRowExec`（标量子查询最多一行），
// 以及语句上下文重置与锁键去重等辅助函数。`SelectRuntime` 抽象会话/存储边界。
#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 本地临时空间配额耗尽时的 panic 文案。
pub const globalPanicStorageExceed: &str = "Out Of Quota For Local Temporary Space!";
/// 全局内存限额耗尽时的 panic 文案。
pub const globalPanicMemoryExceed: &str = "Out Of Global Memory Limit!";
/// ANALYZE 全局内存限额耗尽时的 panic 文案。
pub const globalPanicAnalyzeMemoryExceed: &str = "Out Of Global Analyze Memory Limit!";

/// `CHECK TABLE` 快速校验使用的桶大小（字节），可运行时调整。
pub static CheckTableFastBucketSize: AtomicI64 = AtomicI64::new(1024);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 资源配额类别：本地磁盘、全局内存、ANALYZE 内存。
pub enum ResourceKind {
    /// 本地临时空间（spill 磁盘）。
    Storage,
    /// 全局查询内存。
    Memory,
    /// ANALYZE 专用内存。
    AnalyzeMemory,
    /// 未分类资源。
    Unknown,
}

#[derive(Debug)]
/// 资源消耗追踪器：原子累计已用量与限额（-1 表示不限制）。
pub struct ResourceTracker {
    /// 已消耗字节数。
    consumed: AtomicI64,
    /// 限额；负值表示不限制。
    limit: AtomicI64,
}

impl ResourceTracker {
    /// 构造追踪器。
    fn new(limit: i64) -> Self {
        Self {
            consumed: AtomicI64::new(0),
            limit: AtomicI64::new(limit),
        }
    }

    /// 当前已消耗量。
    pub fn consumed(&self) -> i64 {
        self.consumed.load(Ordering::Acquire)
    }

    /// 当前限额。
    pub fn limit(&self) -> i64 {
        self.limit.load(Ordering::Acquire)
    }
}

/// 全局查询内存追踪器。
static GLOBAL_MEMORY: OnceLock<Arc<ResourceTracker>> = OnceLock::new();
/// 全局本地磁盘追踪器。
static GLOBAL_DISK: OnceLock<Arc<ResourceTracker>> = OnceLock::new();
/// ANALYZE 全局内存追踪器。
static GLOBAL_ANALYZE_MEMORY: OnceLock<Arc<ResourceTracker>> = OnceLock::new();
/// 后端资源监控是否已启动。
static BACKEND_STARTED: AtomicBool = AtomicBool::new(false);

/// 初始化三类全局资源追踪器与默认桶大小。
pub fn init() {
    let _ = GLOBAL_MEMORY.set(Arc::new(ResourceTracker::new(-1)));
    let _ = GLOBAL_DISK.set(Arc::new(ResourceTracker::new(-1)));
    let _ = GLOBAL_ANALYZE_MEMORY.set(Arc::new(ResourceTracker::new(-1)));
    CheckTableFastBucketSize.store(1024, Ordering::Release);
}

/// 标记资源监控后端已启动。
pub fn Start() {
    BACKEND_STARTED.store(true, Ordering::Release);
}

/// 标记资源监控后端已停止。
pub fn Stop() {
    BACKEND_STARTED.store(false, Ordering::Release);
}

/// 配额超限时的动作：加锁后按资源类别 panic（与 Go 行为一致）。
pub struct globalPanicOnExceed {
    /// 串行化 panic 动作，避免并发重复触发。
    mutex: Mutex<()>,
}

impl Default for globalPanicOnExceed {
    fn default() -> Self {
        Self {
            mutex: Mutex::new(()),
        }
    }
}

impl globalPanicOnExceed {
    /// 按资源类别抛出对应 panic 文案，永不返回。
    pub fn Action(&self, kind: ResourceKind) -> ! {
        let _guard = self.mutex.lock().expect("OOM action lock poisoned");
        let message = match kind {
            ResourceKind::Storage => globalPanicStorageExceed,
            ResourceKind::Memory => globalPanicMemoryExceed,
            ResourceKind::AnalyzeMemory => globalPanicAnalyzeMemoryExceed,
            ResourceKind::Unknown => "Out of Unknown Resource Quota!",
        };
        panic!("{message}")
    }

    /// 动作优先级（最大，表示最高优先级）。
    pub fn GetPriority(&self) -> i64 {
        i64::MAX
    }
}

/// 数据源执行器接口：暴露底层物理表。
pub trait dataSourceExecutor {
    /// 物理表类型。
    type Table;
    /// 返回当前扫描的表。
    fn Table(&self) -> &Self::Table;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// SELECT 行锁模式。
pub enum LockMode {
    /// `FOR UPDATE` 排他锁。
    ForUpdate,
    /// `FOR SHARE` / `LOCK IN SHARE MODE` 共享锁。
    Shared,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 悲观锁请求上下文：键集、等待超时与模式等。
pub struct LockContext<K> {
    /// 待加锁的键列表。
    pub keys: Vec<K>,
    /// 锁等待超时。
    pub wait_time: Duration,
    /// 加锁模式。
    pub mode: LockMode,
    /// 是否在加锁时返回已有值。
    pub return_values: bool,
    /// 是否检查键是否存在。
    pub check_existence: bool,
    /// 仅当键已存在时才加锁。
    pub lock_only_if_exists: bool,
}

/// Production boundary shared by the SELECT executors.
///
/// Implementations connect these algorithms to sessionctx, chunk, table and
/// TiKV. Every externally visible action is required; there is no successful
/// placeholder implementation.
/// 生产边界：连接会话、chunk、表与 TiKV；所有对外可见操作均为必选实现。
pub trait SelectRuntime {
    /// 会话 / 执行上下文。
    type Context;
    /// 列式结果块。
    type Chunk;
    /// 单行类型。
    type Row: Clone;
    /// 行锁键（TiKV key）。
    type Key: Clone + Eq + std::hash::Hash;
    /// 物理表。
    type Table;
    /// 通用语句。
    type Statement;
    /// UPDATE 语句。
    type UpdateStatement;
    /// DELETE 语句。
    type DeleteStatement;
    /// 存储快照。
    type Snapshot;
    /// 错误类型。
    type Error;

    /// 清空 chunk 行数据。
    fn reset_chunk(&mut self, chunk: &mut Self::Chunk);
    /// 按容量增长并重置 chunk。
    fn grow_and_reset_chunk(&mut self, chunk: &mut Self::Chunk, capacity: usize);
    /// 当前 chunk 行数。
    fn chunk_rows(&self, chunk: &Self::Chunk) -> usize;
    /// chunk 容量。
    fn chunk_capacity(&self, chunk: &Self::Chunk) -> usize;
    /// 截断 chunk 至指定行数。
    fn truncate_chunk(&mut self, chunk: &mut Self::Chunk, rows: usize);
    /// 追加一行全 NULL。
    fn append_null_row(&mut self, chunk: &mut Self::Chunk, columns: usize);
    /// 交换两个 chunk 内容。
    fn swap_chunk(&mut self, destination: &mut Self::Chunk, source: &mut Self::Chunk);
    /// 新建指定容量的 chunk。
    fn new_chunk(&mut self, capacity: usize) -> Self::Chunk;

    /// 打开子执行器。
    fn open_child(&mut self, context: &mut Self::Context, child: usize) -> Result<(), Self::Error>;
    /// 关闭子执行器。
    fn close_child(&mut self, child: usize) -> Result<(), Self::Error>;
    /// 从子执行器拉取一批行。
    fn next_child(
        &mut self,
        context: &mut Self::Context,
        child: usize,
        chunk: &mut Self::Chunk,
    ) -> Result<(), Self::Error>;
    /// 子执行器初始 chunk 容量。
    fn child_initial_capacity(&self, child: usize) -> usize;
    /// 子执行器最大 chunk 行数。
    fn child_max_chunk_size(&self, child: usize) -> usize;
    /// 输出 schema 列数。
    fn schema_columns(&self) -> usize;

    /// 当前语句是否需要加行锁。
    fn select_lock_enabled(&self) -> bool;
    /// 行锁模式。
    fn select_lock_mode(&self) -> LockMode;
    /// 锁等待时间配置。
    fn select_lock_wait_time(&self) -> i64;
    /// 从结果 chunk 提取待加锁键。
    fn lock_keys_from_chunk(&mut self, chunk: &Self::Chunk) -> Result<Vec<Self::Key>, Self::Error>;
    /// 键是否属于临时表（无需加锁）。
    fn key_is_temporary_table(&self, key: &Self::Key) -> bool;
    /// 键是否属于锁表元数据。
    fn key_is_lock_table(&self, key: &Self::Key) -> bool;
    /// 当前语句是否配置了锁表白名单。
    fn lock_table_filter_enabled(&self) -> bool {
        false
    }
    /// 键是否为未修改索引（跳过加锁）。
    fn key_is_untouched_index(&self, key: &Self::Key) -> bool;
    /// 执行悲观锁（pessimistic lock）请求。
    fn pessimistic_lock(
        &mut self,
        context: &mut Self::Context,
        lock_context: &mut LockContext<Self::Key>,
    ) -> Result<(), Self::Error>;
    /// 记录成功加锁的键数量。
    fn record_locked_keys(&mut self, keys: usize);
    /// 判断错误是否为死锁。
    fn lock_error_is_deadlock(&self, error: &Self::Error) -> bool;
    /// 记录死锁诊断信息。
    fn record_deadlock(&mut self, error: &Self::Error);
    /// 语句最大执行截止时间。
    fn max_execution_deadline(&self) -> Option<Instant>;
    /// 超时/中断错误。
    fn interrupted_error(&self) -> Self::Error;
    /// 标量子查询返回多行错误。
    fn subquery_more_than_one_row_error(&self) -> Self::Error;
    /// 将锁等待配置转为超时时长。
    fn lock_wait_timeout(&self, wait_time: i64) -> Result<Duration, Self::Error>;

    /// 过滤是否走批处理路径。
    fn selection_batched(&self) -> bool;
    /// 向量化计算过滤条件，返回每行是否选中。
    fn selection_vectorized_filter(
        &mut self,
        context: &mut Self::Context,
        input: &Self::Chunk,
    ) -> Result<Vec<bool>, Self::Error>;
    /// 非批模式下按选中位追加行，并推进游标。
    fn selection_append_selected(
        &mut self,
        output: &mut Self::Chunk,
        input: &Self::Chunk,
        selected: &[bool],
        cursor: &mut usize,
    );
    /// 批模式下一次性将选中行写入输出。
    fn selection_take_selected(
        &mut self,
        output: &mut Self::Chunk,
        input: &Self::Chunk,
        selected: &[bool],
    );

    /// 扫描信息表/虚表，返回全部 chunk。
    fn table_scan_all(
        &mut self,
        context: &mut Self::Context,
    ) -> Result<Vec<Self::Chunk>, Self::Error>;

    /// 按语句重置 StatementContext。
    fn reset_statement_context(&mut self, statement: &Self::Statement) -> Result<(), Self::Error>;
    /// 重置 UPDATE 语句上下文。
    fn reset_update_statement_context(&mut self, statement: &Self::UpdateStatement);
    /// 重置 DELETE 语句上下文。
    fn reset_delete_statement_context(&mut self, statement: &Self::DeleteStatement);
    /// 为 TopSQL 设置快照选项。
    fn set_top_sql_snapshot_options(&mut self, snapshot: &mut Self::Snapshot);
    /// 是否为弱一致读。
    fn weak_consistency_read(&self, statement: &Self::Statement) -> bool;
}

/// `SELECT ... FOR UPDATE/SHARE` 加锁执行器：从子结果提取键并悲观加锁。
pub struct SelectLockExec<R: SelectRuntime> {
    /// 运行时边界。
    pub runtime: R,
    /// 本轮待加锁键缓冲。
    pub keys: Vec<R::Key>,
}

impl<R: SelectRuntime> SelectLockExec<R> {
    /// 打开子执行器并清空键缓冲。
    pub fn Open(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.keys.clear();
        self.runtime.open_child(context, 0)
    }

    /// 拉取子结果；若需加锁则过滤键后执行悲观锁。
    /// 死锁时记录诊断信息。
    pub fn Next(
        &mut self,
        context: &mut R::Context,
        request: &mut R::Chunk,
    ) -> Result<(), R::Error> {
        self.runtime.reset_chunk(request);
        self.runtime.next_child(context, 0, request)?;
        if !self.runtime.select_lock_enabled() {
            return Ok(());
        }

        // 与 Go 一致，非空批次只累计键；读到 EOF 后再一次性加锁。
        if self.runtime.chunk_rows(request) != 0 {
            self.keys
                .extend(self.runtime.lock_keys_from_chunk(request)?);
            return Ok(());
        }
        if self.keys.is_empty() {
            return Ok(());
        }

        // 过滤临时表与未触达索引键；若配置了 LOCK TABLES，只保留白名单键。
        filterTemporaryTableKeys(&self.runtime, &mut self.keys);
        filterLockTableKeys(&self.runtime, &mut self.keys);
        self.keys
            .retain(|key| !self.runtime.key_is_untouched_index(key));
        if self.keys.is_empty() {
            return Ok(());
        }

        checkMaxExecutionTimeExceeded(&self.runtime)?;
        let mut lock_context = newLockCtx(
            &self.runtime,
            self.runtime.select_lock_wait_time(),
            self.keys.clone(),
            self.runtime.select_lock_mode() == LockMode::Shared,
        )?;
        let result = doLockKeys(&mut self.runtime, context, &mut lock_context);
        if let Err(error) = &result {
            if self.runtime.lock_error_is_deadlock(error) {
                self.runtime.record_deadlock(error);
            }
        } else {
            self.runtime.record_locked_keys(lock_context.keys.len());
        }
        result
    }
}

/// 若超过 `max_execution_time` 截止时刻则返回中断错误。
pub fn checkMaxExecutionTimeExceeded<R: SelectRuntime>(runtime: &R) -> Result<(), R::Error> {
    if runtime
        .max_execution_deadline()
        .is_some_and(|deadline| Instant::now() >= deadline)
    {
        Err(runtime.interrupted_error())
    } else {
        Ok(())
    }
}

/// 构造锁上下文：解析等待超时并选择共享/排他模式。
pub fn newLockCtx<R: SelectRuntime>(
    runtime: &R,
    lock_wait_time: i64,
    keys: Vec<R::Key>,
    in_shared_mode: bool,
) -> Result<LockContext<R::Key>, R::Error> {
    let wait_time = runtime.lock_wait_timeout(lock_wait_time)?;
    Ok(LockContext {
        keys,
        wait_time,
        mode: if in_shared_mode {
            LockMode::Shared
        } else {
            LockMode::ForUpdate
        },
        return_values: false,
        check_existence: false,
        lock_only_if_exists: false,
    })
}

/// 在再次检查执行时限后发起悲观加锁。
pub fn doLockKeys<R: SelectRuntime>(
    runtime: &mut R,
    context: &mut R::Context,
    lock_context: &mut LockContext<R::Key>,
) -> Result<(), R::Error> {
    checkMaxExecutionTimeExceeded(runtime)?;
    runtime.pessimistic_lock(context, lock_context)
}

/// 去掉属于临时表的键。
pub fn filterTemporaryTableKeys<R: SelectRuntime>(runtime: &R, keys: &mut Vec<R::Key>) {
    keys.retain(|key| !runtime.key_is_temporary_table(key));
}

/// 配置了锁表白名单时，仅保留属于锁表的键；未配置时保留全部键。
pub fn filterLockTableKeys<R: SelectRuntime>(runtime: &R, keys: &mut Vec<R::Key>) {
    if runtime.lock_table_filter_enabled() {
        keys.retain(|key| runtime.key_is_lock_table(key));
    }
}

/// `LimitExec` 所需的最小运行时边界。
///
/// Go 的 `LimitExec` 只依赖 chunk 与单个子执行器。将这组操作从完整的
/// `SelectRuntime` 中单独表达后，物理计划和 session 行源可以复用同一个
/// open/next/close 与 requiredRows 实现。
pub trait LimitRuntime {
    /// 执行上下文。
    type Context;
    /// 分块结果。
    type Chunk;
    /// 执行错误。
    type Error;

    /// 清空输出 chunk。
    fn reset_chunk(&mut self, chunk: &mut Self::Chunk);
    /// 返回 chunk 行数。
    fn chunk_rows(&self, chunk: &Self::Chunk) -> usize;
    /// 返回 chunk 容量。
    fn chunk_capacity(&self, chunk: &Self::Chunk) -> usize;
    /// 截断 chunk。
    fn truncate_chunk(&mut self, chunk: &mut Self::Chunk, rows: usize);
    /// 创建指定容量的 chunk。
    fn new_chunk(&mut self, capacity: usize) -> Self::Chunk;
    /// 打开唯一子执行器。
    fn open_child(&mut self, context: &mut Self::Context, child: usize) -> Result<(), Self::Error>;
    /// 关闭唯一子执行器。
    fn close_child(&mut self, child: usize) -> Result<(), Self::Error>;
    /// 从唯一子执行器读取下一批。
    fn next_child(
        &mut self,
        context: &mut Self::Context,
        child: usize,
        chunk: &mut Self::Chunk,
    ) -> Result<(), Self::Error>;
    /// 按选择位图复制行。
    fn selection_take_selected(
        &mut self,
        output: &mut Self::Chunk,
        input: &Self::Chunk,
        selected: &[bool],
    );
}

/// 完整 SELECT 运行时天然满足 LIMIT 所需的最小边界。
impl<R: SelectRuntime> LimitRuntime for R {
    type Context = R::Context;
    type Chunk = R::Chunk;
    type Error = R::Error;

    fn reset_chunk(&mut self, chunk: &mut Self::Chunk) {
        SelectRuntime::reset_chunk(self, chunk);
    }

    fn chunk_rows(&self, chunk: &Self::Chunk) -> usize {
        SelectRuntime::chunk_rows(self, chunk)
    }

    fn chunk_capacity(&self, chunk: &Self::Chunk) -> usize {
        SelectRuntime::chunk_capacity(self, chunk)
    }

    fn truncate_chunk(&mut self, chunk: &mut Self::Chunk, rows: usize) {
        SelectRuntime::truncate_chunk(self, chunk, rows);
    }

    fn new_chunk(&mut self, capacity: usize) -> Self::Chunk {
        SelectRuntime::new_chunk(self, capacity)
    }

    fn open_child(&mut self, context: &mut Self::Context, child: usize) -> Result<(), Self::Error> {
        SelectRuntime::open_child(self, context, child)
    }

    fn close_child(&mut self, child: usize) -> Result<(), Self::Error> {
        SelectRuntime::close_child(self, child)
    }

    fn next_child(
        &mut self,
        context: &mut Self::Context,
        child: usize,
        chunk: &mut Self::Chunk,
    ) -> Result<(), Self::Error> {
        SelectRuntime::next_child(self, context, child, chunk)
    }

    fn selection_take_selected(
        &mut self,
        output: &mut Self::Chunk,
        input: &Self::Chunk,
        selected: &[bool],
    ) {
        SelectRuntime::selection_take_selected(self, output, input, selected);
    }
}

/// `LIMIT offset, count` 执行器：跳过 begin 之前的行，截断到 end。
pub struct LimitExec<R: LimitRuntime> {
    /// 运行时边界。
    pub runtime: R,
    /// 跳过行数（OFFSET）。
    pub begin: u64,
    /// 结束位置（OFFSET+COUNT）。
    pub end: u64,
    /// 已消费的子行游标。
    pub cursor: u64,
    /// 是否已产生过首批有效输出。
    pub meet_first_batch: bool,
}

impl<R: LimitRuntime> LimitExec<R> {
    /// 打开 LIMIT 执行器。
    pub fn Open(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.open(context)
    }

    /// 重置游标并打开子执行器。
    pub fn open(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.cursor = 0;
        self.meet_first_batch = self.begin == 0;
        self.runtime.open_child(context, 0)
    }

    /// 跳过 OFFSET 行后输出至多剩余 COUNT 行；必要时截断 chunk。
    pub fn Next(
        &mut self,
        context: &mut R::Context,
        request: &mut R::Chunk,
    ) -> Result<(), R::Error> {
        self.runtime.reset_chunk(request);
        if self.cursor >= self.end {
            return Ok(());
        }

        // 尚未越过 OFFSET：丢弃整批或切出首个有效窗口。
        while !self.meet_first_batch {
            let mut input = self.runtime.new_chunk(self.adjustRequiredRows(request));
            self.runtime.next_child(context, 0, &mut input)?;
            let rows = self.runtime.chunk_rows(&input) as u64;
            if rows == 0 {
                return Ok(());
            }
            if self.cursor.saturating_add(rows) <= self.begin {
                self.cursor = self.cursor.saturating_add(rows);
                continue;
            }
            let skip = self.begin.saturating_sub(self.cursor) as usize;
            let take = (self.end - self.begin).min(rows.saturating_sub(skip as u64)) as usize;
            let selected = (0..rows as usize)
                .map(|index| index >= skip && index < skip + take)
                .collect::<Vec<_>>();
            self.runtime
                .selection_take_selected(request, &input, &selected);
            self.cursor = self.begin.saturating_add(take as u64);
            self.meet_first_batch = true;
            return Ok(());
        }

        self.runtime.next_child(context, 0, request)?;
        let rows = self.runtime.chunk_rows(request) as u64;
        let remaining = self.end.saturating_sub(self.cursor);
        if rows > remaining {
            self.runtime.truncate_chunk(request, remaining as usize);
            self.cursor = self.end;
        } else {
            self.cursor = self.cursor.saturating_add(rows);
        }
        Ok(())
    }

    /// 关闭子执行器。
    pub fn Close(&mut self) -> Result<(), R::Error> {
        self.runtime.close_child(0)
    }

    /// 按剩余窗口与 OFFSET 调整向子执行器请求的行数。
    pub fn adjustRequiredRows(&self, request: &R::Chunk) -> usize {
        let remaining = self.end.saturating_sub(self.cursor) as usize;
        let offset = if self.meet_first_batch {
            0
        } else {
            self.begin.saturating_sub(self.cursor) as usize
        };
        self.runtime
            .chunk_capacity(request)
            .min(remaining.saturating_add(offset))
            .max(1)
    }
}

/// 通用 LIMIT 行块。
struct LimitValueChunk<T> {
    /// 当前批次行。
    rows: Vec<T>,
}

/// 将已解码行作为单子节点分块喂给 `LimitExec`。
struct LimitValueRuntime<T> {
    /// 子节点全部输入行。
    rows: Vec<T>,
    /// 子节点读取位置。
    cursor: usize,
    /// 子节点单批最大行数。
    max_chunk_size: usize,
    /// 子节点是否已经打开。
    opened: bool,
}

impl<T: Clone> LimitRuntime for LimitValueRuntime<T> {
    type Context = ();
    type Chunk = LimitValueChunk<T>;
    type Error = String;

    fn reset_chunk(&mut self, chunk: &mut Self::Chunk) {
        chunk.rows.clear();
    }

    fn chunk_rows(&self, chunk: &Self::Chunk) -> usize {
        chunk.rows.len()
    }

    fn chunk_capacity(&self, chunk: &Self::Chunk) -> usize {
        chunk.rows.capacity().max(1)
    }

    fn truncate_chunk(&mut self, chunk: &mut Self::Chunk, rows: usize) {
        chunk.rows.truncate(rows);
    }

    fn new_chunk(&mut self, capacity: usize) -> Self::Chunk {
        LimitValueChunk {
            rows: Vec::with_capacity(capacity),
        }
    }

    fn open_child(
        &mut self,
        _context: &mut Self::Context,
        child: usize,
    ) -> Result<(), Self::Error> {
        if child != 0 {
            return Err("LIMIT requires exactly one child".to_owned());
        }
        self.cursor = 0;
        self.opened = true;
        Ok(())
    }

    fn close_child(&mut self, child: usize) -> Result<(), Self::Error> {
        if child != 0 {
            return Err("LIMIT requires exactly one child".to_owned());
        }
        self.opened = false;
        Ok(())
    }

    fn next_child(
        &mut self,
        _context: &mut Self::Context,
        child: usize,
        chunk: &mut Self::Chunk,
    ) -> Result<(), Self::Error> {
        if child != 0 {
            return Err("LIMIT requires exactly one child".to_owned());
        }
        if !self.opened {
            return Err("LIMIT child is not open".to_owned());
        }
        chunk.rows.clear();
        let requested = chunk.rows.capacity().max(1).min(self.max_chunk_size);
        let end = self.cursor.saturating_add(requested).min(self.rows.len());
        chunk.rows.extend_from_slice(&self.rows[self.cursor..end]);
        self.cursor = end;
        Ok(())
    }

    fn selection_take_selected(
        &mut self,
        output: &mut Self::Chunk,
        input: &Self::Chunk,
        selected: &[bool],
    ) {
        output.rows.extend(
            input
                .rows
                .iter()
                .zip(selected)
                .filter_map(|(row, selected)| selected.then_some(row.clone())),
        );
    }
}

/// 通过规范 `LimitExec` 生命周期对任意已解码行类型执行 LIMIT/OFFSET。
pub fn ExecuteLimitValues<T: Clone>(
    rows: Vec<T>,
    offset: usize,
    count: usize,
    max_chunk_size: usize,
) -> Result<Vec<T>, String> {
    if max_chunk_size == 0 {
        return Err("LIMIT max chunk size must be positive".to_owned());
    }
    let runtime = LimitValueRuntime {
        rows,
        cursor: 0,
        max_chunk_size,
        opened: false,
    };
    let mut executor = LimitExec {
        runtime,
        begin: offset as u64,
        end: offset.saturating_add(count) as u64,
        cursor: 0,
        meet_first_batch: false,
    };
    let mut context = ();
    executor.Open(&mut context)?;
    let result = (|| {
        let mut rows = Vec::new();
        loop {
            let mut chunk = LimitValueChunk {
                rows: Vec::with_capacity(max_chunk_size),
            };
            executor.Next(&mut context, &mut chunk)?;
            if chunk.rows.is_empty() {
                break;
            }
            rows.extend(chunk.rows);
        }
        Ok(rows)
    })();
    let close_result = executor.Close();
    match (result, close_result) {
        (Ok(rows), Ok(())) => Ok(rows),
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
    }
}

/// `DUAL` 表执行器：产出固定数量的空行（常用于无 FROM 的 SELECT）。
pub struct TableDualExec<R: SelectRuntime> {
    /// 运行时边界。
    pub runtime: R,
    /// 应产出的空行总数。
    pub num_dual_rows: usize,
    /// 已返回行数。
    pub num_returned: usize,
}

impl<R: SelectRuntime> TableDualExec<R> {
    /// 重置已返回计数。
    pub fn Open(&mut self, _context: &mut R::Context) -> Result<(), R::Error> {
        self.num_returned = 0;
        Ok(())
    }

    /// 追加全 NULL 行直至达到 `num_dual_rows`。
    pub fn Next(
        &mut self,
        _context: &mut R::Context,
        request: &mut R::Chunk,
    ) -> Result<(), R::Error> {
        self.runtime.reset_chunk(request);
        let remaining = self.num_dual_rows.saturating_sub(self.num_returned);
        let rows = remaining.min(self.runtime.chunk_capacity(request));
        for _ in 0..rows {
            self.runtime
                .append_null_row(request, self.runtime.schema_columns());
        }
        self.num_returned += rows;
        Ok(())
    }
}

/// Selection 执行器上下文包装。
pub struct selectionExecutorContext<C> {
    /// 会话句柄。
    pub session: C,
}

/// 构造 Selection 上下文。
pub fn newSelectionExecutorContext<C>(session: C) -> selectionExecutorContext<C> {
    selectionExecutorContext { session }
}

/// 过滤（WHERE/HAVING）执行器：支持批处理与逐行追加两种路径。
pub struct SelectionExec<R: SelectRuntime> {
    /// 运行时边界。
    pub runtime: R,
    /// 子执行器输入缓冲。
    pub input: Option<R::Chunk>,
    /// 当前输入批的选中位图。
    pub selected: Vec<bool>,
    /// 非批模式下输入行游标。
    pub input_row: usize,
}

impl<R: SelectRuntime> SelectionExec<R> {
    /// 打开过滤执行器。
    pub fn Open(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.open(context)
    }

    /// 分配输入 chunk 并打开子执行器。
    pub fn open(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.input_row = 0;
        self.selected.clear();
        self.input = Some(
            self.runtime
                .new_chunk(self.runtime.child_initial_capacity(0)),
        );
        self.runtime.open_child(context, 0)
    }

    /// 释放输入缓冲并关闭子执行器。
    pub fn Close(&mut self) -> Result<(), R::Error> {
        self.input = None;
        self.selected.clear();
        self.runtime.close_child(0)
    }

    /// 按配置走批处理或非批处理过滤路径。
    pub fn Next(
        &mut self,
        context: &mut R::Context,
        request: &mut R::Chunk,
    ) -> Result<(), R::Error> {
        // 批处理：整批过滤后一次写出。
        if self.runtime.selection_batched() {
            self.runtime.reset_chunk(request);
            let input = self.input.as_mut().expect("SelectionExec opened");
            self.runtime.reset_chunk(input);
            self.runtime.next_child(context, 0, input)?;
            if self.runtime.chunk_rows(input) == 0 {
                return Ok(());
            }
            self.selected = self.runtime.selection_vectorized_filter(context, input)?;
            self.runtime
                .selection_take_selected(request, input, &self.selected);
            return Ok(());
        }
        self.unBatchedNext(context, request)
    }

    /// 非批处理：循环填充输出 chunk，直到满或子源耗尽。
    pub fn unBatchedNext(
        &mut self,
        context: &mut R::Context,
        request: &mut R::Chunk,
    ) -> Result<(), R::Error> {
        self.runtime.reset_chunk(request);
        while self.runtime.chunk_rows(request) < self.runtime.chunk_capacity(request) {
            let input = self.input.as_mut().expect("SelectionExec opened");
            if self.input_row >= self.runtime.chunk_rows(input) {
                self.runtime.reset_chunk(input);
                self.runtime.next_child(context, 0, input)?;
                if self.runtime.chunk_rows(input) == 0 {
                    return Ok(());
                }
                self.selected = self.runtime.selection_vectorized_filter(context, input)?;
                self.input_row = 0;
            }
            self.runtime.selection_append_selected(
                request,
                input,
                &self.selected,
                &mut self.input_row,
            );
        }
        Ok(())
    }
}

/// 信息表/虚表扫描：一次性物化全部 chunk，再按批交换输出。
pub struct TableScanExec<R: SelectRuntime> {
    /// 运行时边界。
    pub runtime: R,
    /// 物化后的虚表 chunk 列表。
    pub virtual_table_chunks: Vec<R::Chunk>,
    /// 下一次输出的 chunk 下标。
    pub virtual_table_chunk_index: usize,
}

impl<R: SelectRuntime> TableScanExec<R> {
    /// 调整输出容量后从虚表缓存交换一批。
    pub fn Next(
        &mut self,
        context: &mut R::Context,
        request: &mut R::Chunk,
    ) -> Result<(), R::Error> {
        self.runtime
            .grow_and_reset_chunk(request, self.runtime.child_max_chunk_size(0));
        self.nextChunk4InfoSchema(context, request)
    }

    /// 懒加载全部信息表数据，再逐块 swap 到请求。
    pub fn nextChunk4InfoSchema(
        &mut self,
        context: &mut R::Context,
        request: &mut R::Chunk,
    ) -> Result<(), R::Error> {
        if self.virtual_table_chunks.is_empty() {
            self.virtual_table_chunks = self.runtime.table_scan_all(context)?;
        }
        if let Some(chunk) = self
            .virtual_table_chunks
            .get_mut(self.virtual_table_chunk_index)
        {
            self.runtime.swap_chunk(request, chunk);
            self.virtual_table_chunk_index += 1;
        }
        Ok(())
    }

    /// 清空虚表缓存，准备重新扫描。
    pub fn Open(&mut self, _context: &mut R::Context) -> Result<(), R::Error> {
        self.virtual_table_chunks.clear();
        self.virtual_table_chunk_index = 0;
        Ok(())
    }
}

/// 标量子查询封装：保证结果至多一行，否则报错；零行时补 NULL。
pub struct MaxOneRowExec<R: SelectRuntime> {
    /// 运行时边界。
    pub runtime: R,
    /// 是否已评估过（只产出一次）。
    pub evaluated: bool,
}

impl<R: SelectRuntime> MaxOneRowExec<R> {
    /// 打开子执行器并重置评估标志。
    pub fn Open(&mut self, context: &mut R::Context) -> Result<(), R::Error> {
        self.runtime.open_child(context, 0)?;
        self.evaluated = false;
        Ok(())
    }

    /// 取子结果：0 行补 NULL，1 行再探查是否还有行，多行报错。
    pub fn Next(
        &mut self,
        context: &mut R::Context,
        request: &mut R::Chunk,
    ) -> Result<(), R::Error> {
        self.runtime.reset_chunk(request);
        if self.evaluated {
            return Ok(());
        }
        self.evaluated = true;
        self.runtime.next_child(context, 0, request)?;
        match self.runtime.chunk_rows(request) {
            0 => {
                self.runtime
                    .append_null_row(request, self.runtime.schema_columns());
                Ok(())
            }
            1 => {
                let mut extra = self.runtime.new_chunk(1);
                self.runtime.next_child(context, 0, &mut extra)?;
                if self.runtime.chunk_rows(&extra) == 0 {
                    Ok(())
                } else {
                    Err(self.runtime.subquery_more_than_one_row_error())
                }
            }
            _ => Err(self.runtime.subquery_more_than_one_row_error()),
        }
    }
}

/// 按语句重置通用 StatementContext。
pub fn ResetContextOfStmt<R: SelectRuntime>(
    runtime: &mut R,
    statement: &R::Statement,
) -> Result<(), R::Error> {
    runtime.reset_statement_context(statement)
}

/// 重置 UPDATE 语句上下文。
pub fn ResetUpdateStmtCtx<R: SelectRuntime>(runtime: &mut R, statement: &R::UpdateStatement) {
    runtime.reset_update_statement_context(statement);
}

/// 重置 DELETE 语句上下文。
pub fn ResetDeleteStmtCtx<R: SelectRuntime>(runtime: &mut R, statement: &R::DeleteStatement) {
    runtime.reset_delete_statement_context(statement);
}

/// 为 TopSQL 注入快照相关选项。
pub fn setOptionForTopSQL<R: SelectRuntime>(runtime: &mut R, snapshot: &mut R::Snapshot) {
    runtime.set_top_sql_snapshot_options(snapshot);
}

/// 判断语句是否走弱一致读。
pub fn isWeakConsistencyRead<R: SelectRuntime>(runtime: &R, statement: &R::Statement) -> bool {
    runtime.weak_consistency_read(statement)
}

/// 锁键去重并保留首次出现顺序（悲观加锁前整理键集）。
pub fn deduplicateLockKeys<K: Clone + Eq + std::hash::Hash>(keys: &mut Vec<K>) {
    let mut seen = HashSet::with_capacity(keys.len());
    keys.retain(|key| seen.insert(key.clone()));
}

/// 物理表 ID 到列映射的别名。
pub type PhysicalTableColumnMap<C> = HashMap<i64, C>;
