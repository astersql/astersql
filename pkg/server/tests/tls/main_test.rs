// Copyright 2026 AsterSQL.

// TLS 测试进程级初始化与默认配置基线单测。
//
// 对齐 Go TestMain：公共测试初始化、TopSQL 与 metrics 注册，并确认全局配置未被 init 污染。

use std::sync::OnceLock;

static INITIALIZED: OnceLock<Result<(), String>> = OnceLock::new();

fn initialize_tls_test_environment() -> Result<(), String> {
    INITIALIZED
        .get_or_init(|| {
            astersql_testkit_testsetup::SetupForCommonTest();
            astersql_util_topsql_state::EnableTopSQL();

            let default_config = astersql_config::new_config();
            let global_config = astersql_config::get_global_config();
            if format!("{default_config:#?}") != format!("{global_config:#?}") {
                eprintln!(
                    "server: the global config has been changed.\ndefault: {default_config:#?}\nglobal: {global_config:#?}"
                );
            }

            // SAFETY: INITIALIZED 保证本测试进程只初始化、注册一次 metrics。
            unsafe {
                astersql_metrics::metrics::InitMetrics().map_err(|error| error.to_string())?;
                astersql_metrics::metrics::RegisterMetrics().map_err(|error| error.to_string())?;
            }
            Ok(())
        })
        .clone()
}

/// TestMain 初始化应幂等，并保持默认/全局配置及 server 默认监听配置一致。
#[test]
fn canonical_tls_test_main_initializes_shared_environment() {
    let _lock = crate::tls_test::global_test_lock();
    initialize_tls_test_environment().expect("TLS TestMain environment must initialize");
    initialize_tls_test_environment().expect("TLS TestMain initialization must be idempotent");
    assert!(astersql_util_topsql_state::TopSQLEnabled());

    let expected = astersql_config::new_config();
    let actual = astersql_config::get_global_config();
    assert_eq!(actual.host, expected.host);
    assert_eq!(actual.port, expected.port);
    assert_eq!(actual.store, expected.store);
    assert_eq!(actual.status.status_port, expected.status.status_port);
    assert_eq!(actual.status.report_status, expected.status.report_status);

    let config = astersql_server::server::ServerConfig::default();
    assert_eq!(config.host, "0.0.0.0");
    assert!(config.status.report_status);
    assert_eq!(config.status.host, "127.0.0.1");
}
