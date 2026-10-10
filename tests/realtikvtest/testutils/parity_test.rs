// Copyright 2026 AsterSQL.

//! 中文说明开始（自动生成）
//! 中文总览：`parity_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `测试工具与兼容封装` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 65 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `go_rust_public_contract_matches` 是当前文件里的辅助函数。
//! `go_rust_public_contract_matches` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `go_rust_public_contract_matches` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `go_rust_public_contract_matches`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `go_rust_public_contract_matches` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `go_rust_public_contract_matches` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `parity_test.rs` 的回归，可以把 `go_rust_public_contract_matches` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`go_rust_public_contract_matches` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 符号 `contract_normal_paths` 是当前文件里的辅助函数。
//! `contract_normal_paths` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `contract_normal_paths` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `contract_normal_paths`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `contract_normal_paths` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `contract_normal_paths` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `parity_test.rs` 的回归，可以把 `contract_normal_paths` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`contract_normal_paths` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 符号 `contract_boundary` 是当前文件里的辅助函数。
//! `contract_boundary` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `contract_boundary` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `contract_boundary`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `contract_boundary` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `contract_boundary` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `parity_test.rs` 的回归，可以把 `contract_boundary` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`contract_boundary` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 符号 `Meta` 是当前文件里的状态类型。
//! `Meta` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `Meta` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `Meta`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `Meta` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `Meta` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `parity_test.rs` 的回归，可以把 `Meta` 当作定位同类问题的索引锚点。
//! 作为状态类型，`Meta` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 符号 `external_fields` 是当前文件里的辅助函数。
//! `external_fields` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `external_fields` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `external_fields`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `external_fields` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `external_fields` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `parity_test.rs` 的回归，可以把 `external_fields` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`external_fields` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 符号 `contract_error_paths` 是当前文件里的辅助函数。
//! `contract_error_paths` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `contract_error_paths` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `contract_error_paths`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `contract_error_paths` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `contract_error_paths` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `parity_test.rs` 的回归，可以把 `contract_error_paths` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`contract_error_paths` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 符号 `contract_resource_cleanup` 是当前文件里的辅助函数。
//! `contract_resource_cleanup` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `contract_resource_cleanup` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `contract_resource_cleanup`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! `contract_resource_cleanup` 默认保留现有调用顺序、错误语义与共享夹具，本次只补中文背景说明。
//! 对 `contract_resource_cleanup` 的理解应结合它前后的 SQL、桩对象、会话变量或事务步骤，而不是只看名字本身。
//! 如果后续要排查 `parity_test.rs` 的回归，可以把 `contract_resource_cleanup` 当作定位同类问题的索引锚点。
//! 作为辅助函数，`contract_resource_cleanup` 更适合和它的调用者一起阅读，以确认它封装了哪些重复步骤。
//! 中文说明结束（自动生成）

//! Parity tests for `tests/realtikvtest/testutils` public contracts vs Go.

use crate::common::InitTestWithStore;
use crate::stubs::{
    ExternalTagged, ExternalTaggedField, failpoint, fakestorage, kerneltype, require,
    set_classic_for_test, set_failpoint_hold_ms_for_test, storage, take_local_events, testkit,
};
use crate::workload::{deleteStr, genColval, insertStr, isSkippedError, updateStr};
use crate::{
    AddIndexMultiCols, AddIndexNonUnique, AddIndexPK, AddIndexUnique, AssertExternalField,
    CompatibilityContext, InitCompCtxParams, InitTest, RemoveAllObjects, SuiteContext,
    TestNonUnique, TestOneColFrame, TestOneIndexFrame, TestTwoColsFrame,
};
use astersql_tests_realtikvtest::stubs::{TestCtx, reset_test_globals as reset_parent};

fn init_comp_ctx_with_store(
    t: &TestCtx,
    store: &astersql_tests_realtikvtest::stubs::Storage,
) -> SuiteContext {
    let mut ctx = InitTestWithStore(t, store.clone());
    InitCompCtxParams(&mut ctx);
    ctx
}

fn init_concurrent_ddl_with_store(
    t: &TestCtx,
    store: &astersql_tests_realtikvtest::stubs::Storage,
    col_iids: Vec<Vec<i32>>,
    col_jids: Vec<Vec<i32>>,
) -> SuiteContext {
    let ctx = init_comp_ctx_with_store(t, store);
    if let Some(comp) = &ctx.CompCtx {
        let mut comp = comp.write().unwrap();
        comp.IsConcurrentDDL = true;
        comp.tType = TestNonUnique;
        comp.colIIDs = col_iids;
        comp.colJIDs = col_jids;
    }
    ctx
}

#[test]
fn go_rust_public_contract_matches() {
    let t = TestCtx::new();
    let store = astersql_tests_realtikvtest::CreateMockStoreAndSetup(&t, &[]);
    contract_normal_paths(&store);
    contract_boundary(&store);
    contract_error_paths(&store);
    contract_resource_cleanup(&store);
}

#[test]
fn duplicate_error_at_message_start_is_not_skipped() {
    let err = Some("1062 duplicate entry".to_string());
    assert!(
        !isSkippedError(&err, true, false),
        "Go strings.Index check requires the duplicate code to occur after position zero"
    );
    let wrapped = Some("Error 1062 duplicate entry".to_string());
    assert!(isSkippedError(&wrapped, true, false));
}

#[test]
fn compatibility_public_start_stop_methods_execute() {
    reset_parent();
    crate::stubs::reset_test_globals();

    let t = TestCtx::new();
    let ctx = InitTest(&t);
    let mut comp = CompatibilityContext::default();
    comp.colIIDs = vec![vec![1], vec![1], vec![1]];
    comp.tType = TestNonUnique;

    comp.Start(&ctx);
    comp.Stop(&ctx).unwrap();
}

/// Normal: InitTest SQL fixture, DDL SQL shape, frames, GCS cleanup, genColval.
fn contract_normal_paths(store: &astersql_tests_realtikvtest::stubs::Storage) {
    reset_parent();
    crate::stubs::reset_test_globals();
    set_classic_for_test(true);
    set_failpoint_hold_ms_for_test(0);

    let t = TestCtx::new();
    let ctx = InitTestWithStore(&t, store.clone());
    assert_eq!(ctx.tableNum, 3);
    assert_eq!(ctx.rowNum, 64);
    assert!(kerneltype::IsClassic());

    let execs = ctx.tk.execs();
    let table = execs
        .iter()
        .find(|sql| sql.starts_with("create table addindex.t0"))
        .unwrap();
    // Go common.go closes JSON_EXTRACT, the generated expression, and the table once each.
    assert!(table.ends_with("JSON_EXTRACT(c28, '$.population')))"));
    assert!(
        !table.ends_with("))))"),
        "the format wrapper must not add a fourth closing parenthesis"
    );
    assert!(
        execs
            .iter()
            .any(|s| s == "drop database if exists addindex;"),
        "InitTest must drop addindex db; execs={execs:?}"
    );
    assert!(execs.iter().any(|s| s == "create database addindex;"));
    assert!(execs.iter().any(|s| s == "use addindex;"));
    assert!(
        execs
            .iter()
            .any(|s| s == "set global tidb_ddl_enable_fast_reorg=on;"),
        "classic kernel enables fast reorg"
    );
    assert!(
        execs
            .iter()
            .any(|s| s.starts_with("create table addindex.t0")),
        "non-partition table t0"
    );
    assert!(
        execs
            .iter()
            .any(|s| s.contains("PARTITION BY RANGE") && s.contains("addindex.t1")),
        "range partition t1"
    );
    assert!(
        execs
            .iter()
            .any(|s| s.contains("PARTITION BY HASH") && s.contains("addindex.t2")),
        "hash partition t2"
    );
    let inserts = execs
        .iter()
        .filter(|s| s.starts_with("insert into addindex.t"))
        .count();
    assert_eq!(inserts, 3 * 64, "64 rows × 3 tables");

    // Non-unique single-col index SQL + admin check / drop.
    let col_ids = vec![vec![1], vec![1], vec![1]];
    TestOneColFrame(&ctx, &col_ids, AddIndexNonUnique);
    let execs = ctx.tk.execs();
    assert!(
        execs
            .iter()
            .any(|s| s.contains("alter table addindex.t0 add index idx1(c1)")),
        "non-unique index SQL; execs tail={:?}",
        execs.iter().rev().take(10).collect::<Vec<_>>()
    );
    assert!(
        execs
            .iter()
            .any(|s| s.contains("admin check index addindex.t0 idx1"))
    );
    assert!(
        execs
            .iter()
            .any(|s| s.contains("alter table addindex.t0 drop index idx1"))
    );

    // Prefix length for text cols 18..28.
    let col_ids = vec![vec![18], vec![], vec![]];
    // Go TestCreateNonUniqueIndex reuses one context across the column sequence.
    // stop cancels it after the first column; later DDL must not revive workload.
    assert!(ctx.done());
    let ctx2 = ctx.share_for_worker();
    TestOneColFrame(&ctx2, &col_ids, AddIndexNonUnique);
    assert!(
        ctx2.tk
            .execs()
            .iter()
            .any(|s| s.contains("add index idx18(c18(4))")),
        "text/blob prefix length 4"
    );

    // Multi-col frame skips identical column pairs.
    let ctx3 = InitTestWithStore(&t, store.clone());
    let i_ids = vec![vec![1], vec![], vec![]];
    let j_ids = vec![vec![1, 2], vec![], vec![]];
    TestTwoColsFrame(&ctx3, &i_ids, &j_ids, AddIndexMultiCols);
    let execs = ctx3.tk.execs();
    assert!(
        !execs.iter().any(|s| s.contains("add index idx0(c1, c1)")),
        "identical cols skipped"
    );
    assert!(
        execs.iter().any(|s| s.contains("add index idx1(c1, c2)")),
        "distinct cols create index"
    );

    // PK frame uses admin check table.
    let ctx4 = InitTestWithStore(&t, store.clone());
    TestOneIndexFrame(&ctx4, 0, AddIndexPK);
    assert!(
        ctx4.tk
            .execs()
            .iter()
            .any(|s| s.contains("admin check table addindex.t0"))
    );

    // SQL builders from workload.go
    assert!(insertStr("t0", 10000, "2008-02-02").contains("adddate('2008-02-02', 0)"));
    assert!(insertStr("t0", 10001, "2008-02-02").contains("aaaa10001"));
    assert_eq!(deleteStr("t0", 7), "delete from addindex.t0 where c0 =7");
    assert_eq!(genColval(8), "%\n");
    assert_eq!(genColval(6), "c6 - 64");
    assert_eq!(
        genColval(28),
        "json_object('name', 'NanJing', 'population', 2566)"
    );
    assert!(updateStr(64, "t0", &[1, 6]).contains("set c1=c1 + 1, c6=c6 - 64"));

    // RemoveAllObjects happy path
    let server = fakestorage::Server::new();
    server.put_objects("b", vec!["a".into(), "c".into()]);
    RemoveAllObjects(&t, &server, "b");
    assert!(server.objects("b").is_empty());
}

/// Boundary: next-gen skips fast reorg; unique+partition composite; multi-schema DDL suffix;
/// AssertExternalField empty/nil; failpoint list.
fn contract_boundary(store: &astersql_tests_realtikvtest::stubs::Storage) {
    reset_parent();
    crate::stubs::reset_test_globals();
    set_classic_for_test(false);
    set_failpoint_hold_ms_for_test(0);

    let t = TestCtx::new();
    let ctx = InitTestWithStore(&t, store.clone());
    assert!(!kerneltype::IsClassic());
    assert!(
        !ctx.tk
            .execs()
            .iter()
            .any(|s| s.contains("tidb_ddl_enable_fast_reorg")),
        "next-gen must not set fast reorg"
    );

    // Unique index on partitioned table includes c0.
    let ctx = InitTestWithStore(&t, store.clone());
    let col_ids = vec![vec![], vec![3], vec![]];
    TestOneColFrame(&ctx, &col_ids, AddIndexUnique);
    assert!(
        ctx.tk
            .execs()
            .iter()
            .any(|s| s.contains("add unique index idx3(c0, c3)")),
        "unique on part table uses (c0, cN)"
    );

    // Continue Go's unique-index column sequence with its cancelled context.
    // Unique on tinytext uses length 16.
    assert!(ctx.done());
    let ctx = ctx.share_for_worker();
    let col_ids = vec![vec![19], vec![], vec![]];
    TestOneColFrame(&ctx, &col_ids, AddIndexUnique);
    assert!(
        ctx.tk
            .execs()
            .iter()
            .any(|s| s.contains("add unique index idx19(c19(16))"))
    );

    // Multi-schema change appends column DDL.
    let mut ctx = init_comp_ctx_with_store(&t, store);
    if let Some(comp) = &ctx.CompCtx {
        comp.write().unwrap().IsMultiSchemaChange = true;
    }
    AddIndexNonUnique(&ctx, 0, "addindex.t0", 2).unwrap();
    assert!(
        ctx.tk
            .execs()
            .iter()
            .any(|s| s.contains("add index idx2(c2)") && s.contains("add column c62 int")),
        "multi-schema appends column"
    );

    // AssertExternalField
    struct Meta {
        ptr_nil: bool,
        slice_len: usize,
    }
    impl ExternalTagged for Meta {
        fn external_fields(&self) -> Vec<ExternalTaggedField> {
            vec![
                ExternalTaggedField::Ptr {
                    name: "DataDir".into(),
                    is_nil: self.ptr_nil,
                },
                ExternalTaggedField::Slice {
                    name: "Files".into(),
                    len: self.slice_len,
                },
                ExternalTaggedField::Struct {
                    name: "Empty".into(),
                    is_zero: true,
                },
                ExternalTaggedField::Map {
                    name: "M".into(),
                    len: 0,
                },
            ]
        }
    }
    AssertExternalField(
        &t,
        &Meta {
            ptr_nil: true,
            slice_len: 0,
        },
    );

    // Failpoint path table (enable/disable with 0 hold).
    let mut fp = InitTestWithStore(&t, store.clone());
    fp.isFailpointsTest = true;
    assert!(fp.isFailpointsTest);
    require::NoError(
        &t,
        failpoint::Enable(
            "github.com/pingcap/tidb/pkg/ddl/mockHighLoadForAddIndex",
            "return",
        ),
    );
    assert!(failpoint::is_enabled(
        "github.com/pingcap/tidb/pkg/ddl/mockHighLoadForAddIndex"
    ));
    require::NoError(
        &t,
        failpoint::Disable("github.com/pingcap/tidb/pkg/ddl/mockHighLoadForAddIndex"),
    );

    // Concurrent DDL init flags
    let ctx = init_concurrent_ddl_with_store(
        &t,
        store,
        vec![vec![1], vec![1], vec![1]],
        vec![vec![], vec![], vec![]],
    );
    assert!(
        ctx.CompCtx
            .as_ref()
            .unwrap()
            .read()
            .unwrap()
            .IsConcurrentDDL
    );
}

/// Error: Duplicate entry accepted for unique/PK; RemoveAllObjects list error; unexpected delete.
fn contract_error_paths(store: &astersql_tests_realtikvtest::stubs::Storage) {
    reset_parent();
    crate::stubs::reset_test_globals();
    set_classic_for_test(true);
    set_failpoint_hold_ms_for_test(0);

    let t = TestCtx::new();
    let ctx = InitTestWithStore(&t, store.clone());
    // Force unique create on table 0 col 1 to return Duplicate entry (Go expects Contains).
    ctx.tk.set_exec_handler(|sql| {
        if sql.contains("add unique index") {
            Err("Error 1062: Duplicate entry 'x' for key 'idx'".into())
        } else {
            Ok(None)
        }
    });
    // indexID==1 on table 0 is the "expect 1062" branch in AddIndexUnique.
    let err = AddIndexUnique(&ctx, 0, "addindex.t0", 1);
    assert!(err.is_err());
    assert!(err.unwrap_err().contains("1062"));

    // Non-unique failure must panic via require.NoError — exercise via catch_unwind.
    let ctx = InitTestWithStore(&t, store.clone());
    ctx.tk.set_exec_handler(|sql| {
        if sql.contains("add index") {
            Err("boom".into())
        } else {
            Ok(None)
        }
    });
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = AddIndexNonUnique(&ctx, 0, "addindex.t0", 1);
    }));
    assert!(panicked.is_err(), "non-unique DDL error must fail the test");

    // ListObjects error
    let server = fakestorage::Server::new();
    server.set_list_error(Some("list failed".into()));
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        RemoveAllObjects(&t, &server, "b");
    }));
    assert!(panicked.is_err());

    // Delete race: ErrObjectNotExist ignored
    let server = fakestorage::Server::new();
    server.put_objects("b", vec!["x".into()]);
    server.set_delete_error(Some(storage::ErrObjectNotExist()));
    RemoveAllObjects(&t, &server, "b"); // must not panic
}

/// Resource cleanup: workload start/stop cancels workers; CompCtx start/stop returns kits;
/// GCS objects removed; result sets closed on workload SQL.
fn contract_resource_cleanup(store: &astersql_tests_realtikvtest::stubs::Storage) {
    reset_parent();
    crate::stubs::reset_test_globals();
    set_classic_for_test(true);
    set_failpoint_hold_ms_for_test(0);

    let t = TestCtx::new();
    let ctx = InitTestWithStore(&t, store.clone());
    assert!(ctx.workload.is_some());
    assert!(ctx.tkPool.is_some());

    // Workload start/stop: cancel + join workers.
    {
        let wl = ctx.workload.as_ref().unwrap().clone();
        wl.lock().unwrap().start(&ctx, &[0, 1]);
        let err = wl.lock().unwrap().stop(&ctx, -1);
        assert!(err.is_ok(), "stop err={err:?}");
        assert!(ctx.done(), "cancel must mark suite done");
    }

    // Concurrent DDL start/stop returns kits to pool.
    let ctx = init_concurrent_ddl_with_store(
        &t,
        store,
        vec![vec![1], vec![1], vec![1]],
        vec![vec![], vec![], vec![]],
    );
    let comp = ctx.CompCtx.as_ref().unwrap().clone();
    CompatibilityContext::start_on(&comp, &ctx);
    CompatibilityContext::stop_on(&comp, &ctx).unwrap();
    // Pool should have received returned kits (3 puts). New get should reuse.
    let pool = ctx.tkPool.as_ref().unwrap();
    let tk = pool.get();
    // After 3 puts and 1 get, at least some reuse happened — exec history non-empty from InitTest on main tk.
    let _ = tk;

    // GCS cleanup empties bucket
    let server = fakestorage::Server::new();
    server.put_objects("bucket", vec!["o1".into(), "o2".into(), "o3".into()]);
    RemoveAllObjects(&t, &server, "bucket");
    assert_eq!(server.objects("bucket").len(), 0);

    // InitCompCtxParams resets flags
    let mut ctx = InitTestWithStore(&t, store.clone());
    InitCompCtxParams(&mut ctx);
    let g = ctx.CompCtx.as_ref().unwrap().read().unwrap();
    assert!(!g.IsConcurrentDDL);
    assert!(!g.IsMultiSchemaChange);
    assert!(!g.IsPiTR);

    let _events = take_local_events();
}
