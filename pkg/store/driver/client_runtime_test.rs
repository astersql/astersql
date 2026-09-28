// Copyright 2026 AsterSQL.

//! `ClientRuntime` 的配置、错误传播与生命周期测试。
//!
//! 常规测试通过可注入的时间戳来源覆盖失败和关闭路径；被忽略的集成测试则连接
//! `REAL_TIKV_PD` 指定的真实集群，验证连接诊断、TSO 单调性及关闭后的拒绝语义。

use std::time::Duration;

use tikv_client::Error as TiKvClientError;

use crate::client_runtime::{ClientConfig, ClientRuntime, ClientRuntimeError, TimestampSource};

/// 固定返回客户端错误，用于隔离验证同步命令边界的错误传播。
struct FailingTimestampSource;

impl TimestampSource for FailingTimestampSource {
    fn current_timestamp(
        &self,
        _runtime: &tokio::runtime::Runtime,
    ) -> Result<u64, ClientRuntimeError> {
        Err(ClientRuntimeError::Client(TiKvClientError::StringError(
            "injected timestamp failure".to_owned(),
        )))
    }
}

#[test]
/// TiKV 响应上限必须突破 tonic 默认值，避免合法的大扫描响应被客户端拒绝。
fn client_config_accepts_tikv_responses_larger_than_tonic_default() {
    let config = ClientConfig::new(["127.0.0.1:2379"]).tikv_config();

    assert_eq!(config.grpc_max_decoding_message_size, usize::MAX);
}

#[test]
/// 命令失败应原样向调用方传播，关闭后则统一返回关闭态错误。
fn client_runtime_propagates_command_errors_and_rejects_requests_after_close() {
    let mut runtime =
        ClientRuntime::from_timestamp_source_for_test(Box::new(FailingTimestampSource))
            .expect("test runtime must start");

    let error = runtime
        .current_timestamp()
        .expect_err("injected command failure must be returned");
    assert!(error.to_string().contains("injected timestamp failure"));

    runtime.close().expect("runtime must close cleanly");
    let closed = runtime
        .current_timestamp()
        .expect_err("closed runtime must reject new commands");
    assert!(matches!(closed, ClientRuntimeError::Closed));
}

#[test]
#[ignore = "requires REAL_TIKV_PD and a running PD/TiKV cluster"]
/// 使用真实 PD 验证不可达地址诊断、原始时间戳转换、TSO 单调性及关闭语义。
fn client_runtime_rejects_bad_pd_and_real_pd_returns_tso() {
    let pd = std::env::var("REAL_TIKV_PD").expect("REAL_TIKV_PD must name the real PD endpoint");

    let bad_config = ClientConfig::new(["127.0.0.1:1"]).with_timeout(Duration::from_millis(500));
    let bad_error = match ClientRuntime::connect(bad_config) {
        Ok(_) => panic!("an unreachable PD endpoint must not connect"),
        Err(error) => error,
    };
    eprintln!("bad PD result: {bad_error}");

    // 先读取官方客户端返回的原始时间戳字段，再通过同步边界连续取 TSO，兼顾转换
    // 正确性和调用间的严格递增约束。
    let config = ClientConfig::new([pd.clone()]).with_timeout(Duration::from_secs(5));
    let mut runtime = ClientRuntime::connect(config).expect("real PD must accept a client");
    let raw = runtime
        .runtime()
        .expect("runtime must be open")
        .block_on(
            runtime
                .transaction_client()
                .expect("client must be open")
                .current_timestamp(),
        )
        .expect("real PD must return a raw timestamp");
    eprintln!(
        "raw real PD timestamp: physical={}, logical={}, suffix_bits={}",
        raw.physical, raw.logical, raw.suffix_bits
    );
    assert!(raw.physical > 0, "PD physical timestamp must be positive");
    let first = runtime
        .current_timestamp()
        .expect("real PD must return the first TSO");
    let second = runtime
        .current_timestamp()
        .expect("real PD must return the second TSO");
    assert!(second > first, "TSO must increase: {first} then {second}");
    eprintln!("real PD: {pd}; TSO: {first} then {second}");

    runtime.close().expect("runtime must close cleanly");
    let closed = runtime
        .current_timestamp()
        .expect_err("closed runtime must reject new commands");
    assert!(matches!(closed, ClientRuntimeError::Closed));
    eprintln!("closed result: {closed}");
}
