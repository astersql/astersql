// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// `GlobalConfigSyncer` 与 PD 全局配置写入路径的单元测试。
//
// 用内存 Fake PD 客户端验证：notify → recv → store 链路，以及
// `SET GLOBAL` 相关变量名（如 enable_resource_metering）写入后的键值。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::globalconfig::{
    GlobalConfigClient, GlobalConfigError, GlobalConfigEventType, GlobalConfigItem,
    GlobalConfigSyncer,
};

/// 内存 Fake PD：按 `/global/config/{name}` 键保存配置项。
#[derive(Default)]
struct FakePdClient {
    items: Mutex<BTreeMap<String, String>>,
    stored_items: Mutex<Vec<GlobalConfigItem>>,
}

impl FakePdClient {
    /// 按短名列表读取配置；revision 固定为 0（测试桩）。
    fn load(&self, names: &[&str]) -> (Vec<GlobalConfigItem>, i64) {
        let guard = self.items.lock().unwrap();
        let items = names
            .iter()
            .map(|name| {
                let key = format!("/global/config/{name}");
                GlobalConfigItem::new(
                    key,
                    guard
                        .get(&format!("/global/config/{name}"))
                        .cloned()
                        .unwrap_or_default(),
                )
            })
            .collect();
        (items, 0)
    }
}

impl GlobalConfigClient for FakePdClient {
    fn store_global_config(
        &self,
        _prefix: &str,
        items: &[GlobalConfigItem],
    ) -> Result<(), GlobalConfigError> {
        self.stored_items.lock().unwrap().extend_from_slice(items);
        let mut guard = self.items.lock().unwrap();
        // 已带 `/` 前缀的 name 原样作为键，否则补全 `/global/config/`。
        for item in items {
            let key = if item.name.starts_with('/') {
                item.name.clone()
            } else {
                format!("/global/config/{}", item.name)
            };
            guard.insert(key, item.value.clone());
        }
        Ok(())
    }
}

/// Corresponds to Go `TestGlobalConfigSyncer`.
///
/// 对应 Go `TestGlobalConfigSyncer`：单条 notify/store 后可读回。
#[test]
fn test_global_config_syncer() {
    let client = Arc::new(FakePdClient::default());
    let syncer = GlobalConfigSyncer::new(Some(client.clone()));
    syncer.notify(GlobalConfigItem::new("a", "b"));
    let item = syncer.recv_notification().unwrap();
    syncer.store_global_config(item).unwrap();

    let (items, revision) = client.load(&["a"]);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "/global/config/a");
    assert_eq!(revision, 0);
    assert_eq!(items[0].value, "b");
}

/// Go `pd.GlobalConfigItem` 的事件类型与 payload 必须在 notify/store 链路中原样保留。
#[test]
fn global_config_item_metadata_survives_notify_and_store() {
    assert_eq!(GlobalConfigEventType::Put as i32, 0);
    assert_eq!(GlobalConfigEventType::Delete as i32, 1);
    let default_put = GlobalConfigItem::new("current", "value");
    assert_eq!(default_put.event_type, GlobalConfigEventType::Put);
    assert!(default_put.payload.is_empty());

    let client = Arc::new(FakePdClient::default());
    let syncer = GlobalConfigSyncer::new(Some(client.clone()));
    let item = GlobalConfigItem {
        event_type: GlobalConfigEventType::Delete,
        name: "obsolete".to_owned(),
        value: String::new(),
        payload: vec![0, 1, 2, 255],
    };

    syncer.notify(item.clone());
    let notified = syncer.recv_notification().unwrap();
    assert_eq!(notified, item);

    syncer.store_global_config(notified).unwrap();
    assert_eq!(*client.stored_items.lock().unwrap(), vec![item]);
}

/// Corresponds to Go `TestStoreGlobalConfig`.
///
/// Go drives this through `SET GLOBAL` + domain bootstrap; Rust covers the syncer
/// side of that path: notified variable changes are stored into PD global config.
///
/// 对应 Go `TestStoreGlobalConfig`：多条变量通知按序写入 PD。
#[test]
fn test_store_global_config() {
    let client = Arc::new(FakePdClient::default());
    let syncer = GlobalConfigSyncer::new(Some(client.clone()));

    // enable top sql is translated to enable_resource_metering in Go.
    // Go 中 enable top sql 会映射为 enable_resource_metering。
    syncer.notify(GlobalConfigItem::new("enable_resource_metering", "true"));
    syncer.notify(GlobalConfigItem::new("source_id", "2"));
    let first = syncer.recv_notification().unwrap();
    let second = syncer.recv_notification().unwrap();
    syncer.store_global_config(first).unwrap();
    syncer.store_global_config(second).unwrap();

    let (items, _) = client.load(&["enable_resource_metering", "source_id"]);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].name, "/global/config/enable_resource_metering");
    assert_eq!(items[0].value, "true");
    assert_eq!(items[1].name, "/global/config/source_id");
    assert_eq!(items[1].value, "2");
}
