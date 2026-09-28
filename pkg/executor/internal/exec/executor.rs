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

// 执行器（Executor）基础框架：Open/Next/Close 生命周期与 RU v2 计量。
//
// SQL 执行以火山模型（Volcano）拉取数据：父算子反复调用子算子的 `Next` 填充 `Chunk`
//（列式结果缓冲）。本模块提供元数据、会话变量、运行时统计、SQL Killer 中断，以及
// RUV2（Request Unit v2）按算子类型累计行/单元格开销的挂钩。

use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 执行器运行期错误：查询被杀、panic、会话错误或其他。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// 会话 `killed` 标志置位，对应 query interrupted。
    Killed,
    /// Open/Next/Close 中 catch_unwind 捕获到 panic。
    Panic,
    Session(String),
    Other(String),
}
impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Killed => f.write_str("query interrupted"),
            Self::Panic => f.write_str("executor panic"),
            Self::Session(v) | Self::Other(v) => f.write_str(v),
        }
    }
}
impl std::error::Error for Error {}
/// 本模块统一的 Result 别名。
pub type Result<T> = std::result::Result<T, Error>;

/// 列字段类型占位（完整类型系统迁移前仅保留 type_code）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FieldType {
    pub type_code: u8,
}
/// 结果集 schema：字段类型列表。
#[derive(Clone, Debug, Default)]
pub struct Schema {
    pub fields: Vec<FieldType>,
}

/// 列式结果块：行数、列数、初始容量与最大行数上限。
#[derive(Clone, Debug, Default)]
pub struct Chunk {
    rows: usize,
    columns: usize,
    pub capacity: usize,
    pub max_size: usize,
}
impl Chunk {
    /// 按字段数创建空 Chunk，行数初始为 0。
    pub fn new(fields: &[FieldType], capacity: usize, max_size: usize) -> Self {
        Self {
            rows: 0,
            columns: fields.len(),
            capacity,
            max_size,
        }
    }
    /// 清空行数，供下一次 Next 复用缓冲。
    pub fn Reset(&mut self) {
        self.rows = 0;
    }
    /// 设置行数，不超过 max_size。
    pub fn SetNumRows(&mut self, rows: usize) {
        self.rows = rows.min(self.max_size);
    }
    /// 当前结果行数。
    pub fn NumRows(&self) -> usize {
        self.rows
    }
    /// 列数（字段数）。
    pub fn NumCols(&self) -> usize {
        self.columns
    }
}

/// Next 路径上累计子算子输入行数与单元格数，供 RUV2 计量。
#[derive(Default)]
pub struct NextIOAcc {
    pub(crate) in_rows: i64,
    pub(crate) in_cells: i64,
}
impl NextIOAcc {
    pub fn reset(&mut self) {
        self.in_rows = 0;
        self.in_cells = 0;
    }
    /// 累加输入：零行直接返回；单元格数 = 行 × 列。
    pub fn addInput(&mut self, rows: usize, cols: usize) {
        if rows == 0 {
            return;
        }
        self.in_rows = self.in_rows.wrapping_add(rows as i64);
        self.in_cells = self.in_cells.wrapping_add(calcCellCount(rows, cols));
    }
}
/// 按 Go 的有符号整数语义计算 rows*cols，溢出时二进制回绕。
pub fn calcCellCount(rows: usize, cols: usize) -> i64 {
    (rows as i64).wrapping_mul(cols as i64)
}
/// 是否需要本地 NextIOAcc：有子节点且（跟踪 RUV2 或存在父级累加器）。
pub fn needNextIOAcc(trackRUV2: bool, has_parent: bool, child_count: usize) -> bool {
    child_count > 0 && (trackRUV2 || has_parent)
}

/// 某类执行器在 RUV2 中的计量元数据：标签、层级、是否按单元格计。
#[derive(Clone, Debug, Default)]
pub struct Ruv2ExecutorMetric {
    pub label: &'static str,
    pub level: u8,
    pub use_cells: bool,
}
/// 按 Go 类型名字符串映射到 RUV2 计量配置；未知类型返回 None。
pub fn ruv2ExecutorMetricByType(exec_type: &str) -> Option<Ruv2ExecutorMetric> {
    let (level, label, use_cells) = match exec_type {
        "*executor.BatchPointGetExec" => (1, "BatchPointGetExec", true),
        "*executor.PointGetExecutor" => (1, "PointGetExecutor", true),
        "*executor.LimitExec" => (1, "LimitExec", true),
        "*aggregate.HashAggExec" => (2, "HashAggExec", false),
        "*executor.ExpandExec" => (2, "ExpandExec", false),
        "*executor.IndexLookUpExecutor" => (2, "IndexLookUpExecutor", false),
        "*executor.IndexReaderExecutor" => (2, "IndexReaderExecutor", false),
        "*executor.MemTableReaderExec" => (2, "MemTableReaderExec", false),
        "*executor.ProjectionExec" => (2, "ProjectionExec", true),
        "*executor.SelectionExec" => (2, "SelectionExec", false),
        "*executor.SelectLockExec" => (2, "SelectLockExec", true),
        "*executor.TableDualExec" => (2, "TableDualExec", false),
        "*executor.TableReaderExecutor" => (2, "TableReaderExecutor", false),
        "*executor.UnionScanExec" => (2, "UnionScanExec", false),
        "*windows.WindowExec" | "*windows.PipelinedWindowExec" | "*windows.OrderedWindowExec" => {
            (2, "WindowExec", false)
        }
        "*join.HashJoinV1Exec" => (2, "HashJoinV1Exec", false),
        "*join.HashJoinV2Exec" => (2, "HashJoinV2Exec", false),
        "*join.IndexLookUpJoin" => (2, "IndexLookUpJoin", true),
        "*join.IndexLookUpMergeJoin" => (2, "IndexLookUpMergeJoin", true),
        "*join.IndexNestedLoopHashJoin" => (2, "IndexNestedLoopHashJoin", true),
        "*join.MergeJoinExec" => (2, "MergeJoinExec", false),
        "*sortexec.TopNExec" => (2, "TopNExec", true),
        "*aggregate.StreamAggExec" => (3, "StreamAggExec", false),
        "*sortexec.SortExec" => (3, "SortExec", true),
        _ => return None,
    };
    Some(Ruv2ExecutorMetric {
        label,
        level,
        use_cells,
    })
}

/// 会话级 RUV2 指标汇聚：可旁路，并按 (level, label) 累加。
#[derive(Default)]
pub struct RUV2Metrics {
    bypass: AtomicBool,
    values: Mutex<HashMap<(u8, String), i64>>,
}
impl RUV2Metrics {
    /// 是否跳过计量（旁路）。
    pub fn Bypass(&self) -> bool {
        self.bypass.load(Ordering::Acquire)
    }
    /// 设置是否旁路 RUV2 计量。
    pub fn SetBypass(&self, bypass: bool) {
        self.bypass.store(bypass, Ordering::Release);
    }
    /// 累加指定层级/标签的计量增量。
    pub fn AddExecutorMetric(&self, level: u8, label: &str, delta: i64) {
        if let Ok(mut values) = self.values.lock() {
            let value = values.entry((level, label.into())).or_default();
            *value = value.wrapping_add(delta);
        }
    }
    /// 读取指定层级/标签的累计计量值。
    pub fn value(&self, level: u8, label: &str) -> i64 {
        self.values
            .lock()
            .ok()
            .and_then(|v| v.get(&(level, label.into())).copied())
            .unwrap_or_default()
    }
}

/// 单次 Open/Next 调用上下文：可选 RUV2 指标与父级输入累加器。
#[derive(Clone, Default)]
pub struct ExecContext {
    pub metrics: Option<Arc<RUV2Metrics>>,
    input_acc: Option<Arc<Mutex<NextIOAcc>>>,
}
impl ExecContext {
    fn withInputAcc(&self, acc: Arc<Mutex<NextIOAcc>>) -> Self {
        let mut next = self.clone();
        next.input_acc = Some(acc);
        next
    }
}

/// 缓存在 BaseExecutor 上的 RUV2 Next 状态，避免每次查表。
#[derive(Clone, Default)]
pub struct Ruv2NextCacheState {
    pub metrics: Option<Arc<RUV2Metrics>>,
    pub region_name: String,
    pub info: Option<Ruv2ExecutorMetric>,
}
/// 根据执行器类型填充 region 名、计量元数据与可用 metrics 句柄。
fn populateRUV2NextCache(ctx: &ExecContext, cache: &mut Ruv2NextCacheState, exec_type: &str) {
    cache.region_name = format!("{exec_type}.Next");
    cache.info = ruv2ExecutorMetricByType(exec_type);
    cache.metrics = cache
        .info
        .as_ref()
        .and_then(|_| ctx.metrics.clone())
        .filter(|metrics| !metrics.Bypass());
}
/// 按缓存的计量元数据把本轮 in/out 行或单元格增量写入 RUV2。
pub fn addRUV2ExecutorMetricCached(
    metrics: Option<&RUV2Metrics>,
    info: &Ruv2ExecutorMetric,
    in_rows: i64,
    out_rows: i64,
    in_cells: i64,
    out_cells: i64,
) {
    let Some(metrics) = metrics else { return };
    let delta = if info.use_cells {
        in_cells.wrapping_add(out_cells)
    } else {
        in_rows.wrapping_add(out_rows)
    };
    if delta != 0 {
        metrics.AddExecutorMetric(info.level, info.label, delta);
    }
}

/// 算子基础运行时统计：Open/Next/Close 耗时与产出行数。
#[derive(Default)]
pub struct BasicRuntimeStats {
    pub open: Duration,
    pub next: Duration,
    pub close: Duration,
    pub rows: u64,
}
impl BasicRuntimeStats {
    fn RecordOpen(&mut self, duration: Duration) {
        self.open += duration;
    }
    fn Record(&mut self, duration: Duration, rows: usize) {
        self.next += duration;
        self.rows = self.rows.saturating_add(rows as u64);
    }
    fn RecordClose(&mut self, duration: Duration) {
        self.close += duration;
    }
}

/// 执行器接口：火山模型生命周期与 chunk/子树/统计挂钩。
pub trait Executor: Send {
    fn executorType(&self) -> &'static str;
    fn Open(&mut self, ctx: &ExecContext) -> Result<()>;
    fn Next(&mut self, ctx: &ExecContext, req: &mut Chunk) -> Result<()>;
    fn Close(&mut self) -> Result<()>;
    fn Schema(&self) -> Schema;
    fn RetFieldTypes(&self) -> Vec<FieldType>;
    fn InitCap(&self) -> usize;
    fn MaxChunkSize(&self) -> usize;
    fn AllChildren(&self) -> &[Box<dyn Executor>];
    fn SetAllChildren(&mut self, children: Vec<Box<dyn Executor>>);
    fn RuntimeStats(&self) -> Option<Arc<Mutex<BasicRuntimeStats>>> {
        None
    }
    fn HandleSQLKillerSignal(&self) -> Result<()> {
        Ok(())
    }
    fn RegisterSQLAndPlanInExecForTopProfiling(&self) {}
    fn ruv2NextCache(&mut self) -> Option<&mut Ruv2NextCacheState> {
        None
    }
    fn reusableNextIOAcc(&mut self) -> Option<Arc<Mutex<NextIOAcc>>> {
        None
    }
    fn Detach(&mut self) -> (Option<Box<dyn Executor>>, bool) {
        (None, false)
    }
    fn NewChunk(&self) -> Chunk {
        Chunk::new(&self.RetFieldTypes(), self.InitCap(), self.MaxChunkSize())
    }
    fn NewChunkWithCapacity(
        &self,
        fields: &[FieldType],
        capacity: usize,
        max_size: usize,
    ) -> Chunk {
        Chunk::new(fields, capacity, max_size)
    }
}

/// 执行器元数据：schema、子树、返回列类型与 plan id。
pub struct ExecutorMeta {
    schema: Option<Schema>,
    children: Vec<Box<dyn Executor>>,
    ret_field_types: Vec<FieldType>,
    pub id: i32,
}
impl ExecutorMeta {
    /// 由 schema 推导返回列类型并持有子执行器。
    pub fn new(schema: Option<Schema>, id: i32, children: Vec<Box<dyn Executor>>) -> Self {
        let ret_field_types = schema
            .as_ref()
            .map(|s| s.fields.clone())
            .unwrap_or_default();
        Self {
            schema,
            children,
            ret_field_types,
            id,
        }
    }
    pub fn Schema(&self) -> Schema {
        self.schema.clone().unwrap_or_default()
    }
    pub fn GetSchema(&self) -> Option<&Schema> {
        self.schema.as_ref()
    }
    pub fn RetFieldTypes(&self) -> Vec<FieldType> {
        self.ret_field_types.clone()
    }
    pub fn ChildrenLen(&self) -> usize {
        self.children.len()
    }
    pub fn EmptyChildren(&self) -> bool {
        self.children.is_empty()
    }
    pub fn SetChildren(&mut self, index: usize, child: Box<dyn Executor>) {
        self.children[index] = child;
    }
    pub fn Children(&self, index: usize) -> &dyn Executor {
        self.children[index].as_ref()
    }
}

/// 按会话 chunk 大小配置分配结果块。
#[derive(Clone)]
pub struct ExecutorChunkAllocator {
    ret_field_types: Vec<FieldType>,
    init_cap: usize,
    max_chunk_size: usize,
}
impl ExecutorChunkAllocator {
    pub fn new(vars: &SessionVars, fields: Vec<FieldType>) -> Self {
        Self {
            ret_field_types: fields,
            init_cap: vars.init_chunk_size,
            max_chunk_size: vars.max_chunk_size,
        }
    }
    pub fn InitCap(&self) -> usize {
        self.init_cap
    }
    pub fn SetInitCap(&mut self, capacity: usize) {
        self.init_cap = capacity;
    }
    pub fn MaxChunkSize(&self) -> usize {
        self.max_chunk_size
    }
    pub fn SetMaxChunkSize(&mut self, size: usize) {
        self.max_chunk_size = size;
    }
    pub fn NewChunk(&self) -> Chunk {
        Chunk::new(&self.ret_field_types, self.init_cap, self.max_chunk_size)
    }
}

/// 语句上下文：规范化 SQL/计划、TopSQL 与按 plan id 的运行时统计。
#[derive(Default)]
pub struct StatementContext {
    pub normalized_sql: String,
    pub sql_digest: String,
    pub normalized_plan: String,
    pub plan_digest: String,
    pub in_restricted_sql: bool,
    pub top_sql_enabled: bool,
    registered: AtomicBool,
    pub runtime_stats: Mutex<HashMap<i32, Arc<Mutex<BasicRuntimeStats>>>>,
}
/// 会话变量中与执行器相关的子集：chunk 大小、语句上下文、killed 标志。
#[derive(Clone)]
pub struct SessionVars {
    pub init_chunk_size: usize,
    pub max_chunk_size: usize,
    pub stmt_ctx: Arc<StatementContext>,
    pub killed: Arc<AtomicBool>,
}
impl Default for SessionVars {
    fn default() -> Self {
        Self {
            init_chunk_size: 32,
            max_chunk_size: 1024,
            stmt_ctx: Arc::new(StatementContext::default()),
            killed: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// 绑定到具体 plan id 的运行时统计句柄，并支持 TopSQL 注册。
#[derive(Clone)]
pub struct ExecutorStats {
    runtime_stats: Option<Arc<Mutex<BasicRuntimeStats>>>,
    stmt_ctx: Arc<StatementContext>,
}
impl ExecutorStats {
    fn new(stmt_ctx: Arc<StatementContext>, id: i32) -> Self {
        let runtime_stats = if id > 0 {
            stmt_ctx
                .runtime_stats
                .lock()
                .ok()
                .map(|mut all| all.entry(id).or_default().clone())
        } else {
            None
        };
        Self {
            runtime_stats,
            stmt_ctx,
        }
    }
    fn RegisterSQLAndPlanInExecForTopProfiling(&self) {
        if self.stmt_ctx.top_sql_enabled {
            let _ = self.stmt_ctx.registered.compare_exchange(
                false,
                true,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }
}

/// 无会话依赖的基础执行器实现：默认 Open 子树、空 Next、Close 聚合错误。
pub struct BaseExecutorV2 {
    pub meta: ExecutorMeta,
    allocator: ExecutorChunkAllocator,
    stats: ExecutorStats,
    killed: Arc<AtomicBool>,
    ruv2_cache: Ruv2NextCacheState,
    next_io_acc: Arc<Mutex<NextIOAcc>>,
}
impl BaseExecutorV2 {
    /// 从会话变量构造基础执行器。
    pub fn NewBaseExecutorV2(
        vars: &SessionVars,
        schema: Option<Schema>,
        id: i32,
        children: Vec<Box<dyn Executor>>,
    ) -> Self {
        let meta = ExecutorMeta::new(schema, id, children);
        Self {
            allocator: ExecutorChunkAllocator::new(vars, meta.RetFieldTypes()),
            stats: ExecutorStats::new(vars.stmt_ctx.clone(), id),
            killed: vars.killed.clone(),
            meta,
            ruv2_cache: Ruv2NextCacheState::default(),
            next_io_acc: Arc::new(Mutex::new(NextIOAcc::default())),
        }
    }
    /// 基于当前执行器的会话侧状态再构造一个同族实例。
    pub fn BuildNewBaseExecutorV2(
        &self,
        schema: Option<Schema>,
        id: i32,
        children: Vec<Box<dyn Executor>>,
    ) -> Self {
        let meta = ExecutorMeta::new(schema, id, children);
        let mut allocator = self.allocator.clone();
        allocator.ret_field_types = meta.RetFieldTypes();
        Self {
            meta,
            allocator,
            stats: ExecutorStats::new(self.stats.stmt_ctx.clone(), id),
            killed: self.killed.clone(),
            ruv2_cache: Ruv2NextCacheState::default(),
            next_io_acc: Arc::new(Mutex::new(NextIOAcc::default())),
        }
    }
}
impl Executor for BaseExecutorV2 {
    fn executorType(&self) -> &'static str {
        "*exec.BaseExecutorV2"
    }
    fn Open(&mut self, ctx: &ExecContext) -> Result<()> {
        for child in &mut self.meta.children {
            Open(ctx, child.as_mut())?;
        }
        Ok(())
    }
    fn Next(&mut self, _ctx: &ExecContext, _req: &mut Chunk) -> Result<()> {
        Ok(())
    }
    fn Close(&mut self) -> Result<()> {
        let mut first = None;
        for child in &mut self.meta.children {
            if let Err(error) = Close(child.as_mut()) {
                if first.is_none() {
                    first = Some(error);
                }
            }
        }
        first.map_or(Ok(()), Err)
    }
    fn Schema(&self) -> Schema {
        self.meta.Schema()
    }
    fn RetFieldTypes(&self) -> Vec<FieldType> {
        self.meta.RetFieldTypes()
    }
    fn InitCap(&self) -> usize {
        self.allocator.InitCap()
    }
    fn MaxChunkSize(&self) -> usize {
        self.allocator.MaxChunkSize()
    }
    fn AllChildren(&self) -> &[Box<dyn Executor>] {
        &self.meta.children
    }
    fn SetAllChildren(&mut self, children: Vec<Box<dyn Executor>>) {
        self.meta.children = children;
    }
    fn RuntimeStats(&self) -> Option<Arc<Mutex<BasicRuntimeStats>>> {
        self.stats.runtime_stats.clone()
    }
    fn HandleSQLKillerSignal(&self) -> Result<()> {
        if self.killed.load(Ordering::Acquire) {
            Err(Error::Killed)
        } else {
            Ok(())
        }
    }
    fn RegisterSQLAndPlanInExecForTopProfiling(&self) {
        self.stats.RegisterSQLAndPlanInExecForTopProfiling();
    }
    fn ruv2NextCache(&mut self) -> Option<&mut Ruv2NextCacheState> {
        Some(&mut self.ruv2_cache)
    }
    fn reusableNextIOAcc(&mut self) -> Option<Arc<Mutex<NextIOAcc>>> {
        if let Ok(mut acc) = self.next_io_acc.lock() {
            acc.reset();
        }
        Some(self.next_io_acc.clone())
    }
}

/// 系统会话池：借用/归还内部会话。
pub trait SessionPool: Send + Sync {
    fn Get(&self) -> Result<Box<dyn SystemSession>>;
    fn Put(&self, session: Box<dyn SystemSession>);
}
/// 内部系统会话：可标记 restricted SQL、回滚与关闭。
pub trait SystemSession: Send {
    fn SetRestrictedSql(&mut self, restricted: bool);
    fn Rollback(&mut self) -> Result<()>;
    fn Close(&mut self);
}
/// 执行器可见的会话上下文：变量、表增量更新、系统会话池。
pub trait SessionContext: Send + Sync {
    fn Vars(&self) -> SessionVars;
    fn UpdateDeltaForTable(&self, table_id: i64);
    fn SystemPool(&self) -> Arc<dyn SessionPool>;
}

/// 带会话上下文的基础执行器，封装系统会话借用等辅助方法。
pub struct BaseExecutor {
    pub ctx: Arc<dyn SessionContext>,
    pub base: BaseExecutorV2,
}
impl BaseExecutor {
    /// 绑定会话上下文并构造内部 `BaseExecutorV2`。
    pub fn NewBaseExecutor(
        ctx: Arc<dyn SessionContext>,
        schema: Option<Schema>,
        id: i32,
        children: Vec<Box<dyn Executor>>,
    ) -> Self {
        let vars = ctx.Vars();
        Self {
            ctx,
            base: BaseExecutorV2::NewBaseExecutorV2(&vars, schema, id, children),
        }
    }
    /// 返回会话上下文句柄。
    pub fn Ctx(&self) -> Arc<dyn SessionContext> {
        self.ctx.clone()
    }
    /// 通知会话更新指定表的增量统计。
    pub fn UpdateDeltaForTableID(&self, id: i64) {
        self.ctx.UpdateDeltaForTable(id);
    }
    /// 从池中取系统会话并标记为 restricted SQL。
    pub fn GetSysSession(&self) -> Result<Box<dyn SystemSession>> {
        let mut session = self.ctx.SystemPool().Get()?;
        session.SetRestrictedSql(true);
        Ok(session)
    }
    /// 归还系统会话：回滚失败则关闭，成功则放回池。
    pub fn ReleaseSysSession(&self, mut session: Box<dyn SystemSession>) {
        if session.Rollback().is_err() {
            session.Close();
        } else {
            self.ctx.SystemPool().Put(session);
        }
    }
}

/// 按执行器返回类型分配可缓存 Chunk。
pub fn TryNewCacheChunk(executor: &dyn Executor) -> Chunk {
    executor.NewChunk()
}
/// 返回执行器结果列类型。
pub fn RetTypes(executor: &dyn Executor) -> Vec<FieldType> {
    executor.RetFieldTypes()
}
/// 按 InitCap/MaxChunkSize 分配首个结果 Chunk。
pub fn NewFirstChunk(executor: &dyn Executor) -> Chunk {
    Chunk::new(
        &executor.RetFieldTypes(),
        executor.InitCap(),
        executor.MaxChunkSize(),
    )
}

/// 包装 Open：填充 RUV2 缓存、捕获 panic，并记录 Open 耗时。
pub fn Open(ctx: &ExecContext, executor: &mut dyn Executor) -> Result<()> {
    let started = Instant::now();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let exec_type = executor.executorType();
        if let Some(cache) = executor.ruv2NextCache() {
            populateRUV2NextCache(ctx, cache, exec_type);
        }
        executor.Open(ctx)
    }))
    .unwrap_or(Err(Error::Panic));
    if let Some(stats) = executor.RuntimeStats() {
        if let Ok(mut stats) = stats.lock() {
            stats.RecordOpen(started.elapsed());
        }
    }
    result
}

/// 包装 Next：处理 SQL Killer、RUV2 输入/输出计量、父累加器与运行时统计。
pub fn Next(ctx: &ExecContext, executor: &mut dyn Executor, req: &mut Chunk) -> Result<()> {
    let started = Instant::now();
    let result = catch_unwind(AssertUnwindSafe(|| {
        executor.HandleSQLKillerSignal()?;
        let exec_type = executor.executorType();
        // 优先使用缓存的计量元数据；region 名为空时再补填。
        let (info, metrics, region_empty) = if let Some(cache) = executor.ruv2NextCache() {
            (
                cache.info.clone(),
                cache.metrics.clone(),
                cache.region_name.is_empty(),
            )
        } else {
            (
                ruv2ExecutorMetricByType(exec_type),
                ctx.metrics.clone().filter(|m| !m.Bypass()),
                false,
            )
        };
        if region_empty {
            if let Some(cache) = executor.ruv2NextCache() {
                populateRUV2NextCache(ctx, cache, exec_type);
            }
        }
        let track = info.is_some() && metrics.is_some();
        let parent_acc = ctx.input_acc.clone();
        let need_local = needNextIOAcc(track, parent_acc.is_some(), executor.AllChildren().len());
        // 需要本地累加器时复用 BaseExecutor 上的实例，避免每轮分配。
        let local_acc = need_local.then(|| {
            executor
                .reusableNextIOAcc()
                .unwrap_or_else(|| Arc::new(Mutex::new(NextIOAcc::default())))
        });
        let child_ctx = local_acc
            .clone()
            .map_or_else(|| ctx.clone(), |acc| ctx.withInputAcc(acc));
        executor.RegisterSQLAndPlanInExecForTopProfiling();
        executor.Next(&child_ctx, req)?;
        // 将本算子产出回写到父级输入累加器。
        if let Some(parent) = parent_acc {
            if let Ok(mut parent) = parent.lock() {
                parent.addInput(req.NumRows(), req.NumCols());
            }
        }
        if track {
            let (in_rows, in_cells) = local_acc
                .and_then(|acc| acc.lock().ok().map(|acc| (acc.in_rows, acc.in_cells)))
                .unwrap_or_default();
            if let Some(info) = info.as_ref() {
                addRUV2ExecutorMetricCached(
                    metrics.as_deref(),
                    info,
                    in_rows,
                    req.NumRows() as i64,
                    in_cells,
                    calcCellCount(req.NumRows(), req.NumCols()),
                );
            }
        }
        executor.HandleSQLKillerSignal()
    }))
    .unwrap_or(Err(Error::Panic));
    if let Some(stats) = executor.RuntimeStats() {
        if let Ok(mut stats) = stats.lock() {
            stats.Record(started.elapsed(), req.NumRows());
        }
    }
    result
}

/// 包装 Close：捕获 panic 并记录关闭耗时。
pub fn Close(executor: &mut dyn Executor) -> Result<()> {
    let started = Instant::now();
    let result = catch_unwind(AssertUnwindSafe(|| executor.Close())).unwrap_or(Err(Error::Panic));
    if let Some(stats) = executor.RuntimeStats() {
        if let Ok(mut stats) = stats.lock() {
            stats.RecordClose(started.elapsed());
        }
    }
    result
}
