// Copyright 2026 AsterSQL.

// `GlobalConfigSyncer` 迁移期补充单元测试。
//
// 覆盖通知队列容量与 FIFO、无 client 时 store 空操作、
// 空 prefix 转发单条配置，以及客户端错误原样上抛。

use astersql_domain_globalconfigsync::globalconfig::*;
use std::sync::{Arc, Mutex};

/// 记录每次 `store_global_config` 调用，并可注入失败信息。
#[derive(Default)]
struct RecordingClient {
    calls: Mutex<Vec<(String, Vec<GlobalConfigItem>)>>,
    failure: Mutex<Option<String>>,
}

impl GlobalConfigClient for RecordingClient {
    fn store_global_config(
        &self,
        prefix: &str,
        items: &[GlobalConfigItem],
    ) -> Result<(), GlobalConfigError> {
        if let Some(message) = self.failure.lock().unwrap().clone() {
            return Err(GlobalConfigError::Client(message));
        }
        self.calls
            .lock()
            .unwrap()
            .push((prefix.to_owned(), items.to_vec()));
        Ok(())
    }
}

/// 验证通知通道容量为 8，且接收顺序与发送 FIFO 一致。
#[test]
fn notify_uses_capacity_eight_and_preserves_fifo_order() {
    let syncer = GlobalConfigSyncer::new(None);
    assert_eq!(syncer.notify_capacity(), 8);

    for index in 0..8 {
        syncer.notify(GlobalConfigItem::new(
            format!("name-{index}"),
            format!("value-{index}"),
        ));
    }

    for index in 0..8 {
        assert_eq!(
            syncer.recv_notification().unwrap(),
            GlobalConfigItem::new(format!("name-{index}"), format!("value-{index}"))
        );
    }
}

/// 无 PD client 时 `store_global_config` 应为空操作并返回 Ok。
#[test]
fn missing_client_makes_store_a_noop() {
    let syncer = GlobalConfigSyncer::new(None);
    assert_eq!(
        syncer.store_global_config(GlobalConfigItem::new("a", "b")),
        Ok(())
    );
}

/// 有 client 时应带空 prefix 转发恰好一项。
#[test]
fn store_forwards_one_item_with_the_empty_prefix() {
    let client = Arc::new(RecordingClient::default());
    let syncer = GlobalConfigSyncer::new(Some(client.clone()));
    let item = GlobalConfigItem::new("a", "b");

    syncer.store_global_config(item.clone()).unwrap();

    assert_eq!(
        *client.calls.lock().unwrap(),
        vec![(String::new(), vec![item])]
    );
}

/// 客户端错误应原样传播，不被同步器吞掉或改写。
#[test]
fn store_propagates_client_errors_without_masking_them() {
    let client = Arc::new(RecordingClient::default());
    *client.failure.lock().unwrap() = Some("pd unavailable".to_owned());
    let syncer = GlobalConfigSyncer::new(Some(client));

    assert_eq!(
        syncer.store_global_config(GlobalConfigItem::new("a", "b")),
        Err(GlobalConfigError::Client("pd unavailable".to_owned()))
    );
}
