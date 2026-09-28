// Copyright 2026 AsterSQL.

// 验证 Rust 版无引导 MockStore 构造器与 Go 包装函数保持一致。
//
// 覆盖经典模式的选项应用、NextGen 模式的全局配置与系统存储注册，
// 以及构造选项失败时的错误传播契约。

use astersql_store::Storage;
use astersql_store_mockstore::{WithDDLChecker, WithTxnLocalLatches};

use super::NewMockStoreWithoutBootstrap;

#[test]
fn new_mock_store_without_bootstrap_matches_go_wrapper() {
    let restore_config = astersql_config::restore_func();
    let initial_config = astersql_config::get_global_config();
    astersql_store::ResetStoreStateForTest();

    // 使用真实 mockstore 选项与后端，而不是 teststore 自建的 HashMap 替身。
    let store =
        NewMockStoreWithoutBootstrap(vec![WithTxnLocalLatches(32)]).expect("real mock store");
    assert!(store.is_latch_enabled());

    if astersql_config_kerneltype::IsNextGen() {
        let config = astersql_config::get_global_config();
        assert_eq!(config.keyspace_name, astersql_keyspace::System);
        assert_eq!(
            config.instance.tidb_service_scope,
            astersql_dxf_framework_handle::NEXT_GEN_TARGET_SCOPE
        );
        let registered = astersql_store::GetSystemStorage().expect("system storage");
        assert_eq!(
            astersql_store::StorageIdentity(&registered),
            astersql_store::StorageIdentity(&(store.clone() as astersql_store::StorageRef))
        );
        assert_eq!(store.GetKeyspace(), astersql_keyspace::System);
    } else {
        let config = astersql_config::get_global_config();
        assert_eq!(config.keyspace_name, initial_config.keyspace_name);
        assert_eq!(
            config.instance.tidb_service_scope,
            initial_config.instance.tidb_service_scope
        );
        assert!(astersql_store::GetSystemStorage().is_none());
        assert_eq!(store.GetKeyspace(), "");
    }

    // 真实 NewMockStore 的构造错误必须由包装器原样传播。
    let error = match NewMockStoreWithoutBootstrap(vec![WithDDLChecker()]) {
        Ok(_) => panic!("option error was swallowed"),
        Err(error) => error,
    };
    assert_eq!(error.0, "DDL checker injector is not installed");

    astersql_store::ResetStoreStateForTest();
    restore_config();
}
