// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// See the License for the specific language governing permissions and
// limitations under the License.

// Aster 迁移单元测试：Owner value 编解码、Mock 全局状态、Mock 竞选/退位与 ListenersWrapper。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serial_test::serial;
use astersql_owner::{
    Context, GetOwnerOpValue, Listener, MockGlobalState, NewListenersWrapper, NewMockManager,
    OpType,
};

/// 记录 become/retire 事件顺序的测试 Listener。
#[derive(Default)]
struct RecordingListener {
    owner: AtomicBool,
    events: Mutex<Vec<&'static str>>,
}

impl Listener for RecordingListener {
    fn OnBecomeOwner(&self) {
        self.owner.store(true, Ordering::SeqCst);
        self.events.lock().unwrap().push("become");
    }

    fn OnRetireOwner(&self) {
        self.owner.store(false, Ordering::SeqCst);
        self.events.lock().unwrap().push("retire");
    }
}

/// 核对 split/join_owner_values 与 Go 下划线分隔规则一致。
#[test]
fn owner_value_codec_matches_go() {
    assert_eq!(
        astersql_owner::split_owner_values(b"node-1"),
        (b"node-1".to_vec(), OpType::OpNone)
    );
    assert_eq!(
        astersql_owner::split_owner_values(b"node-1_\x01"),
        (b"node-1".to_vec(), OpType::OpSyncUpgradingState),
    );
    assert_eq!(
        astersql_owner::split_owner_values(b"node_\x01_extra"),
        (b"node".to_vec(), OpType::OpNone),
    );
    assert_eq!(
        astersql_owner::join_owner_values(&[b"node-1", &[OpType::OpSyncUpgradingState as u8]]),
        b"node-1_\x01",
    );
}

/// 核对 OpType Display、IsSyncedUpgradingState 与未知字节回落。
#[test]
fn op_type_matches_go_string_and_state_checks() {
    assert_eq!(OpType::OpNone.to_string(), "none");
    assert_eq!(
        OpType::OpSyncUpgradingState.to_string(),
        "sync upgrading state"
    );
    assert!(!OpType::OpNone.IsSyncedUpgradingState());
    assert!(OpType::OpSyncUpgradingState.IsSyncedUpgradingState());
    assert_eq!(OpType::from(99), OpType::OpNone);
}

/// 核对 MockGlobalState 按 store/key 隔离，以及 Set/Unset 的 CAS 语义。
#[test]
fn mock_global_state_is_scoped_and_compare_and_sets() {
    let state = MockGlobalState::default();
    let ddl = state.OwnerKey("store-1", "ddl");
    let stats = state.OwnerKey("store-1", "stats");
    let other_store = state.OwnerKey("store-2", "ddl");

    assert_eq!(ddl.GetOwner(), "");
    assert!(ddl.IsOwner(""));
    assert!(ddl.SetOwner("node-1"));
    assert!(!ddl.SetOwner("node-2"));
    assert!(ddl.IsOwner("node-1"));
    assert_eq!(stats.GetOwner(), "");
    assert_eq!(other_store.GetOwner(), "");
    assert!(!ddl.UnsetOwner("node-2"));
    assert!(ddl.UnsetOwner("node-1"));
    assert_eq!(ddl.GetOwner(), "");
}

/// 两个 MockManager 竞争同一 key：Resign 后另一节点接管并通知 Listener。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mock_managers_compete_resign_and_notify_like_go() {
    let ctx = Context::new();
    let first = NewMockManager(ctx.child_token(), "node-1", None, "/owner/key");
    let second = NewMockManager(ctx.child_token(), "node-2", None, "/owner/key");
    let first_listener = Arc::new(RecordingListener::default());
    let second_listener = Arc::new(RecordingListener::default());
    first.SetListener(first_listener.clone()).await;
    second.SetListener(second_listener.clone()).await;

    first.CampaignOwner(&[]).await.unwrap();
    second.CampaignOwner(&[]).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !first.IsOwner() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!second.IsOwner());
    assert!(first.OwnerEpoch()>0);
    assert_eq!(first.GetOwnerID(&ctx).await.unwrap(), "node-1");

    first.ResignOwner(&ctx).await.unwrap();
    assert!(
        first_listener
            .events
            .lock()
            .unwrap()
            .starts_with(&["become", "retire"])
    );
    first.CampaignCancel().await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !second.IsOwner() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(!first.IsOwner());
    assert!(second.OwnerEpoch()>0);
    assert!(second_listener.owner.load(Ordering::SeqCst));

    first.Close().await;
    second.Close().await;
}

/// 无 etcd client 时 GetOwnerOpValue / SetOwnerOpValue 走 Mock 路径。
#[tokio::test]
#[serial]
async fn mock_owner_op_value_is_initialized_and_updated() {
    let ctx = Context::new();
    let manager = NewMockManager(ctx.child_token(), "node-op", None, "/owner/op");
    assert_eq!(
        GetOwnerOpValue(&ctx, None, "/owner/op").await.unwrap(),
        OpType::OpNone
    );
    manager
        .SetOwnerOpValue(&ctx, OpType::OpSyncUpgradingState)
        .await
        .unwrap();
    assert_eq!(
        GetOwnerOpValue(&ctx, None, "/owner/op").await.unwrap(),
        OpType::OpSyncUpgradingState,
    );
    manager.Close().await;
}

/// ListenersWrapper 按输入顺序广播 become/retire。
#[test]
fn listeners_wrapper_broadcasts_in_input_order() {
    let first = Arc::new(RecordingListener::default());
    let second = Arc::new(RecordingListener::default());
    let wrapper = NewListenersWrapper(vec![first.clone(), second.clone()]);
    wrapper.OnBecomeOwner();
    assert!(first.owner.load(Ordering::SeqCst));
    assert!(second.owner.load(Ordering::SeqCst));
    wrapper.OnRetireOwner();
    assert!(!first.owner.load(Ordering::SeqCst));
    assert!(!second.owner.load(Ordering::SeqCst));
    assert_eq!(*first.events.lock().unwrap(), vec!["become", "retire"]);
    assert_eq!(*second.events.lock().unwrap(), vec!["become", "retire"]);
}
