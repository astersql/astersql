// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// `options` 模块的功能选项模式单测。
//
// 直接验证生产侧 functional options，覆盖 Go `TestOptions` 的组合场景以及
// Rust 侧其余依赖注入字段和选项应用顺序。

use std::sync::Arc;
use std::time::Duration;

use ddl_systable::{SchemaLoader, SchemaLoaderError};

use crate::options::{
    apply_options, with_auto_id_client, with_etcd_client, with_event_publish_store,
    with_info_cache, with_lease, with_schema_loader, with_store,
};

struct TestSchemaLoader;

impl SchemaLoader for TestSchemaLoader {
    fn reload(&self) -> Result<(), SchemaLoaderError> {
        Ok(())
    }
}

/// 组合多个 with_* 选项后，各字段应与输入一致。
#[test]
fn test_options() {
    let lease = Duration::from_secs(3);
    let loader: Arc<dyn SchemaLoader> = Arc::new(TestSchemaLoader);
    let configured = apply_options([
        with_etcd_client("test"),
        with_lease(lease),
        with_store("mock-store"),
        with_info_cache("cache-16"),
        with_auto_id_client("auto-id"),
        with_schema_loader(Arc::clone(&loader)),
        with_event_publish_store("event-store"),
    ]);

    assert_eq!(configured.etcd_client.as_deref(), Some("test"));
    assert_eq!(configured.lease, lease);
    assert_eq!(configured.store.as_deref(), Some("mock-store"));
    assert_eq!(configured.info_cache.as_deref(), Some("cache-16"));
    assert_eq!(configured.auto_id_client.as_deref(), Some("auto-id"));
    assert!(Arc::ptr_eq(
        configured.schema_loader.as_ref().expect("schema loader"),
        &loader
    ));
    assert_eq!(
        configured.event_publish_store.as_deref(),
        Some("event-store")
    );
}

#[test]
fn options_are_applied_in_order() {
    let configured = apply_options([with_store("first-store"), with_store("last-store")]);

    assert_eq!(configured.store.as_deref(), Some("last-store"));
}
