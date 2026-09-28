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

// 受限 SQL 执行边界：系统表查询、执行选项、RecordSet 与 Drain 辅助。
//
// 对应 Go `pkg/util/sqlexec` 中的 `RestrictedSQLExecutor`、`SQLExecutor`、
// `RecordSet` 等接口。SnapshotTS 为快照时间戳（MVCC 读版本），Analyze 相关
// 选项控制统计信息收集行为。

use std::any::Any;
use std::fmt;

#[cfg(feature = "formal-crate")]
use crate::{ast, chunk, context, logutil, parser, resolve, sysproctrack, terror, variable};

/// Rust error boundary corresponding to Go's built-in `error` interface.
/// 对应 Go 内置 `error` 接口的跨边界错误类型。
pub type GoError = Box<dyn std::error::Error + Send + Sync>;

/// Executes SQL against system tables with the restrictions imposed by a session.
/// 在会话限制下执行系统表 SQL 的受限执行器接口。
pub trait RestrictedSQLExecutor {
    /// 带参数解析 SQL，返回 AST 节点。
    fn ParseWithParams(
        &mut self,
        ctx: &context::Context,
        sql: &str,
        args: Vec<Box<dyn Any>>,
    ) -> Result<ast::NodeRef, GoError>;

    /// 执行已解析语句，返回行与结果字段。
    fn ExecRestrictedStmt(
        &mut self,
        ctx: &context::Context,
        stmt: ast::NodeRef,
        opts: Vec<OptionFuncAlias>,
    ) -> Result<(Vec<chunk::Row>, Vec<resolve::ResultField>), GoError>;

    /// 解析并执行受限 SQL（可带参数与 ExecOption）。
    fn ExecRestrictedSQL(
        &mut self,
        ctx: &context::Context,
        opts: Vec<OptionFuncAlias>,
        sql: &str,
        args: Vec<Box<dyn Any>>,
    ) -> Result<(Vec<chunk::Row>, Vec<resolve::ResultField>), GoError>;
}

/// 注册系统过程跟踪的回调类型。
pub type TrackSysProcFn =
    Box<dyn Fn(u64, sysproctrack::TrackProcRef) -> Result<(), GoError> + Send + Sync>;
/// 取消系统过程跟踪的回调类型。
pub type UnTrackSysProcFn = Box<dyn Fn(u64) + Send + Sync>;

/// Options applied by `ExecRestrictedStmt` and `ExecRestrictedSQL`.
/// `ExecRestrictedStmt` / `ExecRestrictedSQL` 使用的执行选项集合。
#[derive(Default)]
pub struct ExecOption {
    /// 是否使用 analyze 快照读。
    pub AnalyzeSnapshot: Option<bool>,
    /// 系统过程开始跟踪回调。
    pub TrackSysProc: Option<TrackSysProcFn>,
    /// 系统过程结束跟踪回调。
    pub UnTrackSysProc: Option<UnTrackSysProcFn>,
    /// 分区裁剪模式（Partition Prune Mode）字符串。
    pub PartitionPruneMode: String,
    /// 快照时间戳 SnapshotTS（MVCC 读版本）。
    pub SnapshotTS: u64,
    /// Analyze 统计信息版本号。
    pub AnalyzeVer: i32,
    /// 被跟踪的系统过程 ID。
    pub TrackSysProcID: u64,
    /// 是否忽略警告。
    pub IgnoreWarning: bool,
    /// true 使用当前会话；false 从会话池取会话。
    pub UseCurSession: bool,
    /// 是否在 DDL 路径启用 analyze。
    pub EnableDDLAnalyze: bool,
}

/// 一次性修改 `ExecOption` 的函数别名（对应 Go 的 OptionFunc）。
pub type OptionFuncAlias = Box<dyn FnOnce(&mut ExecOption)>;

/// 设置忽略警告。
pub fn ExecOptionIgnoreWarning(option: &mut ExecOption) {
    option.IgnoreWarning = true;
}

/// 启用 DDL analyze。
pub fn ExecOptionEnableDDLAnalyze(option: &mut ExecOption) {
    option.EnableDDLAnalyze = true;
}

/// 将 AnalyzeVer 设为 2。
pub fn ExecOptionAnalyzeVer2(option: &mut ExecOption) {
    option.AnalyzeVer = 2;
}

/// 构造指定分区裁剪模式的选项闭包。
pub fn GetPartitionPruneModeOption(pruneMode: String) -> OptionFuncAlias {
    Box::new(move |option| option.PartitionPruneMode = pruneMode)
}

/// 构造是否使用 analyze 快照的选项闭包。
pub fn GetAnalyzeSnapshotOption(analyzeSnapshot: bool) -> OptionFuncAlias {
    Box::new(move |option| option.AnalyzeSnapshot = Some(analyzeSnapshot))
}

/// 强制使用当前会话执行。
pub fn ExecOptionUseCurSession(option: &mut ExecOption) {
    option.UseCurSession = true;
}

/// 强制从会话池取会话执行。
pub fn ExecOptionUseSessionPool(option: &mut ExecOption) {
    option.UseCurSession = false;
}

/// 构造带快照时间戳的选项闭包。
pub fn ExecOptionWithSnapshot(snapshot: u64) -> OptionFuncAlias {
    Box::new(move |option| option.SnapshotTS = snapshot)
}

/// 构造系统过程跟踪相关选项闭包。
pub fn ExecOptionWithSysProcTrack(
    procID: u64,
    track: TrackSysProcFn,
    untrack: UnTrackSysProcFn,
) -> OptionFuncAlias {
    Box::new(move |option| {
        option.TrackSysProcID = procID;
        option.TrackSysProc = Some(track);
        option.UnTrackSysProc = Some(untrack);
    })
}

/// 按顺序应用选项闭包，得到最终 `ExecOption`。
pub fn GetExecOption(opts: Vec<OptionFuncAlias>) -> ExecOption {
    let mut option = ExecOption::default();
    for apply in opts {
        apply(&mut option);
    }
    option
}

/// SQL execution boundary used to break package dependency cycles.
/// 通用 SQL 执行边界，用于打断包依赖环。
pub trait SQLExecutor {
    /// 执行 SQL，可能返回多个结果集。
    fn Execute(
        &mut self,
        ctx: &context::Context,
        sql: &str,
    ) -> Result<Vec<Box<dyn RecordSet>>, GoError>;

    /// 内部执行（可带参数），至多一个结果集。
    fn ExecuteInternal(
        &mut self,
        ctx: &context::Context,
        sql: &str,
        args: Vec<Box<dyn Any>>,
    ) -> Result<Option<Box<dyn RecordSet>>, GoError>;

    /// 执行已解析的语句节点。
    fn ExecuteStmt(
        &mut self,
        ctx: &context::Context,
        stmtNode: ast::NodeRef,
    ) -> Result<Option<Box<dyn RecordSet>>, GoError>;
}

/// SQL 解析器边界接口。
pub trait SQLParser {
    /// 解析 SQL，返回语句列表与警告/错误列表。
    fn ParseSQL(
        &mut self,
        ctx: &context::Context,
        sql: &str,
        params: &[&dyn parser::ParseParam],
    ) -> Result<(Vec<ast::NodeRef>, Vec<GoError>), GoError>;
}

/// SQL statement implementations must be safe for concurrent use.
/// 已编译/已准备的语句抽象；实现须可并发安全使用。
pub trait Statement: Send + Sync {
    /// 原始 SQL 文本。
    fn OriginText(&self) -> String;
    /// 当前文本。
    fn Text(&self) -> String;
    /// 用于日志的文本；`keepHint` 控制是否保留 hint。
    fn GetTextToLog(&self, keepHint: bool) -> String;
    /// 执行语句。
    fn Exec(&self, ctx: &context::Context) -> Result<Option<Box<dyn RecordSet>>, GoError>;
    /// 是否为 Prepared Statement。
    fn IsPrepared(&self) -> bool;
    /// 在给定会话变量下是否只读。
    fn IsReadOnly(&self, vars: &variable::SessionVars) -> bool;
    /// 重建执行计划（物理算子树），返回计划相关标识。
    fn RebuildPlan(&self, ctx: &context::Context) -> Result<i64, GoError>;
    /// 取得底层 AST 节点。
    fn GetStmtNode(&self) -> ast::NodeRef;
}

/// A chunk may be owned directly or supplied by the package's recycling allocator.
/// The enum preserves Go's single `*chunk.Chunk` abstraction across the two Rust
/// ownership forms exposed by the migrated chunk crate.
/// Chunk 可能直接拥有，或由回收分配器提供；枚举统一两种所有权形态。
pub enum RecordChunk {
    /// 直接拥有的 Chunk。
    Owned(Box<chunk::Chunk>),
    /// 分配器租借的 Chunk 引用。
    Allocated(chunk::ChunkRef),
}

impl RecordChunk {
    /// 由装箱 Chunk 构造 Owned 变体。
    pub fn from_boxed(chunk: Box<chunk::Chunk>) -> Self {
        Self::Owned(chunk)
    }

    /// 由分配器引用构造 Allocated 变体。
    pub fn from_allocated(chunk: chunk::ChunkRef) -> Self {
        Self::Allocated(chunk)
    }

    /// 是否为分配器路径。
    pub fn is_allocated(&self) -> bool {
        matches!(self, Self::Allocated(_))
    }

    /// 只读访问底层 Chunk。
    pub fn with_chunk<R>(&self, inspect: impl FnOnce(&chunk::Chunk) -> R) -> R {
        match self {
            Self::Owned(chunk) => inspect(chunk),
            Self::Allocated(chunk) => {
                let guard = chunk.lock().expect("chunk allocator mutex poisoned");
                inspect(&guard)
            }
        }
    }

    /// 可变访问底层 Chunk。
    pub fn with_chunk_mut<R>(&mut self, mutate: impl FnOnce(&mut chunk::Chunk) -> R) -> R {
        match self {
            Self::Owned(chunk) => mutate(chunk),
            Self::Allocated(chunk) => {
                let mut guard = chunk.lock().expect("chunk allocator mutex poisoned");
                mutate(&mut guard)
            }
        }
    }

    /// 返回当前行数。
    pub fn NumRows(&self) -> usize {
        self.with_chunk(chunk::Chunk::NumRows)
    }

    /// 拷贝出全部行（用于 Drain）。
    fn copy_rows(&self) -> Vec<chunk::Row> {
        self.with_chunk(|chunk| {
            (0..chunk.NumRows())
                .map(|index| chunk.GetRow(index).CopyConstruct())
                .collect()
        })
    }

    /// 按新容量 Renew，始终得到 Owned chunk。
    fn renew(&self, maxChunkSize: usize) -> Self {
        Self::Owned(self.with_chunk(|chunk| chunk::Renew(chunk, maxChunkSize)))
    }
}

/// 结果集接口：字段元信息、按 chunk 迭代、关闭与可选钩子。
pub trait RecordSet {
    /// 结果列字段。
    fn Fields(&self) -> &[resolve::ResultField];
    /// 填充下一 chunk；无更多行时 `req` 行数为 0。
    fn Next(&mut self, ctx: &context::Context, req: &mut RecordChunk) -> Result<(), GoError>;
    /// 分配用于 `Next` 的新 chunk。
    fn NewChunk(&self, allocator: Option<&mut dyn chunk::Allocator>) -> RecordChunk;
    /// 关闭结果集并释放资源。
    fn Close(&mut self) -> Result<(), GoError>;

    /// Optional statement-finalization hook. Record sets that do not own an
    /// executor have the same behavior as a Go value that does not implement
    /// `interface { Finish() error }`.
    /// 可选的语句收尾钩子；默认成功空操作。
    fn Finish(&mut self) -> Result<(), GoError> {
        Ok(())
    }

    /// Optional detach hook. The default is the exact result of a failed Go
    /// `sqlexec.DetachableRecordSet` assertion.
    /// 可选分离钩子；默认等价于 Go 类型断言失败。
    fn TryDetach(&mut self) -> Result<(Option<Box<dyn RecordSet>>, bool), GoError> {
        Ok((None, false))
    }

    /// Optional COM_FETCH notification hook.
    /// MySQL 协议 COM_FETCH 返回后的可选通知钩子。
    fn OnFetchReturned(&mut self) {}
}

/// 可从游标/会话分离的结果集。
pub trait DetachableRecordSet: RecordSet {
    fn TryDetach(&mut self) -> Result<(Option<Box<dyn RecordSet>>, bool), GoError>;
}

/// 多语句无延迟结果（对应 multi-query no-delay 协议字段）。
pub trait MultiQueryNoDelayResult {
    fn AffectedRows(&self) -> u64;
    fn LastMessage(&self) -> String;
    fn WarnCount(&self) -> u16;
    fn Status(&self) -> u16;
    fn LastInsertID(&self) -> u64;
}

/// Error returned by `DrainRecordSet`; collected rows remain available exactly as
/// in Go's `return rows, err` path.
/// `DrainRecordSet` 错误：已收集行与源错误一并保留，对齐 Go `return rows, err`。
pub struct DrainError {
    /// 失败前已读出的行。
    pub rows: Vec<chunk::Row>,
    /// 原始错误源。
    pub source: GoError,
}

impl fmt::Debug for DrainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DrainError")
            .field("rows", &self.rows)
            .field("source", &self.source.to_string())
            .finish()
    }
}

impl fmt::Display for DrainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.source.fmt(formatter)
    }
}

impl std::error::Error for DrainError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// 持续调用 `Next` 直到空 chunk，汇聚全部行；中途失败则带回已读行。
pub fn DrainRecordSet(
    ctx: &context::Context,
    rs: &mut dyn RecordSet,
    maxChunkSize: usize,
) -> Result<Vec<chunk::Row>, DrainError> {
    let mut rows = Vec::new();
    let mut req = rs.NewChunk(None);
    loop {
        if let Err(source) = rs.Next(ctx, &mut req) {
            return Err(DrainError { rows, source });
        }
        // 空 chunk 表示结果集耗尽。
        if req.NumRows() == 0 {
            return Ok(rows);
        }
        rows.extend(req.copy_rows());
        req = req.renew(maxChunkSize);
    }
}

/// Drain 后始终尝试 Close；Close 失败只记日志，不覆盖 Drain 结果。
pub fn DrainRecordSetAndClose(
    ctx: &context::Context,
    rs: &mut dyn RecordSet,
    maxChunkSize: usize,
) -> Result<Vec<chunk::Row>, DrainError> {
    let result = DrainRecordSet(ctx, rs, maxChunkSize);
    if let Err(closeErr) = rs.Close() {
        logutil::BgLogger().error(format!(
            "failed to close recordSet in DrainRecordSetAndClose: {closeErr}"
        ));
    }
    result
}

/// 经 `ExecuteInternal` 执行 SQL，Drain 结果集并保证 Close。
pub fn ExecSQL(
    ctx: &context::Context,
    exec: &mut dyn SQLExecutor,
    sql: &str,
    args: Vec<Box<dyn Any>>,
) -> Result<Option<Vec<chunk::Row>>, GoError> {
    let mut rs = exec.ExecuteInternal(ctx, sql, args)?;
    // nil 结果集对应无返回行的语句（如 SET）。
    let Some(recordSet) = rs.as_mut() else {
        return Ok(None);
    };

    let result =
        DrainRecordSet(ctx, recordSet.as_mut(), 1024).map_err(|error| Box::new(error) as GoError);
    // 对齐 Go terror.Call：Close 错误单独处理，不掩盖 Drain 结果。
    terror::Call(|| {
        recordSet.Close().map_err(|source| DrainError {
            rows: Vec::new(),
            source,
        })
    });
    result.map(Some)
}
