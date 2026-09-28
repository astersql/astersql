// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// Schema 版本同步器中 `nodeVersions` 的无 kit 单元测试。
//
// 验证一次性匹配回调（once-match）：仅当全部节点版本达到目标后
// 回调返回 true 并被消费，之后再次更新不会重复触发。

use crate::{
    Context, DDLAllSchemaVersionsByJob, EtcdClient, EventType, KeyValue, MemoryEtcdClient,
    NewEtcdSyncer, SetMDLEnabled, SetMockCompaction, SetNextGen, SyncSummary, TEST_CONFIG_LOCK,
    calculateUpdatedMap, decodeJobVersionEvent, newNodeVersions,
};
use astersql_domain_serverinfo as serverinfo;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

/// 两个节点均达到版本 8 时回调只触发一次。
#[test]
fn canonical_node_versions_notifies_once_when_all_nodes_reach_target() {
    let versions = newNodeVersions(1, None);
    assert!(versions.emptyAndNotUsed());
    versions.add("a".into(), 10);
    versions.add("b".into(), 20);
    assert!(!versions.emptyAndNotUsed());
    assert_eq!(versions.len(), 2);

    let notifications = Arc::new(Mutex::new(0_usize));
    let observed = notifications.clone();
    let water_mark = Arc::new(Mutex::new(10_i64));
    let observed_water_mark = water_mark.clone();
    versions.matchOrSet(Box::new(move |nodes| {
        if nodes
            .values()
            .all(|version| *version >= *observed_water_mark.lock().unwrap())
        {
            *observed.lock().unwrap() += 1;
            true
        } else {
            false
        }
    }));
    assert!(!versions.getMatchFn());

    *water_mark.lock().unwrap() = 20;
    let observed = notifications.clone();
    let observed_water_mark = water_mark.clone();
    versions.matchOrSet(Box::new(move |nodes| {
        if nodes
            .values()
            .all(|version| *version >= *observed_water_mark.lock().unwrap())
        {
            *observed.lock().unwrap() += 1;
            true
        } else {
            false
        }
    }));
    assert!(versions.getMatchFn());
    versions.add("a".into(), 20);
    assert!(!versions.getMatchFn());
    assert_eq!(*notifications.lock().unwrap(), 2);

    versions.del("a");
    assert_eq!(versions.len(), 1);
    versions.del("b");
    assert!(versions.emptyAndNotUsed());
    versions.matchOrSet(Box::new(|_| false));
    assert!(!versions.emptyAndNotUsed());
}

/// 对齐 Go TestDecodeJobVersionEvent 的全部无效、PUT 与 DELETE 场景。
#[test]
fn decode_job_version_event_matches_go() {
    let prefix = "/tidb/ddl/all_schema_by_job_versions/";
    let event = |suffix: &str, value: &[u8]| KeyValue {
        Key: format!("{prefix}{suffix}").into_bytes(),
        Value: value.to_vec(),
        ModRevision: 0,
    };

    assert!(!decodeJobVersionEvent(&event("1", b""), EventType::PUT, prefix).3);
    assert!(!decodeJobVersionEvent(&event("a/aa", b""), EventType::PUT, prefix).3);
    assert!(!decodeJobVersionEvent(&event("1/aa", b"aa"), EventType::PUT, prefix).3);
    assert_eq!(
        decodeJobVersionEvent(&event("1/aa", b"123"), EventType::PUT, prefix),
        (1, "aa".into(), 123, true)
    );
    assert_eq!(
        decodeJobVersionEvent(&event("1/aa", b"aaaa"), EventType::DELETE, prefix),
        (1, "aa".into(), 0, true)
    );

    // strings.TrimPrefix 不会拒绝空节点 ID。
    let empty_node = KeyValue {
        Key: format!("{prefix}1/").into_bytes(),
        Value: b"123".to_vec(),
        ModRevision: 0,
    };
    assert_eq!(
        decodeJobVersionEvent(&empty_node, EventType::PUT, prefix),
        (1, String::new(), 123, true)
    );
}

/// 对齐 Go TestSyncJobSchemaVerLoop：初始扫描、watch 更新、删除与 compaction 重试。
#[test]
fn sync_job_schema_version_loop_matches_go() {
    let _guard = TEST_CONFIG_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let context = Context::Background();
    let client = Arc::new(MemoryEtcdClient::default());
    EtcdClient::Put(
        client.as_ref(),
        &context,
        &format!("{DDLAllSchemaVersionsByJob}/1/aa"),
        "123",
        None,
    )
    .unwrap();
    let syncer = NewEtcdSyncer(client.clone(), "1111");
    let loop_context = context.clone();
    let loop_syncer = syncer.clone();
    let worker = thread::spawn(move || loop_syncer.SyncJobSchemaVerLoop(loop_context));

    let (notify_tx, notify_rx) = mpsc::sync_channel(1);
    let item = syncer.jobSchemaVerMatchOrSet(
        1,
        Box::new(move |versions| {
            if versions.values().all(|version| *version >= 123) {
                let _ = notify_tx.try_send(());
                true
            } else {
                false
            }
        }),
    );
    notify_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(!item.getMatchFn());
    EtcdClient::Delete(
        client.as_ref(),
        &context,
        &format!("{DDLAllSchemaVersionsByJob}/1/aa"),
    )
    .unwrap();

    let (notify_tx, notify_rx) = mpsc::sync_channel(1);
    let item = syncer.jobSchemaVerMatchOrSet(
        2,
        Box::new(move |versions| {
            if ["aa", "bb"]
                .iter()
                .all(|id| versions.get(*id).is_some_and(|version| *version >= 123))
            {
                let _ = notify_tx.try_send(());
                true
            } else {
                false
            }
        }),
    );
    assert!(item.getMatchFn());
    for (id, version) in [("aa", "123"), ("bb", "124")] {
        EtcdClient::Put(
            client.as_ref(),
            &context,
            &format!("{DDLAllSchemaVersionsByJob}/2/{id}"),
            version,
            None,
        )
        .unwrap();
    }
    notify_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(!item.getMatchFn());
    for id in ["aa", "bb"] {
        EtcdClient::Delete(
            client.as_ref(),
            &context,
            &format!("{DDLAllSchemaVersionsByJob}/2/{id}"),
        )
        .unwrap();
    }

    SetMockCompaction(true);
    EtcdClient::Put(
        client.as_ref(),
        &context,
        &format!("{DDLAllSchemaVersionsByJob}/3/aa"),
        "123",
        None,
    )
    .unwrap();
    let (notify_tx, notify_rx) = mpsc::sync_channel(1);
    let item = syncer.jobSchemaVerMatchOrSet(
        3,
        Box::new(move |versions| {
            if versions.get("aa").is_some_and(|version| *version >= 123) {
                let _ = notify_tx.try_send(());
                true
            } else {
                false
            }
        }),
    );
    notify_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(!item.getMatchFn());
    EtcdClient::Delete(
        client.as_ref(),
        &context,
        &format!("{DDLAllSchemaVersionsByJob}/3/aa"),
    )
    .unwrap();

    // Go also exercises the public wait path after the watch loop has populated
    // the per-job versions. Use the in-memory server-info backend in place of its
    // failpoint so the inputs and result shape remain equivalent.
    SetMDLEnabled(true);
    let info_client = Arc::new(serverinfo::MemoryEtcdClient::default());
    let mut info = server("aa", "test", 1, "");
    serverinfo::EtcdClient::Put(
        info_client.as_ref(),
        &serverinfo::Context::Background(),
        &serverinfo::serverInfoKeyPath("aa"),
        info.Marshal().unwrap(),
        None,
    )
    .unwrap();
    let info_syncer: Arc<serverinfo::Syncer> = Arc::from(serverinfo::NewSyncer(
        "1".into(),
        Arc::new(|| 1),
        Some(info_client),
        Arc::new(serverinfo::NoopMinStartTSReporter),
    ));
    syncer.SetServerInfoSyncer(Some(info_syncer));
    EtcdClient::Put(
        client.as_ref(),
        &context,
        &format!("{DDLAllSchemaVersionsByJob}/4/aa"),
        "333",
        None,
    )
    .unwrap();
    assert_eq!(
        syncer.WaitVersionSynced(context.clone(), 4, 333, false),
        Ok(SyncSummary {
            ServerCount: 1,
            AssumedServerCount: 0,
        })
    );
    EtcdClient::Delete(
        client.as_ref(),
        &context,
        &format!("{DDLAllSchemaVersionsByJob}/4/aa"),
    )
    .unwrap();

    context.Cancel();
    worker.join().unwrap();
    SetMockCompaction(false);
    SetMDLEnabled(false);
}

fn server(id: &str, ip: &str, start: i64, assumed_keyspace: &str) -> serverinfo::ServerInfo {
    serverinfo::ServerInfo {
        StaticInfo: serverinfo::StaticInfo {
            ID: id.into(),
            IP: ip.into(),
            Port: 4000,
            StartTimestamp: start,
            AssumedKeyspace: assumed_keyspace.into(),
            ServerIDGetter: Some(Arc::new(|| 0)),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// 对齐 Go TestCalculateUpdatedMap：实例去重与 assumed 计数。
#[test]
fn calculate_updated_map_matches_go() {
    let map = |values: Vec<serverinfo::ServerInfo>| {
        values
            .into_iter()
            .map(|info| (info.StaticInfo.ID.clone(), info))
            .collect::<HashMap<_, _>>()
    };

    let (updated, summary) = calculateUpdatedMap(map(vec![
        server("a", "a", 0, ""),
        server("b", "b", 0, ""),
        server("c", "c", 0, ""),
    ]));
    assert_eq!(updated.len(), 3);
    assert_eq!(
        summary,
        SyncSummary {
            ServerCount: 3,
            AssumedServerCount: 0
        }
    );

    let (updated, summary) = calculateUpdatedMap(map(vec![
        server("a", "a", 0, ""),
        server("b", "b", 0, ""),
        server("c", "c", 0, "a"),
    ]));
    assert_eq!(updated.len(), 3);
    assert_eq!(summary.AssumedServerCount, 1);

    let (updated, summary) = calculateUpdatedMap(map(vec![
        server("a", "a", 100, ""),
        server("b", "a", 200, ""),
        server("c", "a", 300, "a"),
    ]));
    assert_eq!(updated.len(), 1);
    assert_eq!(
        summary,
        SyncSummary {
            ServerCount: 1,
            AssumedServerCount: 1
        }
    );

    let (updated, summary) = calculateUpdatedMap(map(vec![
        server("a", "a", 100, ""),
        server("b", "a", 200, "a"),
        server("c", "a", 300, ""),
    ]));
    assert_eq!(updated.len(), 1);
    assert_eq!(
        summary,
        SyncSummary {
            ServerCount: 1,
            AssumedServerCount: 0
        }
    );
}

/// 对齐 Go TestGetServersForISSync：next-gen 可按开关过滤 assumed 节点。
#[test]
fn get_servers_for_info_schema_sync_matches_go() {
    let _guard = TEST_CONFIG_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let info_client = Arc::new(serverinfo::MemoryEtcdClient::default());
    for mut info in [
        server("s1", "s1", 1, ""),
        server("s2", "s2", 1, ""),
        server("s3", "s3", 1, "system"),
    ] {
        let id = info.StaticInfo.ID.clone();
        serverinfo::EtcdClient::Put(
            info_client.as_ref(),
            &serverinfo::Context::Background(),
            &serverinfo::serverInfoKeyPath(&id),
            info.Marshal().unwrap(),
            None,
        )
        .unwrap();
    }
    let info_syncer: Arc<serverinfo::Syncer> = Arc::from(serverinfo::NewSyncer(
        "1".into(),
        Arc::new(|| 1),
        Some(info_client),
        Arc::new(serverinfo::NoopMinStartTSReporter),
    ));
    let syncer = NewEtcdSyncer(Arc::new(MemoryEtcdClient::default()), "ddl");
    syncer.SetServerInfoSyncer(Some(info_syncer));

    SetNextGen(false);
    let classic = syncer
        .getServersForISSync(Context::Background(), false)
        .unwrap();
    assert_eq!(classic.len(), 3);

    SetNextGen(true);
    let regular = syncer
        .getServersForISSync(Context::Background(), false)
        .unwrap();
    assert_eq!(regular.len(), 2);
    assert!(regular.values().all(|info| !info.StaticInfo.IsAssumed()));

    let all = syncer
        .getServersForISSync(Context::Background(), true)
        .unwrap();
    assert_eq!(all.len(), 3);
    SetNextGen(false);
}
