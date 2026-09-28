// Copyright 2026 AsterSQL.

//! 与 Go `br/pkg/gluetidb/mock/mock.go` 的公开契约对等测试。
//! 覆盖正常路径、边界（GlobalVars / Next 错误吞掉）、资源关闭与未实现 DDL panic。
//! Parity tests for `br/pkg/gluetidb/mock` vs Go `mock.go`.

use std::any::Any;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use astersql_br_pkg_glue::{
    BatchCreateTableSession, CIStr, ClientCLP, Context, DBInfo, Glue as GlueTrait, PolicyInfo,
    RefreshMetaArgs, SecurityOption, Session, Storage, TableInfo,
};
use astersql_errors::{New, SharedError};

use crate::{
    Chunk, InternalTxnBR, MockGlue, MockSession, RecordSet, SessionAPI,
    take_last_internal_source_for_test,
};

// 占位 Storage：Open/CreateSession 只要求实现 trait，不关心 name。
struct DummyStore;
impl Storage for DummyStore {}

// 可注入 Next 错误与关闭观测的结果集替身。
struct FakeRecordSet {
    next_err: Option<String>,
    panic_in_next: bool,
    next_calls: Arc<AtomicUsize>,
    closed: Arc<AtomicBool>,
}

impl RecordSet for FakeRecordSet {
    // NewChunk 返回空块即可，Execute 只关心 Next/Close。
    fn NewChunk(&mut self, _capacity: Option<usize>) -> Chunk {
        Chunk::default()
    }
    fn Next(&mut self, _ctx: Context, _chunk: &mut Chunk) -> Result<(), SharedError> {
        // 计数供断言「至少调用一次 Next」以触发副作用。
        self.next_calls.fetch_add(1, Ordering::SeqCst);
        if self.panic_in_next {
            panic!("next panic");
        }
        // take 一次性错误，避免重复失败干扰后续断言。
        if let Some(msg) = self.next_err.take() {
            return Err(New(msg));
        }
        Ok(())
    }
    fn Close(&mut self) {
        // SeqCst 保证 Execute 返回后断言可见 Close。
        self.closed.store(true, Ordering::SeqCst);
    }
}

// 可配置错误/结果集的 SessionAPI，用于驱动 MockSession 执行路径。
struct FakeSession {
    closed: Arc<AtomicBool>,
    last_sql: Arc<Mutex<String>>,
    execute_err: Option<String>,
    /// 置位时 ExecuteInternal 返回 FakeRecordSet。
    /// When set, ExecuteInternal returns a result set.
    with_rs: bool,
    rs_next_err: Option<String>,
    rs_next_panics: bool,
    rs_next_calls: Arc<AtomicUsize>,
    rs_closed: Arc<AtomicBool>,
    ctx_handle: Arc<()>,
}

impl FakeSession {
    // 默认无错误、无结果集，便于各子场景按需改字段。
    fn new() -> Self {
        Self {
            closed: Arc::new(AtomicBool::new(false)),
            last_sql: Arc::new(Mutex::new(String::new())),
            execute_err: None,
            with_rs: false,
            rs_next_err: None,
            rs_next_panics: false,
            rs_next_calls: Arc::new(AtomicUsize::new(0)),
            rs_closed: Arc::new(AtomicBool::new(false)),
            ctx_handle: Arc::new(()),
        }
    }
}

impl SessionAPI for FakeSession {
    fn ExecuteInternal(
        &mut self,
        _ctx: Context,
        sql: &str,
        _args: &[Box<dyn Any + Send>],
    ) -> Result<Option<Box<dyn RecordSet>>, SharedError> {
        // 记录 SQL，供断言转发内容。
        *self.last_sql.lock().unwrap() = sql.to_string();
        // 优先返回注入错误，模拟底层 session 失败。
        if let Some(msg) = self.execute_err.take() {
            return Err(New(msg));
        }
        // with_rs 时移交可观测的 FakeRecordSet。
        if self.with_rs {
            return Ok(Some(Box::new(FakeRecordSet {
                next_err: self.rs_next_err.take(),
                panic_in_next: self.rs_next_panics,
                next_calls: Arc::clone(&self.rs_next_calls),
                closed: Arc::clone(&self.rs_closed),
            })));
        }
        // 无结果集路径：对齐多数 DDL/简单 SQL。
        Ok(None)
    }
    // Close 标记供 Session.Close 资源断言。
    fn Close(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
    }
    // 返回稳定句柄，满足 GetSessionCtx。
    fn session_ctx_handle(&self) -> astersql_br_pkg_glue::SessionCtxHandle {
        Arc::clone(&self.ctx_handle) as astersql_br_pkg_glue::SessionCtxHandle
    }
}

/// Go 在取得结果集后立即 defer Close；即使 Next panic，资源也必须关闭。
#[test]
fn result_set_closes_when_next_panics() {
    let mut fake = FakeSession::new();
    fake.with_rs = true;
    fake.rs_next_panics = true;
    let rs_closed = Arc::clone(&fake.rs_closed);
    let se_api: Arc<Mutex<dyn SessionAPI>> = Arc::new(Mutex::new(fake));
    let mut se = MockSession::new(Some(se_api), HashMap::new());

    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = se.Execute(Context::new(), "ADMIN RECOVER INDEX t");
    }));

    assert!(result.is_err(), "Next panic must propagate");
    assert!(
        rs_closed.load(Ordering::SeqCst),
        "RecordSet.Close must match Go defer during unwinding"
    );
}

/// 对照 Go mock 公开行为：版本/存储/会话变量/Execute/DDL panic/进度。
#[test]
fn go_rust_public_contract_matches() {
    let store = DummyStore;

    // --- 正常：版本、OwnsStorage、客户端类型 ---
    // --- normal: GetVersion / OwnsStorage / GetClient ---
    let g = MockGlue::new();
    // 固定版本串便于识别 mock。
    assert_eq!(g.GetVersion(), "mock glue");
    // mock 声明拥有 storage。
    assert!(g.OwnsStorage());
    // 客户端类型与生产 BR 一致为 ClientCLP。
    assert_eq!(g.GetClient(), ClientCLP);

    // --- 正常：GetDomain / Open 成功（Go nil,nil → 无错误） ---
    // --- normal: GetDomain / Open succeed (Go nil,nil → no error) ---
    assert!(g.GetDomain(&store).is_ok());
    let opened = g.Open("tikv://x", SecurityOption::default()).expect("open");
    // NilStorage.name 为空串。
    assert_eq!(opened.name(), "");

    // --- 正常：CreateSession 后全局变量默认 "True" ---
    // --- normal: CreateSession + GetGlobalVariable default "True" ---
    let mut se = g.CreateSession(&store).expect("create session");
    // new_collation_enabled 未注入时应为 "True"。
    assert_eq!(
        se.GetGlobalVariable("new_collation_enabled").unwrap(),
        "True"
    );
    // GetGlobalSysVar 在 mock 中固定空串。
    assert_eq!(se.GetGlobalSysVar("any").unwrap(), "");

    // --- 边界：GlobalVars 覆盖指定键，未覆盖键仍默认 True ---
    // --- boundary: GlobalVars override ---
    let mut g = MockGlue::new();
    g.GlobalVars
        .insert("new_collation_enabled".into(), "False".into());
    let mut se = g.CreateSession(&store).unwrap();
    // 注入键走覆盖值。
    assert_eq!(
        se.GetGlobalVariable("new_collation_enabled").unwrap(),
        "False"
    );
    // 未注入键仍回落默认。
    assert_eq!(se.GetGlobalVariable("other").unwrap(), "True");

    // --- 正常：Execute 打上 InternalTxnBR 并转发 SQL ---
    // --- normal: Execute path stamps InternalTxnBR and forwards SQL ---
    let fake = FakeSession::new();
    let last_sql = Arc::clone(&fake.last_sql);
    let se_api: Arc<Mutex<dyn SessionAPI>> = Arc::new(Mutex::new(fake));
    let mut g = MockGlue::new();
    g.SetSession(Arc::clone(&se_api));
    let mut se = g.CreateSession(&store).unwrap();
    // 先清空 TLS，避免前序用例残留。
    let _ = take_last_internal_source_for_test();
    se.Execute(Context::new(), "SELECT 1").unwrap();
    // 必须打上 BR 内部事务来源。
    assert_eq!(take_last_internal_source_for_test(), Some(InternalTxnBR));
    // SQL 原文应原样转发到底层。
    assert_eq!(last_sql.lock().unwrap().as_str(), "SELECT 1");

    // --- 错误：底层 ExecuteInternal 失败应向上传播 ---
    // --- error: ExecuteInternal propagates session error ---
    {
        let mut fake = FakeSession::new();
        fake.execute_err = Some("boom".into());
        let se_api: Arc<Mutex<dyn SessionAPI>> = Arc::new(Mutex::new(fake));
        let mut g = MockGlue::new();
        g.SetSession(se_api);
        let mut se = g.CreateSession(&store).unwrap();
        let err = se.Execute(Context::new(), "BAD").unwrap_err();
        let msg = format!("{err:?}");
        // 错误信息应包含注入的 boom。
        assert!(msg.contains("boom"), "{msg}");
    }

    // --- 边界+资源：结果集 Next 失败仍返回 Ok，且必须 Close ---
    // --- boundary + resource: result set Next error → Ok, but Close still runs ---
    {
        let mut fake = FakeSession::new();
        fake.with_rs = true;
        fake.rs_next_err = Some("next failed".into());
        let next_calls = Arc::clone(&fake.rs_next_calls);
        let rs_closed = Arc::clone(&fake.rs_closed);
        let se_api: Arc<Mutex<dyn SessionAPI>> = Arc::new(Mutex::new(fake));
        let mut g = MockGlue::new();
        g.SetSession(se_api);
        let mut se = g.CreateSession(&store).unwrap();
        // ADMIN RECOVER INDEX 依赖至少一次 Next 触发副作用。
        se.Execute(Context::new(), "ADMIN RECOVER INDEX t").unwrap();
        assert_eq!(next_calls.load(Ordering::SeqCst), 1);
        assert!(
            rs_closed.load(Ordering::SeqCst),
            "RecordSet.Close must run after Next error"
        );
    }

    // --- 资源：Session.Close 关闭底层 SessionAPI ---
    // --- resource: Close closes underlying session ---
    {
        let fake = FakeSession::new();
        let closed = Arc::clone(&fake.closed);
        let se_api: Arc<Mutex<dyn SessionAPI>> = Arc::new(Mutex::new(fake));
        let mut g = MockGlue::new();
        g.SetSession(se_api);
        let mut se = g.CreateSession(&store).unwrap();
        se.Close();
        assert!(closed.load(Ordering::SeqCst));
    }

    // --- UseOneShotSession：调用 fn，且不传入 GlobalVars ---
    // --- UseOneShotSession: invokes fn, does NOT pass GlobalVars ---
    {
        let mut g = MockGlue::new();
        g.GlobalVars.insert("k".into(), "v".into());
        let fake = FakeSession::new();
        g.SetSession(Arc::new(Mutex::new(fake)));
        let mut saw = false;
        g.UseOneShotSession(&store, false, &mut |mut se| {
            saw = true;
            // Go UseOneShotSession 省略 globalVars → 默认 "True"
            // Go UseOneShotSession omits globalVars → default "True"
            assert_eq!(se.GetGlobalVariable("k").unwrap(), "True");
            Ok(())
        })
        .unwrap();
        assert!(saw);
    }

    // --- 错误：未实现 DDL/meta 方法 panic（对齐 Go log.Fatal） ---
    // --- error: unimplemented DDL / meta methods panic (Go log.Fatal) ---
    {
        let mut se = MockSession::new(None, HashMap::new());
        // GetGlobalVariable 不依赖底层 session，仍可读默认值。
        // GetGlobalVariable works without underlying session
        assert_eq!(se.GetGlobalVariable("x").unwrap(), "True");

        // 用 catch_unwind 断言每条未实现路径都会 panic。
        let panics = |f: Box<dyn FnOnce() + Send>| {
            let r = catch_unwind(AssertUnwindSafe(f));
            assert!(r.is_err(), "expected panic");
        };
        // CreateDatabaseOnExistError 未实现。
        panics(Box::new(|| {
            let mut se = MockSession::new(None, HashMap::new());
            let _ = se.CreateDatabaseOnExistError(Context::new(), &DBInfo::default());
        }));
        // CreatePlacementPolicy 未实现。
        panics(Box::new(|| {
            let mut se = MockSession::new(None, HashMap::new());
            let _ = se.CreatePlacementPolicy(Context::new(), &PolicyInfo::default());
        }));
        // CreateTable 未实现。
        panics(Box::new(|| {
            let mut se = MockSession::new(None, HashMap::new());
            let _ = se.CreateTable(
                Context::new(),
                CIStr::default(),
                &TableInfo::default(),
                vec![],
            );
        }));
        // 批量 CreateTables 未实现。
        panics(Box::new(|| {
            let mut se = MockSession::new(None, HashMap::new());
            let _ = BatchCreateTableSession::CreateTables(
                &mut se,
                Context::new(),
                HashMap::new(),
                vec![],
            );
        }));
        // AlterTableMode 未实现。
        panics(Box::new(|| {
            let mut se = MockSession::new(None, HashMap::new());
            let _ = se.AlterTableMode(Context::new(), 1, 2, 0);
        }));
        // RefreshMeta 未实现。
        panics(Box::new(|| {
            let mut se = MockSession::new(None, HashMap::new());
            let _ = se.RefreshMeta(Context::new(), &RefreshMetaArgs::default());
        }));
    }

    // --- StartProgress：nop 进度可 Inc/Close（Go 返回 nil） ---
    // --- StartProgress: nop progress Inc/Close (Go returns nil) ---
    {
        let p = g.StartProgress(Context::new(), "cmd", 10, true);
        p.Inc();
        p.IncBy(2);
        // 1 + 2 = 3。
        assert_eq!(p.GetCurrent(), 3);
        p.Close();
    }

    // Record 为空操作，不应 panic。
    // Record is a no-op (does not panic)
    g.Record("unit", 1);
}
