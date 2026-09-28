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
