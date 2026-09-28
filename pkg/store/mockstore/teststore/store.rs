// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// `teststore` 只包装真实 mockstore 工厂，避免引入 testkit 形成循环依赖。

use std::sync::Arc;

use astersql_keyspace::{ApiVersion, BasicCodec, Codec};
use astersql_store::{Storage, StorageRef};
use astersql_store_mockstore::{MockStorage, MockTiKVStoreOption};

pub use astersql_store_mockstore::StoreError;

/// 将 mockstore 的具体存储适配为 store 包使用的共享 Storage 接口。
pub struct MockStore {
    inner: MockStorage,
    keyspace: String,
    codec: BasicCodec,
}

impl MockStore {
    fn new(inner: MockStorage) -> Self {
        let (keyspace, keyspace_id) = inner
            .current_keyspace
            .as_ref()
            .map_or_else(|| (String::new(), 0), |meta| (meta.name.clone(), meta.id));
        let codec = BasicCodec {
            api_version: if keyspace.is_empty() {
                ApiVersion::V1
            } else {
                ApiVersion::V2
            },
            keyspace_id,
        };
        Self {
            inner,
            keyspace,
            codec,
        }
    }

    /// 暴露真实 mockstore 的 latch 状态，供构造选项回归测试使用。
    pub fn is_latch_enabled(&self) -> bool {
        self.inner.is_latch_enabled()
    }
}

impl Storage for MockStore {
    fn GetKeyspace(&self) -> &str {
        &self.keyspace
    }

    fn GetCodec(&self) -> &dyn Codec {
        &self.codec
    }

    fn Close(&self) -> Result<(), astersql_store::StoreError> {
        self.inner
            .close()
            .map_err(|error| astersql_store::StoreError::other(error.to_string()))
    }
}

/// 使用 mockstore 的完整后端、选项处理和错误路径创建存储。
pub fn NewMockStore(opts: Vec<MockTiKVStoreOption>) -> Result<Arc<MockStore>, StoreError> {
    let next_gen = astersql_config_kerneltype::IsNextGen();
    astersql_store_mockstore::set_next_gen(next_gen);
    astersql_store_mockstore::NewMockStore(opts).map(|store| Arc::new(MockStore::new(store)))
}

/// 创建真实 MockStore；NextGen 下同步配置并登记同一个 SYSTEM 存储实例。
pub fn NewMockStoreWithoutBootstrap(
    opts: Vec<MockTiKVStoreOption>,
) -> Result<Arc<MockStore>, StoreError> {
    let store = NewMockStore(opts)?;
    if astersql_config_kerneltype::IsNextGen() {
        astersql_config::update_global(|config| {
            config.keyspace_name = astersql_keyspace::System.into();
            config.instance.tidb_service_scope =
                astersql_dxf_framework_handle::NEXT_GEN_TARGET_SCOPE.into();
        });
        let storage: StorageRef = store.clone();
        astersql_store::SetSystemStorage(Some(storage));
    }
    Ok(store)
}
