// Copyright 2026 AsterSQL.

// standby 包 canonical 主流程单测。
//
// 验证 `NoopStandbyController` 在激活准备阶段会且仅会调用一次
// `init_tidb_listener`，完成待命到可服务状态的监听器初始化。

use std::sync::Once;
use std::time::Duration;

use astersql_sessionctx_vardef::SetSchemaLease;
use astersql_testkit_testsetup::SetupForCommonTest;
use astersql_util_topsql_state::{EnableTopSQL, TopSQLEnabled};

static TEST_MAIN_INIT: Once = Once::new();

/// Rust 测试没有 Go 的进程级 TestMain，因此用一次性初始化复现其公共环境设置。
fn ensure_test_main_environment() {
    TEST_MAIN_INIT.call_once(|| {
        SetupForCommonTest();
        EnableTopSQL();
        SetSchemaLease(Duration::ZERO);
        unsafe {
            astersql_metrics::metrics::InitMetrics().expect("initialize server test metrics");
            astersql_metrics::metrics::RegisterMetrics().expect("register server test metrics");
        }
    });
}

/// TestMain 的公共初始化、TopSQL 开关、schema lease 与全局配置检查。
#[test]
fn canonical_standby_test_main_initializes_shared_environment() {
    ensure_test_main_environment();
    assert!(TopSQLEnabled());
    let default_config = format!("{:#?}", astersql_config::new_config());
    let global_config = format!("{:#?}", astersql_config::get_global_config());
    assert_eq!(
        default_config, global_config,
        "server global config was changed by package initialization"
    );
}

/// 待命控制器激活时，准备阶段应恰好触发一次 TiDB 监听器初始化。
#[test]
fn canonical_standby_default_controller_activates_listener_once() {
    use astersql_server::standby::{NoopStandbyController, StandbyController, StandbyReadyServer};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 记录 `init_tidb_listener` 调用次数的假 Ready Server。
    struct Ready(AtomicUsize);
    impl StandbyReadyServer for Ready {
        fn init_tidb_listener(&self) -> Result<(), String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let ready = Ready(AtomicUsize::new(0));
    // 空操作待命控制器：激活准备时直接初始化监听器。
    NoopStandbyController
        .prepare_for_activation(&ready)
        .unwrap();
    assert_eq!(ready.0.load(Ordering::SeqCst), 1);
}
