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

// 内存 Schema 版本同步器（`MemSyncer`）的单元测试。
//
// 覆盖 owner 发布全局版本后观察通道能收到通知，以及
// 关闭/重启 mock session 后 `Done` 信号的状态翻转。

use crate::{
    Context, DDLAllSchemaVersions, DDLGlobalSchemaVersion, EtcdClient, InitialVersion,
    MemoryEtcdClient, NewEtcdSyncer, NewMemSyncer, SetCheckVersFirstWaitTime, SetMDLEnabled,
    SetMockUpdateMDLError, SyncSummary, TEST_CONFIG_LOCK,
};
use std::sync::{Arc, Barrier};
use std::time::Duration;

/// 验证全局版本发布可达观察通道，且 Restart 会重置 Done 信号。
#[test]
fn canonical_mem_syncer_publishes_global_version_and_restarts_session() {
    let _guard = TEST_CONFIG_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let syncer = NewMemSyncer();
    let context = Context::Background();
    syncer.Init(context.clone()).unwrap();
    let watch = syncer.GlobalVersionCh();
    syncer
        .OwnerUpdateGlobalVersion(context.clone(), 42)
        .unwrap();
    let response = watch.RecvTimeout(Duration::from_millis(100)).unwrap();
    assert!(response.Events.is_empty());

    // 关闭 session 后 Done 为真；Restart 后恢复为未完成。
    syncer.CloseSession();
    assert!(syncer.Done().Done());
    syncer.Restart(context).unwrap();
    assert!(!syncer.Done().Done());
}

/// Go 的 MemSyncer.Init 只重建 map/channel/session，不重置 selfSchemaVersion。
#[test]
fn mem_syncer_init_preserves_non_mdl_self_schema_version_like_go() {
    let _guard = TEST_CONFIG_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    SetMDLEnabled(false);
    let syncer = NewMemSyncer();
    let context = Context::Background();
    syncer.Init(context.clone()).unwrap();
    syncer.UpdateSelfVersion(context.clone(), 0, 42).unwrap();

    syncer.Init(context.clone()).unwrap();

    assert_eq!(
        syncer
            .WaitVersionSynced(context.WithTimeout(Duration::from_millis(20)), 0, 42, false)
            .unwrap()
            .ServerCount,
        1
    );
}

/// 对齐 Go TestSyncerSimple：初始化、watch、超时、双节点同步与关闭清理。
#[test]
fn etcd_syncer_simple_flow_matches_go() {
    let _guard = TEST_CONFIG_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    SetMDLEnabled(false);
    SetCheckVersFirstWaitTime(Duration::ZERO);
    let context = Context::Background();
    let client = Arc::new(MemoryEtcdClient::default());
    let syncer1 = NewEtcdSyncer(client.clone(), "1");
    let syncer2 = NewEtcdSyncer(client.clone(), "2");
    syncer1.Init(context.clone()).unwrap();
    syncer2.Init(context.clone()).unwrap();

    for id in ["1", "2"] {
        let response = EtcdClient::Get(
            client.as_ref(),
            &context,
            &format!("{DDLAllSchemaVersions}/{id}"),
            false,
        )
        .unwrap();
        assert_eq!(response.Kvs.len(), 1);
        assert_eq!(response.Kvs[0].Value, InitialVersion.as_bytes());
    }

    let watch = syncer1.GlobalVersionCh();
    syncer1
        .OwnerUpdateGlobalVersion(context.clone(), 123)
        .unwrap();
    let response = watch.RecvTimeout(Duration::from_secs(1)).unwrap();
    assert_eq!(response.Events.len(), 1);
    assert_eq!(response.Events[0].Kv.Key, DDLGlobalSchemaVersion.as_bytes());
    assert_eq!(response.Events[0].Kv.Value, b"123");

    assert!(
        syncer1
            .WaitVersionSynced(
                context.WithTimeout(Duration::from_millis(30)),
                0,
                123,
                false
            )
            .is_err()
    );
    syncer1
        .UpdateSelfVersion(Context::Background(), 0, 123)
        .unwrap();
    syncer2
        .UpdateSelfVersion(Context::Background(), 0, 123)
        .unwrap();
    assert_eq!(
        syncer1
            .WaitVersionSynced(context.clone(), 0, 122, false)
            .unwrap(),
        SyncSummary {
            ServerCount: 2,
            AssumedServerCount: 0
        }
    );
    assert_eq!(
        syncer1
            .WaitVersionSynced(context.clone(), 0, 123, false)
            .unwrap()
            .ServerCount,
        2
    );

    syncer1.Close();
    assert!(
        EtcdClient::Get(
            client.as_ref(),
            &context,
            &format!("{DDLAllSchemaVersions}/1"),
            false,
        )
        .unwrap()
        .Kvs
        .is_empty()
    );
    SetCheckVersFirstWaitTime(Duration::from_millis(50));
}

/// 对齐 Go TestPutKVToEtcdMono 的顺序写入、并发 CAS 冲突与取消错误边界。
#[test]
fn monotonic_put_path_preserves_values_and_context_errors() {
    let client = Arc::new(MemoryEtcdClient::default());
    let context = Context::Background();
    for value in ["1", "2", "3"] {
        EtcdClient::PutMono(client.as_ref(), &context, "testKey", value).unwrap();
    }
    assert_eq!(
        EtcdClient::Get(client.as_ref(), &context, "testKey", false)
            .unwrap()
            .Kvs[0]
            .Value,
        b"3"
    );

    let worker_count = 30;
    let start = Arc::new(Barrier::new(worker_count));
    let workers: Vec<_> = (0..worker_count)
        .map(|_| {
            let client = Arc::clone(&client);
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                EtcdClient::PutMono(client.as_ref(), &Context::Background(), "testKey", "5")
            })
        })
        .collect();
    let conflict_count = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .filter(Result::is_err)
        .count();
    assert!(
        conflict_count > 0,
        "concurrent monotonic writes must expose at least one CAS conflict"
    );

    let cancelled = Context::Background();
    cancelled.Cancel();
    assert!(EtcdClient::PutMono(client.as_ref(), &cancelled, "testKey", "5").is_err());
}

/// MemSyncer 的 MDL job 分流与 failpoint 错误保持 Go 行为。
#[test]
fn mem_syncer_mdl_versions_and_update_error_match_go() {
    let _guard = TEST_CONFIG_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    SetMDLEnabled(true);
    let syncer = NewMemSyncer();
    let context = Context::Background();
    syncer.Init(context.clone()).unwrap();
    syncer.UpdateSelfVersion(context.clone(), 7, 88).unwrap();
    assert_eq!(
        syncer
            .WaitVersionSynced(context.WithTimeout(Duration::from_millis(30)), 7, 88, false)
            .unwrap()
            .ServerCount,
        1
    );

    SetMockUpdateMDLError(true);
    assert!(
        syncer
            .UpdateSelfVersion(Context::Background(), 7, 89)
            .is_err()
    );
    SetMockUpdateMDLError(false);
    SetMDLEnabled(false);
}
