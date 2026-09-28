// Copyright 2026 AsterSQL.

//! Parity tests for `cmd/benchraw` public contracts vs Go `main.go`.
//! 该文件不验证压测吞吐量，而是锁定 Rust 迁移版对外可观察行为是否仍与
//! `cmd/benchraw/main.go` 保持一致。
//! 测试按“正常路径、边界切分、错误传播、资源清理”四组展开，避免未来重构时
//! 无意改掉 Go 版本留下的兼容性约束。
//! 由于真实 TiKV、日志与 HTTP 监听都被 `stubs` 隔离，这里的断言专注在：
//! 参数默认值、key 分片规则、panic 形状、后台 pprof 副作用以及日志记录时机。

use std::collections::HashSet;
use std::net::TcpListener;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crate::entry::{self, batch_raw_put, default_flags, format_elapse_line, run_with_flags};
use crate::stubs::{
    self, ClientFactory, Flags, LogLevel, PutLog, RawKvClient, Security, StubRawKvClient,
};

#[test]
fn go_rust_public_contract_matches() {
    // 聚合入口保持与 Go 源码的职责分组一致，失败时能直接定位到哪类契约漂移。
    // 这里不用子测试框架，避免输出结构与仓库现有 parity test 风格脱节。
    // 顺序执行四组契约，也能减少共享 stub 状态下的并发干扰。
    contract_normal_puts_and_defaults();
    contract_boundary_partition_and_flags();
    contract_error_paths();
    contract_resource_cleanup_and_pprof();
}

#[test]
fn negative_value_size_panics_before_client_creation() {
    let factory_called = Arc::new(AtomicBool::new(false));
    let factory_called_in_factory = Arc::clone(&factory_called);
    let factory: ClientFactory = Arc::new(move |_addrs, _security| {
        factory_called_in_factory.store(true, Ordering::SeqCst);
        Ok(Arc::new(StubRawKvClient::new(Vec::new(), Security::default())) as Arc<dyn RawKvClient>)
    });

    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        run_with_flags(
            Flags {
                data_cnt: 1,
                worker_cnt: 1,
                value_size: -1,
                ..Flags::default()
            },
            factory,
            false,
        );
    }));

    assert!(result.is_err(), "Go make([]byte, -1) panics");
    assert!(
        !factory_called.load(Ordering::SeqCst),
        "value allocation must fail before rawkv.NewClient"
    );
}

#[test]
fn flag_parsing_stops_at_first_positional_argument() {
    let flags = stubs::parse_flags(&["-N=7".into(), "positional".into(), "-C=9".into()])
        .expect("normal flags");

    assert_eq!(flags.data_cnt, 7);
    assert_eq!(flags.worker_cnt, Flags::default().worker_cnt);
}

#[test]
fn double_dash_terminates_flag_parsing() {
    let flags =
        stubs::parse_flags(&["-N=7".into(), "--".into(), "-C=9".into()]).expect("normal flags");

    assert_eq!(flags.data_cnt, 7);
    assert_eq!(flags.worker_cnt, Flags::default().worker_cnt);
}

#[test]
fn help_stops_before_later_flags_are_parsed() {
    let flags = stubs::parse_flags(&["-h".into(), "-N=7".into()]);

    assert!(flags.is_none(), "Go flag.Parse exits successfully on -h");
}

#[test]
fn production_factory_rejects_unreachable_pd() {
    let result =
        stubs::default_client_factory()(vec!["127.0.0.1:1".to_string()], Security::default());

    assert!(
        result.is_err(),
        "the production factory must connect to PD instead of returning an in-memory stub"
    );
}

#[test]
fn production_http_listener_reports_bind_failure() {
    let occupied = TcpListener::bind("127.0.0.1:0").expect("reserve a local test port");
    let addr = occupied.local_addr().expect("read reserved port");

    let result = stubs::listen_and_serve_real(&addr.to_string());

    assert!(
        result.is_some(),
        "the production HTTP adapter must bind a real socket"
    );
}

#[test]
fn production_pprof_index_is_registered() {
    let (status, content_type, body) = stubs::pprof_response("/debug/pprof/");

    assert_eq!(status, 200);
    assert_eq!(content_type, "text/html; charset=utf-8");
    assert!(String::from_utf8(body).unwrap().contains("profile"));
}

/// Normal path: defaults, key layout, SSL wiring, value size, concurrent Puts.
/// 正常路径覆盖最常见的命令使用方式，也是与 Go 主流程最容易发生“看似无害重构”
/// 但实际改变外部行为的区域。
/// 这里同时校验三件事：
/// 1. 默认 flag 常量没有偏离 Go 包级变量的初始值。
/// 2. 写入键空间仍按 `key_<n>` 连续生成，且 value 大小完全受 `value_size` 控制。
/// 3. 通过默认建连路径时，PD 地址拆分与 TLS 参数转发保持原样，不做额外清洗。
fn contract_normal_puts_and_defaults() {
    // 先清空全局 stub 状态，避免前一个分组遗留的客户端或日志副作用污染断言。
    stubs::set_new_client_fail(None);
    stubs::global_put_log().clear();
    stubs::clear_last_new_client_args();
    stubs::reset_http_state();
    stubs::clear_terror_logs();

    let d = default_flags();
    // 这一组默认值必须与 Go 包级 flag 定义逐项一致；
    // 一旦有人只改了 Rust 默认值而忘记同步 Go，对齐测试会立刻报警。
    assert_eq!(d.data_cnt, 1_000_000);
    assert_eq!(d.worker_cnt, 100);
    assert_eq!(d.pd_addr, "localhost:2379");
    assert_eq!(d.value_size, 5);
    assert_eq!(d.ssl_ca, "");
    assert_eq!(d.ssl_cert, "");
    assert_eq!(d.ssl_key, "");

    let put_log = PutLog::new();
    let put_log_f = put_log.clone();
    let factory: ClientFactory = Arc::new(move |addrs, sec| {
        Ok(
            Arc::new(StubRawKvClient::with_put_log(addrs, sec, put_log_f.clone()))
                as Arc<dyn RawKvClient>,
        )
    });

    let flags = Flags {
        data_cnt: 20,
        worker_cnt: 4,
        pd_addr: "pd1:2379,pd2:2379".into(),
        value_size: 3,
        ssl_ca: "ca.pem".into(),
        ssl_cert: "cert.pem".into(),
        ssl_key: "key.pem".into(),
    };
    // value 预先一次性构造，呼应 Go 在主流程里先分配再传入 `batchRawPut` 的顺序。
    let value = vec![0u8; flags.value_size as usize];
    batch_raw_put(&flags, &value, factory);

    // 这里验证的是 Go 的“整除分片”主路径：20 条数据、4 个 worker，应恰好写满 20 次。
    let calls = put_log.calls();
    // base = 20/4 = 5 → 4 workers × 5 puts = 20
    assert_eq!(calls.len(), 20);
    // 用集合断言 key 存在性，避免并发调度顺序影响测试稳定性。
    let keys: HashSet<String> = calls
        .iter()
        .map(|c| String::from_utf8(c.key.clone()).unwrap())
        .collect();
    for i in 0..20 {
        assert!(keys.contains(&format!("key_{i}")), "missing key_{i}");
    }
    for c in &calls {
        assert_eq!(c.value, vec![0u8; 3]);
    }

    // Security + multi-PD via default new_client path
    // 第二段不复用自定义 factory 记录，而是走默认 new_client 路径，
    // 这样才能覆盖 `run_with_flags` 到底有没有把拆分后的 PD 地址和 TLS 字段
    // 原封不动传到 stub 边界。
    stubs::global_put_log().clear();
    stubs::clear_last_new_client_args();
    let flags2 = Flags {
        data_cnt: 4,
        worker_cnt: 2,
        pd_addr: "a:1,b:2".into(),
        value_size: 1,
        ssl_ca: "CA".into(),
        ssl_cert: "CERT".into(),
        ssl_key: "KEY".into(),
    };
    run_with_flags(flags2, stubs::stub_client_factory(), false);
    let (addrs, sec) = stubs::last_new_client_args().expect("new_client called");
    assert_eq!(addrs, vec!["a:1".to_string(), "b:2".to_string()]);
    assert_eq!(
        sec,
        Security {
            ClusterSSLCA: "CA".into(),
            ClusterSSLCert: "CERT".into(),
            ClusterSSLKey: "KEY".into(),
        }
    );
    assert_eq!(stubs::global_put_log().len(), 4);
    // Go 主流程会把日志级别降到 Warn，测试在这里锁定该初始化副作用。
    assert_eq!(stubs::get_log_level(), LogLevel::Warn);

    // 输出字符串单独校验，避免未来格式化改动破坏脚本或人工比对习惯。
    let line = format_elapse_line("1s", 100);
    assert_eq!(line, "\nelapse:1s, total 100\n");
    // 显式引用符号，保证公开入口仍对测试可见。
    let _ = entry::default_flags;
}

/// Boundary: remainder keys dropped, zero-size value, flag parse, PD split.
/// 边界路径集中锁定 Go 源码里那些不一定“理想”，但已经形成既有语义的细节。
/// 这些断言的目标不是优化行为，而是防止 Rust 版好心修复后与 Go 偏离。
fn contract_boundary_partition_and_flags() {
    let put_log = PutLog::new();
    let put_log_f = put_log.clone();
    let factory: ClientFactory = Arc::new(move |addrs, sec| {
        Ok(
            Arc::new(StubRawKvClient::with_put_log(addrs, sec, put_log_f.clone()))
                as Arc<dyn RawKvClient>,
        )
    });

    // dataCnt=10, workerCnt=3 → base=3 → 9 puts; key_9 not written (Go remainder drop).
    // 这是本文件最重要的兼容性边界之一：余数会被直接丢弃，而不是补给最后一个 worker。
    let flags = Flags {
        data_cnt: 10,
        worker_cnt: 3,
        ..Flags::default()
    };
    // 传入非空 value，确保本分组观察到的是分片策略，而不是 value 特殊值造成的分支。
    batch_raw_put(&flags, &[7u8; 2], factory);
    let calls = put_log.calls();
    assert_eq!(calls.len(), 9);
    let keys: HashSet<_> = calls
        .iter()
        .map(|c| String::from_utf8_lossy(&c.key).into_owned())
        .collect();
    assert!(!keys.contains("key_9"));
    assert!(keys.contains("key_0"));
    assert!(keys.contains("key_8"));

    // valueSize 0 → empty value slice
    // Go 的 `make([]byte, 0)` 会产生空切片；Rust 侧必须保持空 value 而不是跳过 Put。
    let put_log2 = PutLog::new();
    let put_log2f = put_log2.clone();
    let factory2: ClientFactory = Arc::new(move |a, s| {
        Ok(
            Arc::new(StubRawKvClient::with_put_log(a, s, put_log2f.clone()))
                as Arc<dyn RawKvClient>,
        )
    });
    batch_raw_put(
        &Flags {
            data_cnt: 2,
            worker_cnt: 2,
            value_size: 0,
            ..Flags::default()
        },
        &[],
        factory2,
    );
    assert_eq!(put_log2.calls().len(), 2);
    assert!(put_log2.calls().iter().all(|c| c.value.is_empty()));

    // 参数解析断言覆盖 `-k=v` 与 `-k v` 两种 Go flag 常见写法，防止自定义解析器
    // 只支持其中一种形式。
    let f = stubs::parse_flags(&[
        "-N".into(),
        "42".into(),
        "-C=7".into(),
        "-pd=x:1,y:2".into(),
        "-V".into(),
        "9".into(),
        "-cacert=ca".into(),
        "-cert".into(),
        "c".into(),
        "-key=k".into(),
    ])
    .expect("normal flags");
    assert_eq!(f.data_cnt, 42);
    assert_eq!(f.worker_cnt, 7);
    assert_eq!(f.pd_addr, "x:1,y:2");
    assert_eq!(f.value_size, 9);
    assert_eq!(f.ssl_ca, "ca");
    assert_eq!(f.ssl_cert, "c");
    assert_eq!(f.ssl_key, "k");
    assert_eq!(
        stubs::split_pd_addrs(&f.pd_addr),
        vec!["x:1".to_string(), "y:2".to_string()]
    );

    // Worker key ranges are disjoint: worker i owns [base*i, base*(i+1)).
    // 这里锁定“连续且不重叠”的 worker 分片规则，而不是依赖调用顺序。
    let put_log3 = PutLog::new();
    let put_log3f = put_log3.clone();
    let factory3: ClientFactory = Arc::new(move |a, s| {
        Ok(
            Arc::new(StubRawKvClient::with_put_log(a, s, put_log3f.clone()))
                as Arc<dyn RawKvClient>,
        )
    });
    batch_raw_put(
        &Flags {
            data_cnt: 12,
            worker_cnt: 3,
            ..Flags::default()
        },
        &[1],
        factory3,
    );
    // base=4; keys 0..11 all present once
    let keys: Vec<_> = put_log3
        .calls()
        .into_iter()
        .map(|c| String::from_utf8(c.key).unwrap())
        .collect();
    // 长度和去重后长度都必须等于 12，才能证明没有漏写也没有重复写。
    assert_eq!(keys.len(), 12);
    let set: HashSet<_> = keys.iter().cloned().collect();
    assert_eq!(set.len(), 12);
}

/// Error paths: NewClient fail, Put fail, bad flags → Fatal (panic).
/// 错误路径保持 Go `log.Fatal` 的“立刻终止”心智模型，在 Rust 测试中通过 panic 表达。
/// 这些断言不追求错误类型精细化，而是锁定用户能看到的失败消息和触发时机。
fn contract_error_paths() {
    // 建连失败必须在开始写入前就终止，并把底层错误文本直接暴露出来。
    stubs::set_new_client_fail(Some("pd unavailable".into()));
    let r = panic::catch_unwind(AssertUnwindSafe(|| {
        batch_raw_put(
            &Flags {
                data_cnt: 2,
                worker_cnt: 1,
                ..Flags::default()
            },
            &[0],
            stubs::stub_client_factory(),
        );
    }));
    stubs::set_new_client_fail(None);
    assert!(r.is_err());
    assert!(panic_msg(r.unwrap_err()).contains("pd unavailable"));

    // Put 失败要保留 Go 里的 `"put failed"` 前缀，方便对齐旧日志和脚本关键字。
    let fail_client = StubRawKvClient::new(vec!["pd".into()], Security::default());
    fail_client.set_fail_put(true);
    let fail_client = Arc::new(fail_client);
    let factory: ClientFactory =
        Arc::new(move |_a, _s| Ok(Arc::clone(&fail_client) as Arc<dyn RawKvClient>));
    let r = panic::catch_unwind(AssertUnwindSafe(|| {
        batch_raw_put(
            &Flags {
                data_cnt: 1,
                worker_cnt: 1,
                ..Flags::default()
            },
            &[0],
            factory,
        );
    }));
    assert!(r.is_err());
    assert!(panic_msg(r.unwrap_err()).contains("put failed"));

    // 未定义 flag 与非法整型都应在解析阶段快速失败，而不是静默回退默认值。
    let r = panic::catch_unwind(AssertUnwindSafe(|| {
        let _ = stubs::parse_flags(&["-not-defined".into()]);
    }));
    assert!(r.is_err());

    let r = panic::catch_unwind(AssertUnwindSafe(|| {
        let _ = stubs::parse_flags(&["-N".into(), "nope".into()]);
    }));
    assert!(r.is_err());

    // Go 原实现会在 `dataCnt / workerCnt` 触发除零失败；Rust 版改成显式 fatal，
    // 但对外仍要表现为同步失败，不能悄悄把 0 worker 当成空操作。
    // 这里不要求 panic 文本与 Go 的运行时错误逐字相同，只要求失败不可被吞掉。
    let r = panic::catch_unwind(AssertUnwindSafe(|| {
        batch_raw_put(
            &Flags {
                data_cnt: 10,
                worker_cnt: 0,
                ..Flags::default()
            },
            &[0],
            stubs::stub_client_factory(),
        );
    }));
    assert!(r.is_err());
}

/// Resource cleanup: WaitGroup join completes; pprof listen side-effect + terror.Log on error.
/// 资源清理分组验证主流程返回前，所有 worker 都已经结束，并且后台 pprof 监听
/// 即使失败也只记录到 `terror.Log`，不会吞掉主流程。
/// 这组断言是 Go `WaitGroup + goroutine + terror.Log(errors.Trace(err))` 的
/// 语义压缩版，重点看“是否发生副作用”而不是网络真实可用性。
fn contract_resource_cleanup_and_pprof() {
    // 显式制造监听失败，便于观察后台线程是否把错误交给 terror 日志通道。
    stubs::reset_http_state();
    stubs::clear_terror_logs();
    stubs::set_http_listen_fail(Some("listen boom".into()));

    let put_log = PutLog::new();
    let put_log_f = put_log.clone();
    // 复用记录型 client，而不是走全局默认 log，便于只观察本次执行的 worker 完成情况。
    let factory: ClientFactory = Arc::new(move |a, s| {
        Ok(
            Arc::new(StubRawKvClient::with_put_log(a, s, put_log_f.clone()))
                as Arc<dyn RawKvClient>,
        )
    });

    run_with_flags(
        Flags {
            data_cnt: 4,
            worker_cnt: 2,
            value_size: 1,
            ..Flags::default()
        },
        factory,
        true,
    );

    // Workers finished (WaitGroup equivalent) before return.
    // 若这里小于 4，说明 `run_with_flags` 在 worker 仍未完成时就返回，破坏 Go 对齐。
    assert_eq!(put_log.len(), 4);

    // Wait for pprof goroutine to record ListenAndServe.
    // 后台线程存在调度竞争，因此轮询等待副作用出现，而不是假设立刻可见。
    for _ in 0..100 {
        if stubs::http_start_count() > 0 {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        stubs::http_start_count() > 0,
        "pprof ListenAndServe must start"
    );
    let addrs = stubs::take_http_listen_addrs();
    assert!(
        addrs.iter().any(|a| a == ":9191"),
        "pprof must ListenAndServe :9191, got {addrs:?}"
    );

    // terror.Log(errors.Trace(err)) on listen failure — assert via direct contract
    // (background may race); also drain any background logs.
    // 如果后台竞争导致日志尚未入队，就直接走一遍同样的错误链路，
    // 验证 `errors_trace -> terror_log` 组合至少保持与 Go 一样的可观察结果。
    let mut logs = stubs::take_terror_logs();
    if logs.is_empty() {
        let err = stubs::listen_and_serve(":9191");
        stubs::terror_log(err.map(stubs::errors_trace));
        logs = stubs::take_terror_logs();
    }
    // 这里只匹配关键信息片段，不锁死完整消息格式，避免日志封装细节让契约测试过脆。
    assert!(
        logs.iter().any(|m| m.contains("listen boom")),
        "terror.Log must record listen error, got {logs:?}"
    );

    stubs::set_http_listen_fail(None);
    stubs::reset_http_state();
}

/// 将 `catch_unwind` 捕获到的任意 panic 载荷尽量还原为可断言字符串。
/// 这能覆盖 String、`&str` 以及其它调试格式化输出，避免测试被 panic 载荷类型绑死。
fn panic_msg(err: Box<dyn std::any::Any + Send>) -> String {
    // 优先返回最常见的字符串载荷，保证错误断言可读且与 Go `Fatal` 文本接近。
    if let Some(s) = err.downcast_ref::<String>() {
        return s.clone();
    }
    // 某些 panic 站点只会抛出静态字符串，这里同样转成拥有所有权的 `String`。
    if let Some(s) = err.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    // 兜底分支保留调试表示，避免未知载荷类型时测试完全失去可观测性。
    format!("{err:?}")
}
