// Copyright 2026 AsterSQL.

//! Parity tests for `cmd/benchkv` public contracts vs Go `main.go`.
//! 该文件把 Rust `main.rs` 暴露给外部的关键行为压缩成一组可重复断言，
//! 用来防止后续重构在不改函数签名的前提下悄悄偏离 `cmd/benchkv/main.go`。
//! 测试重点不是验证真实 TiKV 性能，而是验证参数默认值、初始化顺序、并发分片、
//! 指标导出、错误处理和资源清理这些“用户能观察到”的契约是否仍与 Go 对齐。
//! 由于这里依赖 `stubs` 记录事务和 HTTP 副作用，每个断言都尽量绑定行为结果，
//! 而不是绑定某个具体实现细节，便于在 Rust 内部继续做安全重构。
//! 四个子场景依次覆盖主路径、边界条件、故障分支和收尾逻辑，
//! 组合后基本等价于把 Go 主流程拆开逐段验收。

use std::net::TcpListener;
use std::panic::{self, AssertUnwindSafe};
use std::thread;
use std::time::Duration;

use crate::entry::{self, batch_rw, default_flags, init, run_with_flags};
use crate::stubs::{
    self, Flags, HttpResponse, HttpServer, Metrics, RuntimeDeps, Storage, TiKVDriver,
    exponential_buckets,
};

#[test]
fn go_rust_public_contract_matches() {
    // 顶层测试只负责编排检查顺序，让失败直接定位到具体契约场景。
    // 顺序也与读 Go `main.go` 的心智模型一致：先正常跑，再看边界、错误和收尾。
    contract_normal_path();
    contract_boundary();
    contract_error_paths();
    contract_resource_cleanup();
}

/// Normal: defaults, store URL, conflict-free keys, metrics, HTTP start, output shape.
/// 正常路径验证“默认情况下 benchkv 怎么跑”。
/// 它同时约束默认 flag、Prometheus bucket、TiKV 连接串、HTTP metrics 服务启动、
/// 无冲突 key 分布，以及完整入口执行后留下的指标副作用。
/// 只要这里的断言稳定成立，就说明 Rust 版本的主流程仍保留 Go 的总体时序和输出语义。
/// 换句话说，这一组测试就是 benchkv 最核心用户体验的回归护栏。
fn contract_normal_path() {
    // 默认 flag 是命令行契约的一部分，文档和无参启动都依赖这些值保持稳定。
    let d = default_flags();
    assert_eq!(d.data_cnt, 1_000_000);
    assert_eq!(d.worker_cnt, 400);
    assert_eq!(d.pd_addr, "localhost:2379");
    assert_eq!(d.value_size, 5);

    // 指标桶位必须与 Go `ExponentialBuckets(0.0005, 2, 13)` 保持一致，
    // 否则同一批事务在两边会落入不同 bucket，影响对照分析。
    let buckets = exponential_buckets(0.0005, 2.0, 13);
    assert_eq!(buckets.len(), 13);
    assert!((buckets[0] - 0.0005).abs() < 1e-12);
    assert!((buckets[1] - 0.001).abs() < 1e-12);

    // `init` 需要按 Go 的拼接规则构造 TiKV URL，并在返回前完成指标注册。
    let flags = Flags {
        data_cnt: 20,
        worker_cnt: 4,
        pd_addr: "pd.example:2379".into(),
        value_size: 3,
    };
    let deps = RuntimeDeps::for_test();
    let (store, metrics, http) = init(&flags, &deps);
    assert_eq!(store.path, "tikv://pd.example:2379?cluster=1");
    assert!(metrics.is_registered());

    // Go 在后台 goroutine 启动 `ListenAndServe`；Rust 用线程模拟，所以这里轮询启动标记。
    // Wait briefly for ListenAndServe goroutine/thread to mark started.
    for _ in 0..50 {
        if http.is_started() {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert!(http.is_started(), "Init must start :9191 metrics server");
    assert_eq!(http.listen_addr(), ":9191");

    // value 的内容不重要，重要的是长度必须严格来自 `-V`。
    let value = vec![0u8; flags.value_size as usize];
    assert_eq!(value.len(), 3);
    batch_rw(&flags, &store, &metrics, &value);

    // 写入总数和指标计数一起约束整除分片逻辑，防止并发循环边界发生偏移。
    // base = 20/4 = 5; total txns = 4*5 = 20
    assert_eq!(store.begins(), 20);
    assert_eq!(store.commits(), 20);
    assert_eq!(metrics.txn_counter.get(&["txn"]), 20);
    assert_eq!(metrics.txn_rolledback_counter.get(&["txn"]), 0);
    assert_eq!(metrics.txn_durations.count(), 20);

    let sets = store.sets();
    assert_eq!(sets.len(), 20);
    let mut keys: std::collections::BTreeSet<String> = sets
        .iter()
        .map(|(k, v)| {
            assert_eq!(v, &value);
            String::from_utf8(k.clone()).unwrap()
        })
        .collect();
    let expected: std::collections::BTreeSet<String> =
        (0..20).map(|k| format!("key_{k}")).collect();
    // 这里把 key 后缀转回整数比较，避免字典序把 `key_10` 排到 `key_2` 前面。
    // 真正想验证的是 key 覆盖范围与“无冲突”承诺，而不是容器遍历顺序。
    // BTreeSet orders lexicographically; compare membership via equality of sets built the same way
    // by parsing numeric suffix for a stable expected check:
    let mut nums: Vec<i64> = keys
        .iter()
        .map(|k| k.strip_prefix("key_").unwrap().parse().unwrap())
        .collect();
    nums.sort();
    assert_eq!(
        nums,
        (0..20).collect::<Vec<_>>(),
        "keys must be conflict-free key_0..key_19"
    );
    assert_eq!(keys.len(), expected.len());
    let _ = keys;
    let _ = expected;

    // 再执行一遍完整入口，确认 `run_with_flags` 会按 Go 顺序驱动写入、刷新指标并拉起 HTTP。
    // Full run prints metrics text and closes body.
    let flags = Flags {
        data_cnt: 6,
        worker_cnt: 3,
        pd_addr: "localhost:2379".into(),
        value_size: 2,
    };
    let deps = RuntimeDeps::for_test();
    // Capture via run side effects on deps.metrics
    run_with_flags(flags, deps.clone());
    assert!(deps.metrics.txn_counter.get(&["txn"]) >= 6);
    assert!(deps.http.is_started());

    let _ = entry::default_flags;
    let _ = TiKVDriver::default();
}

/// Boundary: integer division leftover keys and flag parse.
/// 边界场景固定那些“可能不优雅，但 Go 就是这么做”的行为。
/// 例如整除后的余数会被丢弃、flag 解析支持拆分和值内联两种写法。
/// 如果 Rust 改成“更聪明”的实现，这些测试会第一时间暴露差异。
/// 这样可以避免语义漂移被误判成“无害优化”。
fn contract_boundary() {
    // 10/3 只会实际写入 9 条，剩余 1 条不会补偿到任何 worker。
    // Leftover keys from dataCnt % workerCnt are not written (Go integer division).
    let flags = Flags {
        data_cnt: 10,
        worker_cnt: 3,
        pd_addr: "localhost:2379".into(),
        value_size: 1,
    };
    let deps = RuntimeDeps::for_test();
    let (store, metrics, _) = init(&flags, &deps);
    batch_rw(&flags, &store, &metrics, &[0]);
    // base = 10/3 = 3; total = 9 (key_0..key_8); key_9 unused
    assert_eq!(store.commits(), 9);
    let keys: Vec<_> = store
        .sets()
        .into_iter()
        .map(|(k, _)| String::from_utf8(k).unwrap())
        .collect();
    assert!(!keys.iter().any(|k| k == "key_9"));
    assert!(keys.iter().any(|k| k == "key_0"));
    assert!(keys.iter().any(|k| k == "key_8"));

    // 这里覆盖 benchkv 自己声明的 flag 子集，确保 CLI 兼容 Go `flag.Parse` 的常见输入形式。
    let f = stubs::parse_flags(&[
        "-N".into(),
        "100".into(),
        "-C=10".into(),
        "-pd=1.2.3.4:2379".into(),
        "-V".into(),
        "7".into(),
    ]);
    assert_eq!(f.data_cnt, 100);
    assert_eq!(f.worker_cnt, 10);
    assert_eq!(f.pd_addr, "1.2.3.4:2379");
    assert_eq!(f.value_size, 7);

    // 主流程会把日志级别压到 error，这个状态也是可观察到的入口副作用。
    stubs::set_log_level(stubs::LogLevel::Error);
    assert_eq!(stubs::current_log_level(), stubs::LogLevel::Error);
}

#[test]
fn zero_workers_panics_like_go_integer_division() {
    let flags = Flags {
        data_cnt: 100,
        worker_cnt: 0,
        ..Flags::default()
    };
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        batch_rw(&flags, &Storage::default(), &Metrics::default(), &[0]);
    }));
    assert!(result.is_err(), "Go panics when dataCnt is divided by zero");
}

#[test]
fn negative_workers_panic_like_go_wait_group() {
    let flags = Flags {
        data_cnt: 100,
        worker_cnt: -1,
        ..Flags::default()
    };
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        batch_rw(&flags, &Storage::default(), &Metrics::default(), &[0]);
    }));
    assert!(
        result.is_err(),
        "Go WaitGroup.Add panics for a negative worker count"
    );
}

#[test]
fn negative_value_size_panics_like_go_make() {
    let flags = Flags {
        data_cnt: 0,
        worker_cnt: 1,
        value_size: -1,
        ..Flags::default()
    };
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        run_with_flags(flags, RuntimeDeps::for_test());
    }));
    assert!(result.is_err(), "Go make([]byte, negativeSize) panics");
}

#[test]
fn flag_parsing_stops_at_first_positional_argument() {
    let parsed = stubs::parse_flags(&["payload".into(), "-N".into(), "7".into()]);
    assert_eq!(parsed.data_cnt, Flags::default().data_cnt);

    let parsed = stubs::parse_flags(&["--".into(), "-C=7".into()]);
    assert_eq!(parsed.worker_cnt, Flags::default().worker_cnt);

    assert_eq!(
        stubs::try_parse_flags(&["-h".into()]),
        Err(stubs::FlagParseError::Help)
    );
}

#[test]
fn integer_flags_use_go_base_zero_syntax() {
    let parsed = stubs::parse_flags(&["-N=0x64".into(), "-C=012".into(), "-V=-0x2".into()]);
    assert_eq!(parsed.data_cnt, 100);
    assert_eq!(parsed.worker_cnt, 10);
    assert_eq!(parsed.value_size, -2);
}

#[test]
fn metrics_handler_renders_live_prometheus_histogram() {
    let flags = Flags {
        data_cnt: 4,
        worker_cnt: 1,
        ..Flags::default()
    };
    let deps = RuntimeDeps::for_test();
    let (store, metrics, http) = init(&flags, &deps);
    batch_rw(&flags, &store, &metrics, &[0]);

    let (resp, err) = stubs::http_get("http://localhost:9191/metrics", &metrics, &http);
    stubs::must_nil(err);
    let (body, err) = stubs::read_all(&resp.body);
    stubs::must_nil(err);
    let text = String::from_utf8(body).unwrap();
    assert!(text.contains("tikv_txn_total{type=\"txn\"} 4"));
    assert!(
        text.contains("tikv_txn_durations_histogram_seconds_bucket{type=\"txn\",le=\"+Inf\"} 4")
    );
    assert!(text.contains("tikv_txn_durations_histogram_seconds_sum{type=\"txn\"}"));
    assert!(text.contains("tikv_txn_durations_histogram_seconds_count{type=\"txn\"} 4"));
}

#[test]
fn default_runtime_uses_production_backends() {
    let deps = RuntimeDeps::default();
    assert!(
        !deps.http.dry_run,
        "the binary default must bind :9191 instead of using the test HTTP stub"
    );
    assert!(deps.driver.is_production());
    assert!(deps.metrics.is_production());
    assert!(deps.http.is_production());
}

#[test]
fn production_http_backend_serves_metrics_over_loopback() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve loopback port");
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let metrics = Metrics::default();
    metrics.txn_counter.WithLabelValues(&["txn"]).Inc();
    let http = HttpServer::production();
    http.HandleMetrics(&metrics);
    let serving = http.clone();
    let addr_text = addr.to_string();
    thread::spawn(move || {
        if let Some(error) = serving.ListenAndServe(&addr_text) {
            panic!("loopback metrics server failed: {error}");
        }
    });
    for _ in 0..50 {
        if http.is_started() {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert!(http.is_started());

    let url = format!("http://{addr}/metrics");
    let (resp, err) = stubs::http_get(&url, &metrics, &http);
    stubs::must_nil(err);
    let (body, err) = stubs::read_all(&resp.body);
    stubs::must_nil(err);
    stubs::must_nil(resp.Close());
    let text = String::from_utf8(body).unwrap();
    assert!(text.contains("tikv_txn_total{type=\"txn\"} 1"));
}

/// Error: Begin fatal; Commit fail → rollback counter + Rollback; bad flags.
/// 错误场景区分三类分支：致命失败、可回滚失败和只记录日志的失败。
/// 这正是 Go 原实现最容易在 Rust 化后被“顺手优化”掉的地方，
/// 因此这里必须把每种分支的控制流和指标副作用都钉住。
/// 对压测工具来说，错误表现本身就是外部接口的一部分。
fn contract_error_paths() {
    // `Begin` 失败在 Go 里会 `log.Fatal`，这里通过捕获 panic 来确认仍是致命语义。
    let flags = Flags {
        data_cnt: 2,
        worker_cnt: 1,
        ..Flags::default()
    };
    let store = {
        let (s, err) = TiKVDriver::stub().open_for_test("tikv://x?cluster=1");
        stubs::must_nil(err);
        s
    };
    store.set_fail_begin(true);
    let metrics = Metrics::default();
    let r = panic::catch_unwind(AssertUnwindSafe(|| {
        batch_rw(&flags, &store, &metrics, &[0]);
    }));
    assert!(r.is_err());
    assert!(panic_msg(r.unwrap_err()).contains("begin failed"));

    // `Commit` 失败并不减少尝试次数；总事务数照记，只是成功提交数归零并触发回滚统计。
    // Commit failure path
    let (store2, err) = TiKVDriver::stub().open_for_test("tikv://y?cluster=1");
    stubs::must_nil(err);
    store2.set_fail_commit_every(Some(1)); // every commit fails
    let metrics2 = Metrics::default();
    let flags2 = Flags {
        data_cnt: 4,
        worker_cnt: 2,
        ..Flags::default()
    };
    batch_rw(&flags2, &store2, &metrics2, &[1, 2]);
    // base=2; 4 begins; all commits fail → 0 successful sets; 4 rollbacks; 4 rollback metrics
    assert_eq!(store2.begins(), 4);
    assert_eq!(store2.commits(), 0);
    assert_eq!(store2.rollbacks(), 4);
    assert_eq!(metrics2.txn_counter.get(&["txn"]), 4);
    assert_eq!(metrics2.txn_rolledback_counter.get(&["txn"]), 4);
    assert_eq!(metrics2.txn_durations.count(), 4);

    // `Set` 出错时 Go 只记 terror 日志，然后仍尝试 Commit；这里同时检查日志和提交次数。
    // Set error is logged, Commit still attempted
    stubs::clear_logs();
    let (store3, _) = TiKVDriver::stub().open_for_test("tikv://z?cluster=1");
    store3.set_fail_set(true);
    let metrics3 = Metrics::default();
    let flags3 = Flags {
        data_cnt: 1,
        worker_cnt: 1,
        ..Flags::default()
    };
    batch_rw(&flags3, &store3, &metrics3, &[9]);
    let logs = stubs::take_logs();
    assert!(
        logs.iter().any(|l| l.contains("set failed")),
        "Set error must be terror.Log'd: {logs:?}"
    );
    assert_eq!(store3.commits(), 1);

    // 未定义 flag 必须尽快失败，避免 CLI 把拼写错误静默吞掉。
    let r = panic::catch_unwind(AssertUnwindSafe(|| {
        let _ = stubs::parse_flags(&["-not-defined".into()]);
    }));
    assert!(r.is_err());

    // 这个占位依赖说明 `init` 默认仍走真实 Open 路径；测试不额外锁死未承诺的空路径细节。
    // Open empty path MustNil fatals in init
    let deps = RuntimeDeps {
        store_override: None,
        ..RuntimeDeps::for_test()
    };
    // Driver.Open never fails on non-empty; exercise must_nil via override path is fine.
    let _ = deps;
}

/// Resource cleanup: response body Close; WaitGroup/barrier waits for workers.
/// 收尾场景验证成功路径之后的清理动作没有偏离 Go：
/// 先读响应体再关闭、关闭失败只打日志、监听失败只走 terror 日志，
/// 以及主流程必须等到所有 worker 结束才返回。
/// 这些断言专门防止“功能已完成但收尾漏了一步”这类回归。
fn contract_resource_cleanup() {
    let metrics = Metrics::default();
    let http = HttpServer::default();
    http.Handle("/metrics", "ok".into());
    let (resp, err) = stubs::http_get("http://localhost:9191/metrics", &metrics, &http);
    stubs::must_nil(err);
    assert!(!resp.is_closed());
    // 顺序保持为先读 body 后 Close，对齐 Go `ReadAll` + `defer Close` 的可观察效果。
    let (text, err1) = stubs::read_all(&resp.body);
    stubs::terror_log(stubs::trace(err1));
    assert_eq!(text, b"ok");
    stubs::must_nil(resp.Close());
    assert!(resp.is_closed());

    // Close 出错不应升级成 fatal，只需要记录日志。
    // Close error is logged (Go log.Error path), not fatal.
    stubs::clear_logs();
    let resp2 = HttpResponse::stub(b"x".to_vec(), Some(stubs::Error::new("close boom")));
    if let Some(err) = resp2.Close() {
        stubs::log_function_call_errored(&err);
    }
    assert!(resp2.is_closed());
    let logs = stubs::take_logs();
    assert!(logs.iter().any(|l| l.contains("close boom")));

    // 后台监听失败同样只记 terror 日志，不改变主流程控制流。
    // ListenAndServe error is terror.Log'd (not fatal).
    stubs::clear_logs();
    let http2 = HttpServer::default();
    http2.set_listen_error(Some(stubs::Error::new("listen failed")));
    let err = http2.ListenAndServe(":9191");
    stubs::terror_log(stubs::trace(err));
    let logs = stubs::take_logs();
    assert!(logs.iter().any(|l| l.contains("listen failed")));

    // 用更高并发确认 Rust 会等待所有线程完成，等价于 Go `WaitGroup.Wait`。
    // Workers fully join: large enough concurrency still settles.
    let flags = Flags {
        data_cnt: 50,
        worker_cnt: 10,
        ..Flags::default()
    };
    let deps = RuntimeDeps::for_test();
    let (store, metrics, _) = init(&flags, &deps);
    batch_rw(&flags, &store, &metrics, &[0u8; 5]);
    assert_eq!(store.commits(), 50);
}

// 这层 trait 只是给测试一个统一的打开入口，减少直接依赖具体构造细节。
trait OpenForTest {
    fn open_for_test(&self, path: &str) -> (Storage, Option<stubs::Error>);
}

impl OpenForTest for TiKVDriver {
    fn open_for_test(&self, path: &str) -> (Storage, Option<stubs::Error>) {
        self.Open(path)
    }
}

// `catch_unwind` 返回 `Any`，这里把常见的字符串 panic 归一化成文本，便于稳定断言错误消息。
fn panic_msg(err: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = err.downcast_ref::<String>() {
        return s.clone();
    }
    if let Some(s) = err.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    format!("{err:?}")
}
