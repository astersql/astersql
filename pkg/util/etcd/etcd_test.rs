// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 命名空间包装行为测试。
//
// 用内存 KV 代替真实 etcd，验证 `SetEtcdCliByNamespace` 后写入落在带前缀的 key 上。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use util_etcd::{NamespacedClient, SetEtcdCliByNamespace};

#[derive(Clone, Default)]
/// 线程安全的内存 KV，模拟 etcd 存储。
struct SharedKv {
    entries: Arc<Mutex<HashMap<String, String>>>,
}

impl SharedKv {
    fn put(&self, key: String, value: String) {
        self.entries.lock().unwrap().insert(key, value);
    }

    fn get(&self, key: &str) -> Option<String> {
        self.entries.lock().unwrap().get(key).cloned()
    }

    fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

/// 按当前 namespace 前缀写入，模拟 Go namespaced Put。
fn put_namespaced(client: &mut NamespacedClient<SharedKv>, key: &str, value: &str) {
    let prefix = std::str::from_utf8(client.namespace_prefix()).unwrap();
    client
        .inner()
        .put(format!("{prefix}{key}"), value.to_owned());
}

/// 构造未加前缀的 SharedKv 与 NamespacedClient。
fn test_setup_original() -> (SharedKv, NamespacedClient<SharedKv>) {
    let unprefixed_kv = SharedKv::default();
    let client = NamespacedClient::new(unprefixed_kv.clone());
    (unprefixed_kv, client)
}

#[test]
/// 设置 namespace 后 Put 应写入 `prefix+key`，且仅一条记录。
fn test_set_etcd_cli_by_namespace() {
    let (unprefixed_kv, mut client) = test_setup_original();

    let namespace_prefix = "testNamespace/";
    let key = "testkey";
    let value = "test";

    SetEtcdCliByNamespace(&mut client, namespace_prefix);
    put_namespaced(&mut client, key, value);

    assert_eq!(
        unprefixed_kv.get("testNamespace/testkey"),
        Some("test".to_owned())
    );
    assert_eq!(unprefixed_kv.len(), 1);
}

#[tokio::test]
#[ignore = "requires ASTER_ETCD_TEST_ENDPOINT for a real etcd server"]
async fn namespaced_real_kv_roundtrip_keeps_binary_keys_and_values() {
    let endpoint = std::env::var("ASTER_ETCD_TEST_ENDPOINT").unwrap();
    let mut raw = etcd_client::Client::connect([endpoint], None)
        .await
        .unwrap();
    let mut client = crate::NamespacedClient::new(raw.clone());
    crate::SetEtcdCliByNamespace(&mut client, "/keyspaces/tidb/48");
    client
        .put(vec![b'/', 0, 255], vec![0, 255], None)
        .await
        .unwrap();
    assert_eq!(
        client.get(vec![b'/', 0, 255], false).await.unwrap(),
        vec![(vec![b'/', 0, 255], vec![0, 255])]
    );
    let mut key = b"/keyspaces/tidb/48/".to_vec();
    key.extend([0, 255]);
    assert_eq!(
        raw.get(key.clone(), None).await.unwrap().kvs()[0].value(),
        [0, 255]
    );
    raw.delete(key, None).await.unwrap();
}
