// Copyright 2026 AsterSQL.

//! Parity tests for `br/pkg/mock` public contracts vs Go.
//!
//! 对照 Go `br/pkg/mock` 公开契约的集成式 parity 测试：在跳过真实 sleep、
//! 可注入 SQL/HTTP 钩子的桩环境下，验证 MockBackend / TaskRegister / Encoder /
//! ImportKV / Cluster 等构造器与 EXPECT/Call 语义是否与 Go mockgen + 测试集群一致。
//! 只断言形状、返回值传播与资源关闭副作用，不启动真实 TiDB/PD。
//!
//! 设计约束：
//! - 全局钩子（retry/sql/http/sleep）必须成对 reset，防止跨测试泄漏。
//! - EXPECT 参数用 `&()` 占位匹配任意实参，关注返回值契约而非参数深比较。
//! - panic 边界用 `catch_unwind` 捕获，对齐 Go `log.Panic` 而非 Result。

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::stubs::{
    self, CallOption, CheckCtx, CleanupEngineRequest, CloseEngineRequest, Context, Controller,
    DBInfo, Datum, EncodingConfig, EngineConfig, Error, GetVersionRequest, ImportEngineRequest,
    KVChecksum, LocalWriterConfig, OpenEngineRequest, RowHandle, RowsHandle,
    SimpleChunkFlushStatus, SwitchModeRequest, TableInfo, UUID, WriteEngineRequest,
    WriteEngineResponse,
};
use crate::{
    NewCluster, NewMockBackend, NewMockChunkFlushStatus, NewMockEncoder, NewMockEncodingBuilder,
    NewMockEngineWriter, NewMockImportKV_WriteEngineClient, NewMockImportKVClient, NewMockRow,
    NewMockRows, NewMockTargetInfoGetter, NewMockTaskRegister, getDSN, waitUntilServerOnline,
};

/// 主契约测试：覆盖 mock 成功路径、错误传播、边界类型与 Cluster 生命周期。
/// 各段落相互独立地使用 Controller；末尾统一 reset_test_hooks 避免污染后续用例。
#[test]
fn go_rust_public_contract_matches() {
    // 复位钩子并跳过 sleep，使 waitUntilServerOnline 重试可在单测时限内完成。
    stubs::reset_test_hooks();
    stubs::set_skip_sleep(true);

    // --- normal: Backend EXPECT/Call returns configured values ---
    // 正常路径：Backend 各方法按 EXPECT 录制返回成功/常量，调用后 remaining 归零。
    let ctrl = Controller::new();
    let backend = NewMockBackend(ctrl.clone());
    // ISGOMOCK 标记与 Go 侧 gomock 身份探测对齐，供调用方分支识别。
    backend.ISGOMOCK();
    // OpenEngine/CleanupEngine：ReturnError(None) 表示成功；Close 返回空错误切片。
    backend.EXPECT().OpenEngine(&(), &(), &()).ReturnError(None);
    backend.EXPECT().CleanupEngine(&(), &()).ReturnError(None);
    // ShouldPostProcess / RetryImportDelay 为无参查询，校验标量返回绑定。
    backend.EXPECT().ShouldPostProcess().Return1(true);
    backend
        .EXPECT()
        .RetryImportDelay()
        .Return1(Duration::from_millis(42));
    backend.EXPECT().Close().Return(vec![]);

    // 按录制顺序消费期望：成功打开/清理引擎，并读出后处理与重试延迟配置。
    assert!(
        backend
            .OpenEngine(Context::background(), EngineConfig::default(), UUID::new())
            .is_ok()
    );
    assert!(
        backend
            .CleanupEngine(Context::background(), UUID::new())
            .is_ok()
    );
    // 42ms 为任意非零探针值，证明 Duration 未在桩层被截断或单位错误。
    assert!(backend.ShouldPostProcess());
    assert_eq!(backend.RetryImportDelay(), Duration::from_millis(42));
    backend.Close();
    // remaining==0 表示所有 EXPECT 均已匹配，无遗漏调用。
    assert_eq!(ctrl.remaining(), 0);

    // --- error: TaskRegister propagates ReturnError ---
    // 错误路径：RegisterTask 必须把 ReturnError 原样表面化为 Result::Err。
    let ctrl2 = Controller::new();
    let reg = NewMockTaskRegister(ctrl2);
    reg.EXPECT()
        .RegisterTask(&())
        .ReturnError(Some(Error::new("register failed")));
    let err = reg
        .RegisterTask(Context::background())
        .expect_err("must fail");
    assert!(err.msg.contains("register failed"));

    // --- boundary: ChunkFlushStatus / Encode / Rows / Row ---
    // 边界：编码与行批处理相关 mock 的返回句柄 id/size 及副作用调用形状。
    let ctrl3 = Controller::new();
    // Flushed 布尔探针：确认 trait 对象方法经 mock 转发。
    let flush = NewMockChunkFlushStatus(ctrl3.clone());
    flush.EXPECT().Flushed().Return1(true);
    assert!(flush.Flushed());

    // Encode 返回 (RowHandle, Option<Error>) 二元组；此处验证成功分支字段。
    // id/size 使用非零常量，排除 Default 误返回。
    let enc = NewMockEncoder(ctrl3.clone());
    enc.EXPECT()
        .Encode(&(), &(), &(), &())
        .Return2(RowHandle { id: 7, size: 11 }, None);
    let row = enc.Encode(vec![Datum::default()], 1, vec![0], 0).unwrap();
    assert_eq!(row.id, 7);
    assert_eq!(row.size, 11);
    // Close 录制空错误列表，与 Backend.Close 形状一致。
    enc.EXPECT().Close().Return(vec![]);
    enc.Close();

    // EncodingBuilder / Rows 返回新句柄，模拟 Go 侧“清空后换批”语义。
    let builder = NewMockEncodingBuilder(ctrl3.clone());
    builder
        .EXPECT()
        .MakeEmptyRows()
        .Return1(RowsHandle { id: 3 });
    assert_eq!(builder.MakeEmptyRows().id, 3);

    // Clear 后句柄 id 变化，证明返回值来自 EXPECT 而非 self。
    let rows = NewMockRows(ctrl3.clone());
    rows.EXPECT().Clear().Return1(RowsHandle { id: 9 });
    assert_eq!(rows.Clear().id, 9);

    // Row.ClassifyAndAppend 以四个可变引用为出参；此处只验证 Size 与调用可达。
    // 出参容器用 default 占位，不检查分类结果内容（桩层未实现真实分类）。
    let mock_row = NewMockRow(ctrl3.clone());
    mock_row.EXPECT().Size().Return1(99u64);
    mock_row
        .EXPECT()
        .ClassifyAndAppend(&(), &(), &(), &())
        .Return(vec![]);
    assert_eq!(mock_row.Size(), 99);
    let mut a = RowsHandle::default();
    let mut b = KVChecksum::default();
    let mut c = RowsHandle::default();
    let mut d = KVChecksum::default();
    mock_row.ClassifyAndAppend(&mut a, &mut b, &mut c, &mut d);

    // EngineWriter Close returns ChunkFlushStatus
    // Close 成功时返回已刷新的 ChunkFlushStatus 动态对象，与 Go 接口一致。
    // IsSynced=false 再 Close，覆盖“未同步但可关闭并报告已刷盘”的组合。
    let ew = NewMockEngineWriter(ctrl3.clone());
    ew.EXPECT().IsSynced().Return1(false);
    assert!(!ew.IsSynced());
    ew.EXPECT().Close(&()).Return2(
        Box::new(SimpleChunkFlushStatus { flushed: true }) as Box<dyn stubs::ChunkFlushStatus>,
        None,
    );
    let status = ew.Close(Context::background()).unwrap();
    assert!(status.Flushed());

    // TargetInfoGetter map/list shapes
    // 远端元信息：库列表与表 map 形状、CheckRequirements 成功路径。
    // HashMap 键为表名字符串，对齐 Go map[string]*model.TableInfo 的测试用法。
    let getter = NewMockTargetInfoGetter(ctrl3.clone());
    getter
        .EXPECT()
        .CheckRequirements(&(), &())
        .ReturnError(None);
    getter.EXPECT().FetchRemoteDBModels(&()).Return2(
        vec![DBInfo {
            name: "test".into(),
        }],
        None,
    );
    let mut tables: HashMap<String, TableInfo> = HashMap::new();
    tables.insert("t".into(), TableInfo { name: "t".into() });
    getter
        .EXPECT()
        .FetchRemoteTableModels(&(), &(), &())
        .Return2(tables.clone(), None);
    // CheckRequirements 成功：调用方可继续拉库表模型。
    assert!(
        getter
            .CheckRequirements(Context::background(), CheckCtx::default())
            .is_ok()
    );
    assert_eq!(
        getter.FetchRemoteDBModels(Context::background()).unwrap()[0].name,
        "test"
    );
    // 按表名索引取出模型，确认 map 往返未丢键。
    assert_eq!(
        getter
            .FetchRemoteTableModels(Context::background(), "db".into(), vec!["t".into()])
            .unwrap()
            .get("t")
            .unwrap()
            .name,
        "t"
    );

    // --- importer unary + stream ---
    // ImportKV：一元 RPC 成功/失败与双向流 WriteEngine 的 Send/CloseAndRecv。
    // CallOption 切片可为空或含 default，桩层不解释 gRPC 选项内容。
    let ctrl4 = Controller::new();
    let imp = NewMockImportKVClient(ctrl4.clone());
    imp.EXPECT()
        .OpenEngine(&(), &(), &[])
        .Return2(stubs::OpenEngineResponse::default(), None);
    // GetVersion 回传版本字符串，供上层兼容性判断。
    imp.EXPECT().GetVersion(&(), &(), &[]).Return2(
        stubs::GetVersionResponse {
            version: "v1".into(),
        },
        None,
    );
    // ImportEngine 故意注入错误，确认 unary 错误路径消息保留。
    imp.EXPECT()
        .ImportEngine(&(), &(), &[])
        .ReturnError(Some(Error::new("import boom")));
    assert!(
        imp.OpenEngine(
            Context::background(),
            OpenEngineRequest::default(),
            vec![CallOption::default()]
        )
        .is_ok()
    );
    assert_eq!(
        imp.GetVersion(Context::background(), GetVersionRequest::default(), vec![])
            .unwrap()
            .version,
        "v1"
    );
    // unwrap_err + contains：错误文本不得被桩层吞掉或改写。
    assert!(
        imp.ImportEngine(
            Context::background(),
            ImportEngineRequest::default(),
            vec![]
        )
        .unwrap_err()
        .msg
        .contains("import boom")
    );

    // 流式客户端：先 Send 再 CloseAndRecv，空 error 表示写入侧成功收尾。
    // 与 Go ImportKV_WriteEngineClient 的双向流收尾顺序一致。
    let stream = NewMockImportKV_WriteEngineClient(ctrl4.clone());
    stream.EXPECT().Send(&()).ReturnError(None);
    stream.EXPECT().CloseAndRecv().Return2(
        WriteEngineResponse {
            error: String::new(),
        },
        None,
    );
    assert!(stream.Send(&WriteEngineRequest::default()).is_ok());
    assert!(stream.CloseAndRecv().unwrap().error.is_empty());

    // --- getDSN / SplitAfter shape ---
    // getDSN 应用配置闭包后应生成 MySQL DSN；前缀切分逻辑与 waitUntilServerOnline 一致。
    let dsn = getDSN(vec![Some(Box::new(|cfg: &mut stubs::MysqlConfig| {
        cfg.Addr = "127.0.0.1:4000".into();
    }))]);
    assert!(dsn.contains("root@tcp(127.0.0.1:4000)/"));
    let prefix = {
        // same as waitUntilServerOnline return shape
        // 取首个 '/' 及之前作为“可再拼库名”的 DSN 前缀。
        match dsn.find('/') {
            Some(i) => &dsn[..=i],
            None => dsn.as_str(),
        }
    };
    assert_eq!(prefix, "root@tcp(127.0.0.1:4000)/");

    // --- waitUntilServerOnline retries then succeeds ---
    // 注入 SQL：前两次失败、第三次成功；HTTP 始终成功，验证重试计数 ≥3。
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts2 = Arc::clone(&attempts);
    stubs::set_retry_time(5);
    stubs::set_cluster_online(false);
    stubs::set_sql_open(Some(Arc::new(move |_dsn| {
        let n = attempts2.fetch_add(1, Ordering::SeqCst);
        if n < 2 {
            Err(Error::new("not ready"))
        } else {
            Ok(())
        }
    })));
    stubs::set_http_get(Some(Arc::new(|_url| Ok(b"ok".to_vec()))));
    let online_dsn = waitUntilServerOnline("127.0.0.1:5555", 9999);
    // 成功返回值必须以 '/' 结尾，供调用方拼接数据库名。
    assert!(online_dsn.ends_with('/'));
    assert!(attempts.load(Ordering::SeqCst) >= 3);

    // --- error boundary: exhausted retries panics (Go log.Panic) ---
    // 与 Go log.Panic 对齐：SQL 始终失败且耗尽重试时应 unwind panic，而非返回 Err。
    stubs::set_sql_open(Some(Arc::new(|_dsn| Err(Error::new("always fail")))));
    stubs::set_retry_time(2);
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let _ = waitUntilServerOnline("127.0.0.1", 1);
    }));
    assert!(panicked.is_err(), "exhausted SQL retries must panic");

    // --- resource: Cluster New/Start/Stop closes Domain/Storage/Server ---
    // 资源生命周期：NewCluster 引导单 store；Start 填 DSN/Server；Stop 关闭三件套。
    stubs::reset_test_hooks();
    stubs::set_skip_sleep(true);
    // Start() 在 wait 前会置 online；此处先 false 以走完整启动路径。
    stubs::set_cluster_online(false); // Start() sets online before wait
    let mut cluster = NewCluster().expect("NewCluster");
    assert!(cluster.Storage.is_some());
    assert!(cluster.Domain.is_some());
    assert!(cluster.PDClient.is_some());
    assert!(cluster.PDHTTPCli.is_some());
    // BootstrapWithSingleStore 必须在 New 阶段完成，stores==1。
    assert!(
        cluster.Cluster.as_ref().unwrap().bootstrapped,
        "BootstrapWithSingleStore must run"
    );
    assert_eq!(cluster.Cluster.as_ref().unwrap().stores, 1);

    cluster.Start().expect("Start");
    assert!(!cluster.DSN.is_empty());
    assert!(cluster.Server.is_some());
    assert!(cluster.TiDBDriver.is_some());

    // 在 Stop 前克隆 closed 标志，断言 Domain/Storage/Server 均被关闭。
    let domain_closed = cluster.Domain.as_ref().unwrap().closed.clone();
    let storage_closed = cluster.Storage.as_ref().unwrap().closed.clone();
    let server_closed = cluster.Server.as_ref().unwrap().closed.clone();
    cluster.Stop();
    assert!(domain_closed.load(Ordering::SeqCst));
    assert!(storage_closed.load(Ordering::SeqCst));
    assert!(server_closed.load(Ordering::SeqCst));

    // 清理全局钩子，避免影响同 crate 其他测试的默认行为。
    stubs::reset_test_hooks();
}

/// Go gomock does not impose registration order unless `gomock.InOrder` is used.
#[test]
fn gomock_expectations_match_out_of_registration_order() {
    let ctrl = Controller::new();
    let reg = NewMockTaskRegister(ctrl.clone());

    reg.EXPECT().Close(&()).ReturnError(None);
    reg.EXPECT().RegisterTask(&()).ReturnError(None);

    assert!(reg.RegisterTask(Context::background()).is_ok());
    assert!(reg.Close(Context::background()).is_ok());
    assert_eq!(ctrl.remaining(), 0);
}

/// Preserve the exact Go `for retry = range retryTime` terminal-value behavior.
#[test]
fn exhausted_http_retries_return_dsn_like_go_source() {
    stubs::reset_test_hooks();
    stubs::set_skip_sleep(true);
    stubs::set_retry_time(2);
    stubs::set_sql_open(Some(Arc::new(|_| Ok(()))));
    stubs::set_http_get(Some(Arc::new(|_| Err(Error::new("http offline")))));

    let result = catch_unwind(AssertUnwindSafe(|| {
        waitUntilServerOnline("127.0.0.1:4000", 10080)
    }));
    stubs::reset_test_hooks();

    assert_eq!(
        result.expect("Go source does not panic after the range loop exhausts"),
        "root@tcp(127.0.0.1:4000)/"
    );
}
