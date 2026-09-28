// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! 测试用 TiDB Glue 替身，对齐 Go `br/pkg/gluetidb/mock/mock.go`。
//! 重型 TiDB 类型（`sessionapi.Session`、`kv.Storage`、DDL/model）用本地桩替代，
//! 使本包避开 arm64 上 grpcio 重建路径；调用形状与 Go 一致，未实现 DDL 路径 panic。
//! Test glue matching `br/pkg/gluetidb/mock/mock.go`.
//!
//! Heavy TiDB types (`sessionapi.Session`, `kv.Storage`, DDL/model) are local
//! stand-ins so this package stays free of the grpcio rebuild path on arm64.

use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use astersql_br_pkg_glue::{
    BatchCreateTableSession, CIStr, ClientCLP, Context, CreateTableOption, DBInfo, Domain,
    Glue as GlueTrait, GlueClient, PolicyInfo, Progress, RefreshMetaArgs, SecurityOption, Session,
    SessionCtxHandle, Storage, TableInfo, TableMode,
};
use astersql_errors::SharedError;

/// 对应 `kv.InternalTxnBR`（`pkg/kv/option.go`），标记 BR 内部事务来源。
/// Matches `kv.InternalTxnBR` (`pkg/kv/option.go`).
pub const InternalTxnBR: &str = "br";

// 线程局部记录最近一次 WithInternalSourceType 的 source，供单测断言。
thread_local! {
    static LAST_INTERNAL_SOURCE: std::cell::Cell<Option<&'static str>> =
        const { std::cell::Cell::new(None) };
}

/// `kv.WithInternalSourceType` 的测试替身：记录 source 后原样返回 ctx。
/// Stand-in for `kv.WithInternalSourceType` — records the source for tests.
pub fn WithInternalSourceType(ctx: Context, source: &'static str) -> Context {
    LAST_INTERNAL_SOURCE.with(|c| c.set(Some(source)));
    ctx
}

/// 测试辅助：取出并清空最近一次内部事务 source。
/// Test helper: last source passed to [`WithInternalSourceType`].
pub fn take_last_internal_source_for_test() -> Option<&'static str> {
    LAST_INTERNAL_SOURCE.with(|c| c.take())
}

/// `chunk.Chunk` 替身，仅在排空结果集时使用。
/// Chunk stand-in for `chunk.Chunk` used when draining a result set.
#[derive(Clone, Debug, Default)]
pub struct Chunk {
    pub num_rows: usize,
}

/// `sqlexec.RecordSet` 替身，覆盖 Next / Close / NewChunk。
/// Record set stand-in for `sqlexec.RecordSet` (Next / Close / NewChunk).
pub trait RecordSet: Send {
    fn NewChunk(&mut self, capacity: Option<usize>) -> Chunk;
    fn Next(&mut self, ctx: Context, chunk: &mut Chunk) -> Result<(), SharedError>;
    fn Close(&mut self);
}

/// Owns a result set and mirrors Go's `defer rs.Close()` on every exit path.
struct CloseRecordSet(Box<dyn RecordSet>);

impl Drop for CloseRecordSet {
    fn drop(&mut self) {
        self.0.Close();
    }
}

/// `sessionapi.Session` 中 MockSession 所需方法的注入接口。
/// Stand-in for `sessionapi.Session` methods used by [`MockSession`].
pub trait SessionAPI: Send {
    fn ExecuteInternal(
        &mut self,
        ctx: Context,
        sql: &str,
        args: &[Box<dyn Any + Send>],
    ) -> Result<Option<Box<dyn RecordSet>>, SharedError>;
    fn Close(&mut self);
    fn session_ctx_handle(&self) -> SessionCtxHandle;
}

/// [`MockGlue::Open`] 返回的空 Storage（Go 返回 `nil, nil`）。
/// Storage stand-in returned by [`MockGlue::Open`] (Go returns `nil, nil`).
struct NilStorage;

impl Storage for NilStorage {
    // 空名对齐 Go nil store 的零值语义。
    fn name(&self) -> &str {
        ""
    }
}

/// Go `StartProgress` 返回 nil 时的 Progress 替身（Inc/Close 可观测但无 IO）。
/// Progress stand-in for Go's `nil` StartProgress (no-op Inc/Close).
struct NopProgress {
    current: AtomicI64,
    closed: AtomicBool,
}

impl Progress for NopProgress {
    fn Inc(&self) {
        self.IncBy(1);
    }
    fn IncBy(&self, cnt: i64) {
        // Relaxed 即可：仅单测读计数，无跨线程发布约束。
        self.current.fetch_add(cnt, Ordering::Relaxed);
    }
    fn GetCurrent(&self) -> i64 {
        self.current.load(Ordering::Relaxed)
    }
    fn Close(&self) {
        // SeqCst 保证 Close 对后续断言可见。
        self.closed.store(true, Ordering::SeqCst);
    }
}

/// 测试用 Session：包装可选 SessionAPI，并缓存 GlobalVars 查询结果。
/// mockSession is used for test.
pub struct MockSession {
    se: Option<Arc<Mutex<dyn SessionAPI>>>,
    globalVars: HashMap<String, String>,
}

impl MockSession {
    /// 构造 mock session；`se` 为 None 时多数 SQL/DDL 路径会 panic 或缺底层句柄。
    pub fn new(
        se: Option<Arc<Mutex<dyn SessionAPI>>>,
        globalVars: HashMap<String, String>,
    ) -> Self {
        Self { se, globalVars }
    }

    // 取底层 SessionAPI；未注入时与 Go 解引用 nil 一样直接 panic。
    fn se(&self) -> Arc<Mutex<dyn SessionAPI>> {
        self.se.as_ref().expect("nil sessionapi.Session").clone()
    }
}

impl Session for MockSession {
    // GetSessionCtx：转发到底层 session 句柄，供依赖 SessionCtx 的调用方使用。
    // GetSessionCtx implements glue.Session — returns the underlying session handle.
    fn GetSessionCtx(&mut self) -> SessionCtxHandle {
        self.se().lock().expect("session lock").session_ctx_handle()
    }

    // Execute：无绑定参数的 ExecuteInternal 薄封装。
    // Execute implements glue.Session.
    fn Execute(&mut self, ctx: Context, sql: &str) -> Result<(), SharedError> {
        self.ExecuteInternal(ctx, sql, &[])
    }

    fn ExecuteInternal(
        &mut self,
        ctx: Context,
        sql: &str,
        args: &[Box<dyn Any + Send>],
    ) -> Result<(), SharedError> {
        // 对齐 Go：先打上 InternalTxnBR 来源再执行内部 SQL。
        // Go: ctx = kv.WithInternalSourceType(ctx, kv.InternalTxnBR)
        let ctx = WithInternalSourceType(ctx, InternalTxnBR);
        let rs = {
            let se_arc = self.se();
            let mut se = se_arc.lock().expect("session lock");
            se.ExecuteInternal(ctx.clone(), sql, args)?
        };

        // 部分 SQL（如 ADMIN RECOVER INDEX）在轮询结果集时才产生副作用；
        // 至少调用一次 Next 触发副作用（Go 同样未排空全部行）。
        // Some of SQLs (like ADMIN RECOVER INDEX) may lazily take effect
        // when we are polling the result set.
        // At least call `next` once for triggering theirs side effect.
        // (Maybe we'd better drain all returned rows?)
        if let Some(rs) = rs {
            // Go: defer rs.Close() — 无论 Next 成败都关闭结果集。
            let mut rs = CloseRecordSet(rs);
            let mut c = rs.0.NewChunk(None);
            let next_err = rs.0.Next(ctx, &mut c);
            // Go 在 Next 失败时仍返回成功（吞掉错误）。
            // Go returns nil (success) when Next fails.
            if next_err.is_err() {
                return Ok(());
            }
        }
        Ok(())
    }

    // CreateDatabaseOnExistError：mock 未实现，对齐 Go log.Fatal。
    // CreateDatabaseOnExistError implements glue.Session.
    fn CreateDatabaseOnExistError(
        &mut self,
        _ctx: Context,
        _schema: &DBInfo,
    ) -> Result<(), SharedError> {
        panic!("unimplemented CreateDatabase for mock session");
    }

    // CreatePlacementPolicy：mock 未实现。
    // CreatePlacementPolicy implements glue.Session.
    fn CreatePlacementPolicy(
        &mut self,
        _ctx: Context,
        _policy: &PolicyInfo,
    ) -> Result<(), SharedError> {
        panic!("unimplemented CreateDatabase for mock session");
    }

    // CreateTable：mock 未实现。
    // CreateTable implements glue.Session.
    fn CreateTable(
        &mut self,
        _ctx: Context,
        _dbName: CIStr,
        _table: &TableInfo,
        _cs: Vec<CreateTableOption>,
    ) -> Result<(), SharedError> {
        panic!("unimplemented CreateDatabase for mock session");
    }

    // Close：关闭底层 SessionAPI。
    // Close implements glue.Session.
    fn Close(&mut self) {
        self.se().lock().expect("session lock").Close();
    }

    // GetGlobalVariable：优先查注入表，缺省返回 "True"（对齐 Go mock）。
    // GetGlobalVariable implements glue.Session.
    fn GetGlobalVariable(&mut self, name: &str) -> Result<String, SharedError> {
        if let Some(ret) = self.globalVars.get(name) {
            return Ok(ret.clone());
        }
        Ok("True".to_string())
    }

    // GetGlobalSysVar：mock 固定空串。
    // GetGlobalSysVar implements glue.Session.
    fn GetGlobalSysVar(&mut self, _name: &str) -> Result<String, SharedError> {
        Ok(String::new())
    }

    // AlterTableMode：mock 未实现。
    // AlterTableMode implements glue.Session.
    fn AlterTableMode(
        &mut self,
        _ctx: Context,
        _schemaID: i64,
        _tableID: i64,
        _tableMode: TableMode,
    ) -> Result<(), SharedError> {
        panic!("unimplemented AlterTableMode for mock session");
    }

    // RefreshMeta：mock 未实现。
    // RefreshMeta implements glue.Session.
    fn RefreshMeta(&mut self, _ctx: Context, _args: &RefreshMetaArgs) -> Result<(), SharedError> {
        panic!("unimplemented RefreshMeta for mock session");
    }
}

impl BatchCreateTableSession for MockSession {
    // CreateTables：批量建表在 mock 中未实现。
    // CreateTables implements glue.BatchCreateTableSession.
    fn CreateTables(
        &mut self,
        _ctx: Context,
        _tables: HashMap<String, Vec<TableInfo>>,
        _cs: Vec<CreateTableOption>,
    ) -> Result<(), SharedError> {
        panic!("unimplemented CreateDatabase for mock session");
    }
}

/// 仅测试使用的 Glue：可注入 SessionAPI 与全局变量映射。
/// MockGlue only used for test.
#[derive(Default)]
pub struct MockGlue {
    se: Option<Arc<Mutex<dyn SessionAPI>>>,
    // 注入到 CreateSession 产生的 MockSession；UseOneShotSession 故意不带此表。
    pub GlobalVars: HashMap<String, String>,
}

impl MockGlue {
    /// 默认空会话、空 GlobalVars。
    pub fn new() -> Self {
        Self::default()
    }

    /// 注入底层 SessionAPI，供后续 CreateSession / Execute 使用。
    pub fn SetSession(&mut self, se: Arc<Mutex<dyn SessionAPI>>) {
        self.se = Some(se);
    }

    /// 清除已注入会话，模拟 Go 侧重新构造空 mock。
    pub fn clear_session(&mut self) {
        self.se = None;
    }
}

impl GlueTrait for MockGlue {
    // GetDomain：Go 返回 (nil, nil)；此处用空 Domain 表示无错误。
    // GetDomain implements glue.Glue — Go returns (nil, nil).
    fn GetDomain(&self, _store: &dyn Storage) -> Result<Arc<Domain>, SharedError> {
        Ok(Arc::new(Domain))
    }

    // CreateSession：克隆当前 se 与 GlobalVars 到新 MockSession。
    // CreateSession implements glue.Glue.
    fn CreateSession(&self, _store: &dyn Storage) -> Result<Box<dyn Session>, SharedError> {
        Ok(Box::new(MockSession::new(
            self.se.clone(),
            self.GlobalVars.clone(),
        )))
    }

    // Open：返回 NilStorage，对齐 Go (nil, nil)。
    // Open implements glue.Glue — Go returns (nil, nil).
    fn Open(&self, _path: &str, _option: SecurityOption) -> Result<Box<dyn Storage>, SharedError> {
        Ok(Box::new(NilStorage))
    }

    // OwnsStorage：mock 声明拥有 storage（与 Go 一致为 true）。
    // OwnsStorage implements glue.Glue.
    fn OwnsStorage(&self) -> bool {
        true
    }

    // StartProgress：返回可计数的 NopProgress，对齐 Go 的 nil progress 契约。
    // StartProgress implements glue.Glue — Go returns nil progress.
    fn StartProgress(
        &self,
        _ctx: Context,
        _cmdName: &str,
        _total: i64,
        _redirectLog: bool,
    ) -> Box<dyn Progress> {
        Box::new(NopProgress {
            current: AtomicI64::new(0),
            closed: AtomicBool::new(false),
        })
    }

    // Record：Go 为空实现，此处同样忽略指标。
    // Record implements glue.Glue — empty in Go.
    fn Record(&self, _name: &str, _value: u64) {}

    // GetVersion：固定 "mock glue"，便于单测识别替身。
    // GetVersion implements glue.Glue.
    fn GetVersion(&self) -> String {
        "mock glue".to_string()
    }

    // UseOneShotSession：临时 MockSession 只带 se，不带 GlobalVars（对齐 Go）。
    // UseOneShotSession implements glue.Glue.
    // Go creates a temporary mockSession with only `se` (no GlobalVars).
    fn UseOneShotSession(
        &self,
        _store: &dyn Storage,
        _closeDomain: bool,
        fn_: &mut dyn FnMut(Box<dyn Session>) -> Result<(), SharedError>,
    ) -> Result<(), SharedError> {
        // 空 HashMap 使 GetGlobalVariable 走默认 "True"。
        let glueSession = MockSession::new(self.se.clone(), HashMap::new());
        fn_(Box::new(glueSession))
    }

    // GetClient：返回 ClientCLP，与生产 BR 客户端类型一致。
    fn GetClient(&self) -> GlueClient {
        ClientCLP
    }
}
