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

//! Glue abstractions matching `br/pkg/glue/glue.go`.
//!
//! Heavy TiDB types (`kv.Storage`, `domain.Domain`, DDL/model structs) are
//! represented as crate-local stand-ins so this package stays free of the
//! grpcio rebuild path that currently fails on arm64. Downstream gluetidb /
//! gluetikv tasks wire the concrete crates.
//!
//! 本模块是 BR 对 TiDB/KV 能力的抽象层，语义对齐 Go `glue.go`。
//! 重型依赖用本地桩类型占位，避免本 crate 拉入 arm64 上失败的 grpcio 路径；
//! 真正实现由 `gluetidb` / `gluetikv` 等下游包注入。

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use astersql_errors::SharedError;

use crate::console_glue::ConsoleGlue;

/// Cancellation-aware context matching Go `context.Context` for BR glue call sites.
/// Downstream tasks may replace this with `astersql_util_sqlexec::context::Context`
/// once the arm64 grpcio rebuild path is healthy.
/// 轻量取消上下文：BR glue 调用点需要可协作取消，而不依赖完整 sessionctx。
#[derive(Clone, Debug, Default)]
pub struct Context {
    // 多处共享同一取消标志；clone 后 cancel 仍全局可见。
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}

impl Context {
    // 默认未取消，供同步路径构造临时上下文。
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    // 进度 Wait 等循环据此提前返回，对齐 Go ctx.Done。
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::Relaxed)
    }

    // 置位后所有 clone 的 Context 均视为已取消。
    pub fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// GlueClient distinguishes BR CLI vs SQL entry points.
/// 入口形态：CLI（CLP）与 SQL 内嵌，影响 OwnsStorage / 会话生命周期策略。
pub type GlueClient = i32;

// 与 Go iota 一致：0=命令行 BR，1=通过 SQL 触发。
pub const ClientCLP: GlueClient = 0;
pub const ClientSql: GlueClient = 1;

/// PD security option matching `github.com/tikv/pd/client.SecurityOption`.
/// 打开 PD/存储时传入的 TLS 路径三元组，字段名保持 Go 风格。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SecurityOption {
    pub CAPath: String,
    pub CertPath: String,
    pub KeyPath: String,
}

/// Stand-in for `kv.Storage` until gluetikv wires `astersql-kv`.
/// 存储抽象桩：仅保留 BR 路径需要的最小能力，真实实现由 gluetikv 接线。
pub trait Storage: Send + Sync {
    fn name(&self) -> &str {
        "storage"
    }

    /// Keyspace encoded by this storage. Classic (non-keyspace) stores use the
    /// TiKV nullspace sentinel, matching `storage.GetCodec().GetKeyspaceID()`.
    fn keyspace_id(&self) -> u32 {
        u32::MAX
    }
}

/// Stand-in for `domain.Domain`.
/// Domain 桩：GetDomain 返回类型占位，避免依赖完整 domain crate。
#[derive(Debug, Default)]
pub struct Domain;

/// Stand-in for `model.DBInfo`.
/// 库元信息桩，供 CreateDatabaseOnExistError 等 DDL 入口传参。
#[derive(Clone, Debug, Default)]
pub struct DBInfo;

/// Stand-in for `model.TableInfo`.
/// 表元信息桩，CreateTable / CreateTables 使用。
#[derive(Clone, Debug, Default)]
pub struct TableInfo;

/// Stand-in for `model.PolicyInfo`.
/// 放置策略桩，CreatePlacementPolicy 使用。
#[derive(Clone, Debug, Default)]
pub struct PolicyInfo;

/// Stand-in for `model.TableMode`.
/// 表模式枚举桩，AlterTableMode 透传。
pub type TableMode = i32;

/// Stand-in for `model.RefreshMetaArgs`.
/// 刷新元数据参数桩，对齐 Go RefreshMeta 入参。
#[derive(Clone, Debug, Default)]
pub struct RefreshMetaArgs;

/// Stand-in for `ast.CIStr`.
/// 大小写不敏感标识：O 为原始串，L 为小写形式，与 Go CIStr 字段一致。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CIStr {
    pub O: String,
    pub L: String,
}

/// Create-table option matching `ddl.CreateTableOption`.
/// 建表可选钩子；Rust 用 FnOnce 盒子近似 Go 的 option 函数。
pub type CreateTableOption = Box<dyn FnOnce() + Send>;

/// Opaque sessionctx.Context handle (Rust sessionctx::Context is not object-safe).
/// 会话上下文句柄：因 trait 非对象安全，用 Any 擦除后跨边界传递。
pub type SessionCtxHandle = Arc<dyn Any + Send + Sync>;

/// Glue is an abstraction of TiDB function calls used in BR.
/// BR 侧调用 TiDB 能力的统一门面；CLI 与 SQL 各有实现。
pub trait Glue: Send + Sync {
    // 由存储拿到 Domain，备份/恢复元数据路径依赖。
    fn GetDomain(&self, store: &dyn Storage) -> Result<Arc<Domain>, SharedError>;
    // 创建可执行 SQL/DDL 的会话。
    fn CreateSession(&self, store: &dyn Storage) -> Result<Box<dyn Session>, SharedError>;
    // 按路径与 TLS 选项打开底层存储（PD 地址等）。
    fn Open(&self, path: &str, option: SecurityOption) -> Result<Box<dyn Storage>, SharedError>;

    /// OwnsStorage returns whether the storage returned by Open() is owned.
    /// 为 true 时调用方负责关闭存储；SQL 入口常为 false（复用外部 store）。
    fn OwnsStorage(&self) -> bool;

    // 启动进度条；redirectLog 控制是否把进度打到日志而非 TTY。
    fn StartProgress(
        &self,
        ctx: Context,
        cmdName: &str,
        total: i64,
        redirectLog: bool,
    ) -> Box<dyn Progress>;

    // 记录命名指标/计数，供外部观测。
    fn Record(&self, name: &str, value: u64);

    // 返回 BR/TiDB 版本字符串，用于备份元数据。
    fn GetVersion(&self) -> String;

    // 一次性会话：执行回调后按 closeDomain 决定是否拆掉 Domain。
    fn UseOneShotSession(
        &self,
        store: &dyn Storage,
        closeDomain: bool,
        fn_: &mut dyn FnMut(Box<dyn Session>) -> Result<(), SharedError>,
    ) -> Result<(), SharedError>;

    // 报告当前 Glue 来自 CLI 还是 SQL。
    fn GetClient(&self) -> GlueClient;

    /// Optional console glue; Go uses a type assertion on the same value.
    /// Go 用类型断言取控制台；Rust 默认 None，实现方可覆盖。
    fn AsConsoleGlue(&self) -> Option<Arc<dyn ConsoleGlue>> {
        None
    }
}

/// Session is an abstraction of the session.Session interface.
/// 会话抽象：覆盖 BR 恢复路径需要的 DDL/变量/元数据刷新能力。
pub trait Session: Send {
    // 执行用户可见 SQL。
    fn Execute(&mut self, ctx: Context, sql: &str) -> Result<(), SharedError>;
    // 内部 SQL，可带绑定参数盒子（Any 擦除）。
    fn ExecuteInternal(
        &mut self,
        ctx: Context,
        sql: &str,
        args: &[Box<dyn Any + Send>],
    ) -> Result<(), SharedError>;
    // 建库：库已存在时按 Go 语义返回错误而非静默忽略。
    fn CreateDatabaseOnExistError(
        &mut self,
        ctx: Context,
        schema: &DBInfo,
    ) -> Result<(), SharedError>;
    // 单表创建，cs 为可选建表钩子列表。
    fn CreateTable(
        &mut self,
        ctx: Context,
        dbName: CIStr,
        table: &TableInfo,
        cs: Vec<CreateTableOption>,
    ) -> Result<(), SharedError>;
    // 创建放置策略。
    fn CreatePlacementPolicy(
        &mut self,
        ctx: Context,
        policy: &PolicyInfo,
    ) -> Result<(), SharedError>;
    // 释放会话资源。
    fn Close(&mut self);
    // 读全局变量（兼容旧路径命名）。
    fn GetGlobalVariable(&mut self, name: &str) -> Result<String, SharedError>;
    // 读全局系统变量。
    fn GetGlobalSysVar(&mut self, name: &str) -> Result<String, SharedError>;
    // 取出擦除后的 sessionctx 句柄供下游断言。
    fn GetSessionCtx(&mut self) -> SessionCtxHandle;
    // 修改表模式（如 restore 临时模式）。
    fn AlterTableMode(
        &mut self,
        ctx: Context,
        schemaID: i64,
        tableID: i64,
        tableMode: TableMode,
    ) -> Result<(), SharedError>;
    // 按参数刷新 infoschema / 元数据缓存。
    fn RefreshMeta(&mut self, ctx: Context, args: &RefreshMetaArgs) -> Result<(), SharedError>;
}

/// BatchCreateTableSession is an interface to batch create table parallelly.
/// 批量并行建表扩展；tables 按库名分组，对齐 Go BatchCreateTableSession。
pub trait BatchCreateTableSession {
    fn CreateTables(
        &mut self,
        ctx: Context,
        tables: HashMap<String, Vec<TableInfo>>,
        cs: Vec<CreateTableOption>,
    ) -> Result<(), SharedError>;
}

/// Progress is an interface recording the current execution progress.
/// 进度接口：Inc/IncBy 推进，Close 结束展示；实现可映射 mpb 或日志条。
pub trait Progress: Send + Sync {
    fn Inc(&self);
    fn IncBy(&self, cnt: i64);
    fn GetCurrent(&self) -> i64;
    fn Close(&self);
}

/// WithProgress execute some logic with the progress, and close it once done.
/// 包装 StartProgress + 回调 + 必定 Close，保证错误路径也释放进度条。
pub fn WithProgress<F>(
    ctx: Context,
    g: &dyn Glue,
    cmdName: &str,
    total: i64,
    redirectLog: bool,
    cc: F,
) -> Result<(), SharedError>
where
    F: FnOnce(&dyn Progress) -> Result<(), SharedError>,
{
    let p = g.StartProgress(ctx, cmdName, total, redirectLog);
    struct CloseOnDrop<'a>(&'a dyn Progress);
    impl Drop for CloseOnDrop<'_> {
        fn drop(&mut self) {
            self.0.Close();
        }
    }
    // RAII 对齐 Go defer：Ok、Err 与 panic unwind 都会收尾。
    let _close = CloseOnDrop(p.as_ref());
    cc(p.as_ref())
}
