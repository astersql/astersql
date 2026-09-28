// Copyright 2026 AsterSQL.

// unistore server 从 Go 迁移后的行为对齐测试。
//
// 覆盖 Badger 选项映射、Mock/Standalone 建服、PD ID 分配与停止生命周期。

use super::*;
use astersql_store_mockstore_unistore_config::{CompressionType, Config};
use std::sync::Arc;

fn volatile_config() -> Config {
    let mut config = astersql_store_mockstore_unistore_config::DefaultConf.clone();
    config.Engine.VolatileMode = true;
    config
}

/// Go 的 createDB 为 CompressionPerLevel 预分配 conf.Compression 长度，
/// 只填充 Badger 默认的 7 层；额外层保持 None。
#[test]
fn create_db_preserves_go_compression_option_shape() {
    let mut config = volatile_config();
    config.Engine.Compression = vec![
        "snappy".into(),
        "zstd".into(),
        "none".into(),
        "snappy".into(),
        "zstd".into(),
        "none".into(),
        "snappy".into(),
        "zstd".into(),
    ];

    let database = create_db(SUB_PATH_KV, None, &config.Engine).unwrap();
    let compression = &database
        .options()
        .table_builder_options
        .compression_per_level;
    assert_eq!(compression.len(), 8);
    assert_eq!(compression[0], CompressionType::Snappy);
    assert_eq!(compression[1], CompressionType::Zstd);
    assert_eq!(compression[6], CompressionType::Snappy);
    assert_eq!(compression[7], CompressionType::None);
    assert!(!database.options().compaction_filter_enabled);
}

/// NewMock 引导单 Store/Region，并在停止时关闭 MVCC 存储；stop 可重复调用。
#[test]
fn new_mock_bootstraps_and_closes_like_go_server() {
    let config = volatile_config();
    let (server, regions, pd) = new_mock(&config, 11).unwrap();

    assert!(regions.is_bootstrapped());
    assert_eq!(pd.cluster_id(), 11);
    assert_eq!(
        server.get_store_id_by_address(&config.Server.StoreAddr),
        Ok(1)
    );
    assert_eq!(
        server.get_store_address_by_id(1),
        Ok(config.Server.StoreAddr.clone())
    );
    let store = server.mvcc_store();
    assert!(!store.is_closed());

    server.stop().unwrap();
    assert!(store.is_closed());
    server.stop().unwrap();
}

/// Standalone 建服从外部 PD 获取 TSO，并连续分配 Store/Region/Peer 三个 ID。
#[test]
fn new_uses_external_pd_ids_and_propagates_lifecycle() {
    let mut config = volatile_config();
    config.Server.Raft = false;
    let regions = Arc::new(
        astersql_store_mockstore_unistore_tikv::mock_region::MockRegionManager::new(
            23,
            config.Server.RegionSize,
        ),
    );
    let pd = Arc::new(
        astersql_store_mockstore_unistore_tikv::mock_region::MockPd::new(Arc::clone(&regions)),
    );

    let server = new(&config, pd).unwrap();
    assert_eq!(
        server.get_store_id_by_address(&config.Server.StoreAddr),
        Ok(1)
    );
    assert_eq!(
        server.get_store_address_by_id(1),
        Ok(config.Server.StoreAddr.clone())
    );
    server.stop().unwrap();
    assert!(server.mvcc_store().is_closed());
}
