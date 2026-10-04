// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Syncer 单测：拓扑写入/重启、陈旧 ServerInfo 清理、跨 keyspace 假定身份。
//
// 通过内存 etcd 与全局 `ServerConfig` 隔离锁，验证与 Go `TestTopology` 等对齐的行为。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::{
    Context, EtcdClient, GetGlobalServerConfig, MemoryEtcdClient, NewCrossKSSyncer, NewSyncer,
    NoopMinStartTSReporter, ServerConfig, ServerInfo, SetGlobalServerConfig, StaticInfo, SyncError,
    Syncer, TopologyInformationPath, serverInfoKeyPath,
};

/// 串行化对全局 ServerConfig 的测试修改。
fn global_config_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 临时设置全局配置执行 body，结束后恢复；若 panic 则传播并仍恢复配置。
fn with_config<T>(config: ServerConfig, body: impl FnOnce() -> T) -> T {
    let _guard = global_config_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = GetGlobalServerConfig();
    SetGlobalServerConfig(config);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    SetGlobalServerConfig(previous);
    match result {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[test]
fn go_merge_43_status_endpoint_claim_respects_syncer_option() {
    with_config(ServerConfig::default(), || {
        let client = Arc::new(MemoryEtcdClient::default());
        let mut primary = crate::NewSyncerWithOptions(
            "primary".into(),
            Arc::new(|| 1),
            Some(client.clone()),
            Arc::new(NoopMinStartTSReporter),
            &[],
        );
        primary
            .NewSessionAndStoreServerInfo(Context::Background())
            .unwrap();
        assert!(
            client
                .Snapshot()
                .keys()
                .any(|key| key.starts_with("/tidb/server/status_addr/"))
        );
        let mut bootstrap = crate::NewSyncerWithOptions(
            "bootstrap".into(),
            Arc::new(|| 2),
            Some(client.clone()),
            Arc::new(NoopMinStartTSReporter),
            &[crate::SyncerOption::WithoutStatusEndpointClaim],
        );
        bootstrap
            .NewSessionAndStoreServerInfo(Context::Background())
            .unwrap();
        assert_eq!(
            client
                .Snapshot()
                .keys()
                .filter(|key| key.starts_with("/tidb/server/status_addr/"))
                .count(),
            1
        );
        let mut conflict = NewSyncer(
            "conflict".into(),
            Arc::new(|| 3),
            Some(client.clone()),
            Arc::new(NoopMinStartTSReporter),
        );
        conflict
            .NewSessionAndStoreServerInfo(Context::Background())
            .unwrap();
        assert_eq!(
            client
                .Snapshot()
                .values()
                .find(|item| item.key.starts_with("/tidb/server/status_addr/"))
                .unwrap()
                .value,
            b"primary"
        );
        conflict.RemoveServerInfo();
        assert!(
            client
                .Snapshot()
                .keys()
                .any(|key| key.starts_with("/tidb/server/status_addr/"))
        );
        primary.RemoveServerInfo();
        assert!(
            !client
                .Snapshot()
                .keys()
                .any(|key| key.starts_with("/tidb/server/status_addr/"))
        );
    });
}

#[test]
fn go_merge_43_failed_server_info_store_revokes_new_session() {
    struct FailingPutEtcd {
        inner: MemoryEtcdClient,
        revoked: AtomicUsize,
    }
    impl EtcdClient for FailingPutEtcd {
        fn Get(
            &self,
            context: &Context,
            key: &str,
            prefix: bool,
        ) -> Result<Vec<crate::KeyValue>, SyncError> {
            self.inner.Get(context, key, prefix)
        }
        fn Put(&self, _: &Context, _: &str, _: Vec<u8>, _: Option<i64>) -> Result<(), SyncError> {
            Err(SyncError("store failed".into()))
        }
        fn Delete(&self, context: &Context, key: &str) -> Result<(), SyncError> {
            self.inner.Delete(context, key)
        }
        fn DeletePrefix(&self, context: &Context, prefix: &str) -> Result<(), SyncError> {
            self.inner.DeletePrefix(context, prefix)
        }
        fn RevokeLease(&self, context: &Context, lease: i64) -> Result<(), SyncError> {
            self.revoked.fetch_add(1, Ordering::SeqCst);
            self.inner.RevokeLease(context, lease)
        }
    }

    let etcd = Arc::new(FailingPutEtcd {
        inner: MemoryEtcdClient::default(),
        revoked: AtomicUsize::new(0),
    });
    let mut syncer = NewCrossKSSyncer(
        "virtual-server".into(),
        Arc::new(|| 0),
        Some(etcd.clone()),
        Arc::new(NoopMinStartTSReporter),
        "tenant-a".into(),
    );
    assert!(
        syncer
            .NewSessionAndStoreServerInfo(Context::Background())
            .is_err()
    );
    assert!(syncer.session.as_ref().unwrap().Done());
    assert_eq!(etcd.revoked.load(Ordering::SeqCst), 1);
}

impl Syncer {
    /// 测试辅助：从 etcd 读取本节点拓扑 info。
    fn get_topology_from_etcd(&self, context: &Context) -> Result<crate::TopologyInfo, SyncError> {
        let info = self.GetLocalServerInfo();
        let key = format!(
            "{TopologyInformationPath}/{}:{}/info",
            info.StaticInfo.IP, info.StaticInfo.Port
        );
        let client = self
            .etcdCli
            .as_ref()
            .ok_or_else(|| SyncError("etcd client is not configured".into()))?;
        let resp = client.Get(context, &key, false)?;
        if resp.is_empty() {
            return Err(SyncError("not-exists".into()));
        }
        if resp.len() != 1 {
            return Err(SyncError("resp.Kvs error".into()));
        }
        crate::TopologyInfo::Unmarshal(&resp[0].value).map_err(Into::into)
    }

    /// 测试辅助：检查本节点拓扑 ttl 键是否存在。
    fn ttl_key_exists(&self, context: &Context) -> Result<bool, SyncError> {
        let info = self.GetLocalServerInfo();
        let key = format!(
            "{TopologyInformationPath}/{}:{}/ttl",
            info.StaticInfo.IP, info.StaticInfo.Port
        );
        let client = self
            .etcdCli
            .as_ref()
            .ok_or_else(|| SyncError("etcd client is not configured".into()))?;
        let resp = client.Get(context, &key, false)?;
        if resp.len() >= 2 {
            return Err(SyncError("too many arguments in resp.Kvs".into()));
        }
        Ok(resp.len() == 1)
    }
}

/// 验证拓扑写入、RestartTopology 与 updateTopologyAliveness 刷新 ttl。
/// Corresponds to Go `TestTopology`.
#[test]
fn test_topology() {
    with_config(
        ServerConfig {
            AdvertiseAddress: "127.0.0.1".into(),
            Port: 4000,
            StatusPort: 10080,
            MockServerInfo: true,
            Labels: Default::default(),
            ..ServerConfig::default()
        },
        || {
            let client = Arc::new(MemoryEtcdClient::default());
            let mut info = NewSyncer(
                "test".into(),
                Arc::new(|| 1),
                Some(client.clone()),
                Arc::new(NoopMinStartTSReporter),
            );
            let ctx = Context::Background();
            // 首次写入后应能读回固定 Mock 时间戳与标签。
            info.NewTopologySessionAndStoreServerInfo(ctx.clone())
                .unwrap();

            let topology = info.get_topology_from_etcd(&ctx).unwrap();
            assert_eq!(topology.StartTimestamp, 1_282_967_700);
            assert_eq!(topology.Labels.get("foo").map(String::as_str), Some("bar"));
            let self_info = info.GetLocalServerInfo();
            assert_eq!(self_info.ToTopologyInfo(), topology);

            let non_ttl_key = format!(
                "{TopologyInformationPath}/{}:{}/info",
                self_info.StaticInfo.IP, self_info.StaticInfo.Port
            );
            let ttl_key = format!(
                "{TopologyInformationPath}/{}:{}/ttl",
                self_info.StaticInfo.IP, self_info.StaticInfo.Port
            );
            // 删除 info 后 RestartTopology 应重新写入。
            client.Delete(&ctx, &non_ttl_key).unwrap();

            info.RestartTopology(ctx.clone()).unwrap();
            let topology = info.get_topology_from_etcd(&ctx).unwrap();
            let deploy_path = std::env::current_exe()
                .ok()
                .and_then(|path| {
                    path.parent()
                        .map(|parent| parent.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| ".".into());
            assert_eq!(topology.DeployPath, deploy_path);
            assert_eq!(topology.StartTimestamp, 1_282_967_700);
            assert_eq!(info.GetLocalServerInfo().ToTopologyInfo(), topology);

            // 删除 ttl 后 updateTopologyAliveness 应重建。
            assert!(info.ttl_key_exists(&ctx).unwrap());
            client.Delete(&ctx, &ttl_key).unwrap();
            info.updateTopologyAliveness(ctx.clone()).unwrap();
            assert!(info.ttl_key_exists(&ctx).unwrap());
        },
    );
}

/// 验证同 IP:Port 陈旧节点与其 DDL owner 键被清理，其他节点不受影响。
/// Corresponds to Go `TestCleanupStaleServerAndOwnerInfo`.
#[test]
fn test_cleanup_stale_server_and_owner_info() {
    with_config(
        ServerConfig {
            AdvertiseAddress: "1.1.1.1".into(),
            Port: 4000,
            ..ServerConfig::default()
        },
        || {
            let client = Arc::new(MemoryEtcdClient::default());
            let ctx = Context::Background();

            // 预置同地址陈旧 ServerInfo 与 owner 键。
            let stale_id = "stale-uuid-old";
            let mut stale_info = ServerInfo {
                StaticInfo: StaticInfo {
                    ID: stale_id.into(),
                    IP: "1.1.1.1".into(),
                    Port: 4000,
                    ServerIDGetter: Some(Arc::new(|| 0)),
                    ..StaticInfo::default()
                },
                ..ServerInfo::default()
            };
            let stale_info_buf = stale_info.Marshal().unwrap();
            let stale_info_path = serverInfoKeyPath(stale_id);
            client
                .Put(&ctx, &stale_info_path, stale_info_buf, None)
                .unwrap();

            // Go's owner value may carry an operation suffix. Cleanup must scan only
            // the DDL owner election prefix, decode that suffix, and stop after the
            // first matching owner key.
            let stale_owner_key = "/tidb/ddl/fg/owner/12345";
            client
                .Put(
                    &ctx,
                    stale_owner_key,
                    format!("{stale_id}_1").into_bytes(),
                    None,
                )
                .unwrap();
            let second_stale_owner_key = "/tidb/ddl/fg/owner/23456";
            client
                .Put(
                    &ctx,
                    second_stale_owner_key,
                    format!("{stale_id}_2").into_bytes(),
                    None,
                )
                .unwrap();
            let unrelated_ddl_key = "/tidb/ddl/unrelated";
            client
                .Put(&ctx, unrelated_ddl_key, stale_id.as_bytes().to_vec(), None)
                .unwrap();

            let other_id = "other-uuid";
            let mut other_info = ServerInfo {
                StaticInfo: StaticInfo {
                    ID: other_id.into(),
                    IP: "2.2.2.2".into(),
                    Port: 4000,
                    ServerIDGetter: Some(Arc::new(|| 0)),
                    ..StaticInfo::default()
                },
                ..ServerInfo::default()
            };
            let other_info_buf = other_info.Marshal().unwrap();
            let other_info_path = serverInfoKeyPath(other_id);
            client
                .Put(&ctx, &other_info_path, other_info_buf, None)
                .unwrap();

            let new_id = "new-uuid";
            let mut syncer = NewSyncer(
                new_id.into(),
                Arc::new(|| 1),
                Some(client.clone()),
                Arc::new(NoopMinStartTSReporter),
            );
            let new_info = syncer.GetLocalServerInfo();
            assert_eq!(new_info.StaticInfo.IP, "1.1.1.1");
            assert_eq!(new_info.StaticInfo.Port, 4000);
            // 注册新节点应触发陈旧清理。
            syncer.NewSessionAndStoreServerInfo(ctx.clone()).unwrap();

            assert!(
                client
                    .Get(&ctx, &stale_info_path, false)
                    .unwrap()
                    .is_empty(),
                "stale server info should have been deleted"
            );
            assert!(
                client.Get(&ctx, stale_owner_key, false).unwrap().is_empty(),
                "stale DDL owner key should have been deleted"
            );
            assert_eq!(
                client
                    .Get(&ctx, second_stale_owner_key, false)
                    .unwrap()
                    .len(),
                1,
                "cleanup should stop after the first matching owner key"
            );
            assert_eq!(
                client.Get(&ctx, unrelated_ddl_key, false).unwrap().len(),
                1,
                "cleanup must not scan unrelated DDL keys"
            );
            assert_eq!(
                client.Get(&ctx, &other_info_path, false).unwrap().len(),
                1,
                "other node's server info should not be deleted"
            );
            let new_info_path = serverInfoKeyPath(new_id);
            assert_eq!(
                client.Get(&ctx, &new_info_path, false).unwrap().len(),
                1,
                "new server info should be registered"
            );
        },
    );
}

/// 验证普通 Syncer 与 CrossKS Syncer 的 AssumedKeyspace / Keyspace 字段。
/// Corresponds to Go `TestAssumedServerInfoSyncer`.
#[test]
fn test_assumed_server_info_syncer() {
    with_config(
        ServerConfig {
            Keyspace: "SYSTEM".into(),
            ..ServerConfig::default()
        },
        || {
            let syncer = NewSyncer(
                "1".into(),
                Arc::new(|| 1),
                None,
                Arc::new(NoopMinStartTSReporter),
            );
            let info = syncer.GetLocalServerInfo();
            assert!(!info.StaticInfo.IsAssumed());
            assert!(info.StaticInfo.AssumedKeyspace.is_empty());
            assert_eq!(info.StaticInfo.Keyspace, "SYSTEM");

            let syncer = NewCrossKSSyncer(
                "1".into(),
                Arc::new(|| 1),
                None,
                Arc::new(NoopMinStartTSReporter),
                "ks1".into(),
            );
            let info = syncer.GetLocalServerInfo();
            assert!(info.StaticInfo.IsAssumed());
            assert_eq!(info.StaticInfo.AssumedKeyspace, "ks1");
            assert_eq!(info.StaticInfo.Keyspace, "SYSTEM");
            assert!(syncer.statusEndpointClaimKey.is_none());
        },
    );
}

#[test]
fn normal_schema_barrier_transport_cancellation_and_parent_deadline() {
    use std::time::Duration;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for deadline in [false, true] {
        let parent = Context::Background();
        let context = if deadline {
            parent.WithTimeout(Duration::from_millis(30))
        } else {
            parent.clone()
        };
        let context = context.WithTimeout(Duration::from_secs(2));
        let cancel = parent.clone();
        let rpc = async move {
            if !deadline {
                cancel.Cancel();
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
            Ok::<(), etcd_client::Error>(())
        };
        let start = std::time::Instant::now();
        assert!(crate::real_etcd::run_with_context(&runtime, &context, rpc).is_err());
        assert!(
            start.elapsed() < Duration::from_millis(200),
            "network future outlived its parent"
        );
    }
}

#[test]
fn normal_schema_barrier_nested_timeout_cannot_extend_parent() {
    use std::time::Duration;
    let expired = Context::Background().WithTimeout(Duration::ZERO);
    assert!(expired.WithTimeout(Duration::from_secs(2)).Done());
}

#[test]
fn status_endpoint_hostname_removes_only_one_trailing_dot() {
    with_config(
        ServerConfig {
            AdvertiseAddress: "Host..".into(),
            ..ServerConfig::default()
        },
        || {
            use base64::Engine;
            let syncer = NewSyncer(
                "host".into(),
                Arc::new(|| 1),
                None,
                Arc::new(NoopMinStartTSReporter),
            );
            assert_eq!(
                syncer.statusEndpointClaimKey,
                Some(format!(
                    "/tidb/server/status_addr/{}",
                    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"host.:10080")
                ))
            );
        },
    );
}

#[test]
fn status_endpoint_normalization_matches_go_cases() {
    use crate::build_status_endpoint_claim;
    use base64::Engine;
    let cases = [
        (" 127.0.0.1 ", 10080, true, "", "127.0.0.1:10080"),
        (
            "2001:0db8:0000:0000:0000:0000:0000:0001",
            10080,
            true,
            "",
            "[2001:db8::1]:10080",
        ),
        ("2001:db8::1", 10080, true, "", "[2001:db8::1]:10080"),
        ("DB.Example.COM.", 10080, true, "", "db.example.com:10080"),
        (
            "db-b.example.com",
            10080,
            true,
            "",
            "db-b.example.com:10080",
        ),
        ("db.example.com", 10081, true, "", "db.example.com:10081"),
        ("db/name", 10080, true, "", "db/name:10080"),
        ("127.0.0.1", 10080, false, "", ""),
        ("127.0.0.1", 10080, true, "ks1", ""),
        ("", 10080, true, "", ""),
        (".", 10080, true, "", ""),
        ("127.0.0.1", 0, true, "", "127.0.0.1:10080"),
    ];
    for (host, port, enabled, assumed, expected) in cases {
        let info = ServerInfo {
            StaticInfo: StaticInfo {
                IP: host.into(),
                StatusPort: port,
                AssumedKeyspace: assumed.into(),
                ..StaticInfo::default()
            },
            ..ServerInfo::default()
        };
        let (endpoint, key) = build_status_endpoint_claim(&info, enabled);
        assert_eq!(endpoint, expected);
        assert_eq!(
            key,
            if expected.is_empty() {
                String::new()
            } else {
                format!(
                    "/tidb/server/status_addr/{}",
                    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(expected.as_bytes())
                )
            }
        );
        if !key.is_empty() {
            assert_eq!(key.matches('/').count(), 4);
        }
    }
}

#[derive(Default)]
struct ClaimFaultClient {
    inner: MemoryEtcdClient,
    create_faults: Mutex<std::collections::VecDeque<&'static str>>,
    reattach_change: Mutex<Option<(String, i64)>>,
    fail_put: bool,
    wait_remove: bool,
    fail_revoke: bool,
    transactions: AtomicUsize,
    revocations: AtomicUsize,
    revoke_had_deadline: std::sync::atomic::AtomicBool,
}
impl EtcdClient for ClaimFaultClient {
    fn Get(&self, c: &Context, k: &str, p: bool) -> Result<Vec<crate::KeyValue>, SyncError> {
        self.inner.Get(c, k, p)
    }
    fn Put(&self, c: &Context, k: &str, v: Vec<u8>, l: Option<i64>) -> Result<(), SyncError> {
        if self.fail_put {
            Err(SyncError("store failed".into()))
        } else {
            self.inner.Put(c, k, v, l)
        }
    }
    fn Delete(&self, c: &Context, k: &str) -> Result<(), SyncError> {
        self.inner.Delete(c, k)
    }
    fn DeletePrefix(&self, c: &Context, k: &str) -> Result<(), SyncError> {
        self.inner.DeletePrefix(c, k)
    }
    fn RevokeLease(&self, c: &Context, l: i64) -> Result<(), SyncError> {
        self.revocations.fetch_add(1, Ordering::SeqCst);
        self.revoke_had_deadline
            .store(c.HasDeadline(), Ordering::SeqCst);
        if self.fail_revoke {
            Err(SyncError("revoke failed".into()))
        } else {
            self.inner.RevokeLease(c, l)
        }
    }
    fn TryCreateClaim(
        &self,
        c: &Context,
        k: &str,
        id: &str,
        l: i64,
    ) -> Result<(bool, crate::ObservedStatusEndpointClaim), SyncError> {
        self.transactions.fetch_add(1, Ordering::SeqCst);
        let fault = self.create_faults.lock().unwrap().pop_front();
        if fault == Some("before") {
            return Err(SyncError("claim fault".into()));
        }
        if fault == Some("wait") {
            while !c.Done() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            return Err(SyncError("claim deadline".into()));
        }
        let result = self.inner.TryCreateClaim(c, k, id, l)?;
        if fault == Some("after") {
            Err(SyncError("claim fault".into()))
        } else {
            Ok(result)
        }
    }
    fn ReattachClaim(
        &self,
        c: &Context,
        k: &str,
        o: &crate::ObservedStatusEndpointClaim,
        l: i64,
    ) -> Result<bool, SyncError> {
        self.transactions.fetch_add(1, Ordering::SeqCst);
        if let Some((id, lease)) = self.reattach_change.lock().unwrap().take() {
            if id.is_empty() {
                self.inner.Delete(c, k)?;
            } else {
                self.inner.Put(c, k, id.into_bytes(), Some(lease))?;
            }
        }
        self.inner.ReattachClaim(c, k, o, l)
    }
    fn CompareAndDelete(&self, c: &Context, k: &str, e: (&[u8], i64)) -> Result<bool, SyncError> {
        self.transactions.fetch_add(1, Ordering::SeqCst);
        if self.wait_remove {
            while !c.Done() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            return Err(SyncError("cleanup deadline".into()));
        }
        self.inner.CompareAndDelete(c, k, e)
    }
}
fn test_claim<'a>(
    client: &'a dyn EtcdClient,
    id: &str,
    key: &str,
) -> crate::StatusEndpointClaim<'a> {
    crate::StatusEndpointClaim {
        client,
        endpoint: "127.0.0.1:10080".into(),
        key: key.into(),
        local_id: id.into(),
    }
}
#[test]
fn status_endpoint_atomic_competition_restart_and_deletion() {
    use crate::EndpointClaimState::*;
    let client = Arc::new(MemoryEtcdClient::default());
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let handles: Vec<_> = (1..=2)
        .map(|n| {
            let client = client.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                test_claim(client.as_ref(), &format!("server-{n}"), "claim")
                    .acquire(&Context::Background(), n)
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.state == Acquired).count(), 1);
    let loser = results.iter().find(|r| r.state == Conflict).unwrap();
    let winner = results.iter().find(|r| r.state == Acquired).unwrap();
    assert_eq!(loser.existing_id, winner.local_id);
    let warning = loser.warning("ks").unwrap();
    assert!(warning.contains(&format!("existing-lease-id={:016x}", loser.existing_lease)));
    for field in [
        "advertised-status-endpoint=",
        "claim-key=",
        "local-server-info-id=",
        "existing-server-info-id=",
        "existing-lease-id=",
        "keyspace=ks",
        "action=",
    ] {
        assert!(warning.contains(field));
    }
    assert!(winner.warning("").is_none());
    let old_lease = client.Snapshot()["claim"].lease.unwrap();
    let claim = test_claim(client.as_ref(), &winner.local_id, "claim");
    assert_eq!(claim.acquire(&Context::Background(), 3).state, Acquired);
    assert!(
        !client
            .CompareAndDelete(
                &Context::Background(),
                "claim",
                (winner.local_id.as_bytes(), old_lease)
            )
            .unwrap()
    );
    assert!(
        !client
            .CompareAndDelete(
                &Context::Background(),
                "claim",
                (loser.local_id.as_bytes(), 3)
            )
            .unwrap()
    );
    client
        .RevokeLease(&Context::Background(), old_lease)
        .unwrap();
    assert_eq!(client.Snapshot()["claim"].lease, Some(3));
    assert!(
        client
            .CompareAndDelete(
                &Context::Background(),
                "claim",
                (winner.local_id.as_bytes(), 3)
            )
            .unwrap()
    );
    assert_eq!(
        test_claim(client.as_ref(), "replacement", "claim")
            .acquire(&Context::Background(), 4)
            .state,
        Acquired
    );
    assert_eq!(
        test_claim(client.as_ref(), "independent", "other")
            .acquire(&Context::Background(), 5)
            .state,
        Acquired
    );
}
#[test]
fn status_endpoint_revision_races_retry_without_overwrite() {
    use crate::EndpointClaimState::*;
    for (new_id, expected) in [("same", CheckFailed), ("other", Conflict), ("", Acquired)] {
        let client = ClaimFaultClient::default();
        client
            .inner
            .Put(&Context::Background(), "claim", b"same".to_vec(), Some(1))
            .unwrap();
        *client.reattach_change.lock().unwrap() = Some((new_id.into(), 1));
        let result = test_claim(&client, "same", "claim").acquire(&Context::Background(), 2);
        assert_eq!(result.state, expected);
        assert_eq!(client.transactions.load(Ordering::SeqCst), 3);
        if expected == CheckFailed {
            assert!(
                result
                    .error
                    .unwrap()
                    .0
                    .contains("claim changed while reattaching")
            );
            assert_eq!(client.inner.Snapshot()["claim"].lease, Some(1));
        } else if expected == Conflict {
            assert_eq!(result.existing_id, "other");
        } else {
            assert_eq!(client.inner.Snapshot()["claim"].lease, Some(2));
        }
    }
}
#[test]
fn status_endpoint_failures_and_parent_cancellation_report_correctly() {
    use crate::EndpointClaimState::*;
    let client = ClaimFaultClient::default();
    client.create_faults.lock().unwrap().push_back("before");
    let claim = test_claim(&client, "id", "claim");
    let result = claim.acquire(&Context::Background(), 1);
    assert_eq!(result.state, CheckFailed);
    assert!(
        result
            .warning("")
            .unwrap()
            .contains("check etcd connectivity")
    );
    let cancelled = Context::Background();
    cancelled.Cancel();
    assert!(
        claim
            .try_acquire_and_report(&cancelled, 1, |_| panic!(
                "cancelled attempt must not report"
            ))
            .is_none()
    );
    assert_eq!(
        test_claim(&client, "id", "")
            .acquire(&Context::Background(), 1)
            .state,
        Skipped
    );
    client.create_faults.lock().unwrap().push_back("wait");
    let result = claim.acquire(
        &Context::Background().WithTimeout(std::time::Duration::from_millis(10)),
        1,
    );
    assert_eq!(result.state, CheckFailed);
}
#[test]
fn status_endpoint_registration_failure_cleans_unknown_claim_and_preserves_winner() {
    with_config(ServerConfig::default(), || {
        for conflict in [false, true] {
            let client = Arc::new(ClaimFaultClient {
                fail_put: true,
                ..ClaimFaultClient::default()
            });
            let mut syncer = NewSyncer(
                "failed".into(),
                Arc::new(|| 1),
                Some(client.clone()),
                Arc::new(NoopMinStartTSReporter),
            );
            let key = syncer.statusEndpointClaimKey.clone().unwrap();
            if conflict {
                client
                    .inner
                    .Put(&Context::Background(), &key, b"winner".to_vec(), Some(99))
                    .unwrap();
            } else {
                client.create_faults.lock().unwrap().push_back("after");
            }
            assert_eq!(
                syncer.NewSessionAndStoreServerInfo(Context::Background()),
                Err(SyncError("store failed".into()))
            );
            assert!(syncer.session.as_ref().unwrap().Done());
            assert_eq!(client.revocations.load(Ordering::SeqCst), 1);
            assert!(client.revoke_had_deadline.load(Ordering::SeqCst));
            assert!(!client.inner.Snapshot().contains_key(&syncer.serverInfoPath));
            if conflict {
                assert_eq!(client.inner.Snapshot()[&key].value, b"winner");
            } else {
                assert!(!client.inner.Snapshot().contains_key(&key));
            }
        }
    });
}
#[test]
fn status_endpoint_failed_cleanup_is_bounded_and_keeps_store_error() {
    with_config(ServerConfig::default(), || {
        let client = Arc::new(ClaimFaultClient {
            fail_put: true,
            wait_remove: true,
            fail_revoke: true,
            ..ClaimFaultClient::default()
        });
        let mut syncer = NewSyncer(
            "failed".into(),
            Arc::new(|| 1),
            Some(client.clone()),
            Arc::new(NoopMinStartTSReporter),
        );
        let start = std::time::Instant::now();
        assert_eq!(
            syncer.NewSessionAndStoreServerInfo(Context::Background()),
            Err(SyncError("store failed".into()))
        );
        assert!(start.elapsed() >= std::time::Duration::from_millis(900));
        assert!(start.elapsed() < std::time::Duration::from_secs(10));
        assert!(syncer.session.as_ref().unwrap().Done());
        assert_eq!(client.transactions.load(Ordering::SeqCst), 2);
        assert_eq!(client.revocations.load(Ordering::SeqCst), 1);
        assert!(client.revoke_had_deadline.load(Ordering::SeqCst));
        assert!(
            client
                .inner
                .Snapshot()
                .contains_key(syncer.statusEndpointClaimKey.as_ref().unwrap())
        );
    });
}

#[test]
fn status_endpoint_registration_disabled_assumed_default_and_shutdown() {
    with_config(ServerConfig::default(), || {
        let client = Arc::new(MemoryEtcdClient::default());
        let mut nil = NewSyncer(
            "nil".into(),
            Arc::new(|| 1),
            None,
            Arc::new(NoopMinStartTSReporter),
        );
        nil.NewSessionAndStoreServerInfo(Context::Background())
            .unwrap();
        assert!(nil.session.is_none());
        assert!(nil.tryClaimStatusEndpoint(&Context::Background()).is_none());
        let mut assumed = NewCrossKSSyncer(
            "assumed".into(),
            Arc::new(|| 1),
            Some(client.clone()),
            Arc::new(NoopMinStartTSReporter),
            "ks".into(),
        );
        assumed
            .NewSessionAndStoreServerInfo(Context::Background())
            .unwrap();
        assert!(assumed.statusEndpointClaimKey.is_none());
        assert_eq!(
            assumed
                .tryClaimStatusEndpoint(&Context::Background())
                .unwrap()
                .state,
            crate::EndpointClaimState::Skipped
        );
        assert!(client.Snapshot().contains_key(&assumed.serverInfoPath));
        let original = astersql_config::get_global_config();
        astersql_config::update_global(|c| c.status.report_status = false);
        let mut disabled = NewSyncer(
            "disabled".into(),
            Arc::new(|| 1),
            Some(client.clone()),
            Arc::new(NoopMinStartTSReporter),
        );
        astersql_config::store_global_config(original.as_ref().clone());
        disabled
            .NewSessionAndStoreServerInfo(Context::Background())
            .unwrap();
        assert!(disabled.statusEndpointClaimKey.is_none());
        assert_eq!(
            disabled
                .tryClaimStatusEndpoint(&Context::Background())
                .unwrap()
                .state,
            crate::EndpointClaimState::Skipped
        );
        assert!(client.Snapshot().contains_key(&disabled.serverInfoPath));
        SetGlobalServerConfig(ServerConfig {
            StatusPort: 0,
            ..ServerConfig::default()
        });
        let mut primary = NewSyncer(
            "primary".into(),
            Arc::new(|| 1),
            Some(client.clone()),
            Arc::new(NoopMinStartTSReporter),
        );
        primary
            .NewSessionAndStoreServerInfo(Context::Background())
            .unwrap();
        assert_eq!(primary.GetLocalServerInfo().StaticInfo.StatusPort, 0);
        assert_eq!(
            primary
                .tryClaimStatusEndpoint(&Context::Background())
                .unwrap()
                .endpoint,
            "127.0.0.1:10080"
        );
        let lease = primary.session.as_ref().unwrap().Lease();
        primary.session.as_ref().unwrap().Close();
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(()).unwrap();
        struct Store;
        primary.ServerInfoSyncLoop(&Store, rx);
        assert_eq!(primary.session.as_ref().unwrap().Lease(), lease);
        primary.RevokeSession();
        assert!(
            !client
                .Snapshot()
                .contains_key(primary.statusEndpointClaimKey.as_ref().unwrap())
        );
        assert!(!client.Snapshot().contains_key(&primary.serverInfoPath));
    });
}

#[test]
#[ignore = "requires isolated etcd fixture in ASTERSQL_TEST_ETCD_ENDPOINT; run with --include-ignored"]
fn status_endpoint_real_etcd_transactions_namespace_and_lease_cleanup() {
    let endpoint = std::env::var("ASTERSQL_TEST_ETCD_ENDPOINT").expect("etcd fixture endpoint");
    with_config(ServerConfig::default(), || {
        let a = Arc::new(
            crate::RealEtcdClient::connect(vec![endpoint.clone()], None)
                .unwrap()
                .with_namespace("/claim-test/a/".into()),
        );
        let b = Arc::new(
            crate::RealEtcdClient::connect(vec![endpoint], None)
                .unwrap()
                .with_namespace("/claim-test/b/".into()),
        );
        let mut first = NewSyncer(
            "first".into(),
            Arc::new(|| 1),
            Some(a.clone()),
            Arc::new(NoopMinStartTSReporter),
        );
        first
            .NewSessionAndStoreServerInfo(Context::Background())
            .unwrap();
        let mut independent = NewSyncer(
            "independent".into(),
            Arc::new(|| 2),
            Some(b.clone()),
            Arc::new(NoopMinStartTSReporter),
        );
        independent
            .NewSessionAndStoreServerInfo(Context::Background())
            .unwrap();
        let key = first.statusEndpointClaimKey.clone().unwrap();
        assert_eq!(
            a.Get(&Context::Background(), &key, false).unwrap()[0].value,
            b"first"
        );
        assert_eq!(
            b.Get(&Context::Background(), &key, false).unwrap()[0].value,
            b"independent"
        );
        SetGlobalServerConfig(ServerConfig {
            Port: 4001,
            ..ServerConfig::default()
        });
        let mut bootstrap = crate::NewSyncerWithOptions(
            "bootstrap".into(),
            Arc::new(|| 3),
            Some(a.clone()),
            Arc::new(NoopMinStartTSReporter),
            &[crate::SyncerOption::WithoutStatusEndpointClaim],
        );
        bootstrap
            .NewSessionAndStoreServerInfo(Context::Background())
            .unwrap();
        assert!(bootstrap.statusEndpointClaimKey.is_none());
        bootstrap.RemoveServerInfo();
        bootstrap.RevokeSession();
        let mut conflict = NewSyncer(
            "loser".into(),
            Arc::new(|| 3),
            Some(a.clone()),
            Arc::new(NoopMinStartTSReporter),
        );
        conflict
            .NewSessionAndStoreServerInfo(Context::Background())
            .unwrap();
        let warnings: Vec<_> = astersql_util_logutil::log::BgLogger()
            .entries()
            .into_iter()
            .filter(|e| {
                e.message == "advertised status endpoint already has an active claim"
                    && e.fields
                        .contains(&astersql_util_logutil::log::LogField::String(
                            "local-server-info-id".into(),
                            "loser".into(),
                        ))
            })
            .collect();
        assert_eq!(warnings.len(), 1);
        assert_eq!(
            warnings[0].level,
            astersql_util_logutil::log::LogLevel::Warn
        );
        assert!(
            warnings[0]
                .fields
                .contains(&astersql_util_logutil::log::LogField::String(
                    "existing-server-info-id".into(),
                    "first".into()
                ))
        );
        assert_eq!(
            a.Get(&Context::Background(), &first.serverInfoPath, false)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            a.Get(&Context::Background(), &conflict.serverInfoPath, false)
                .unwrap()
                .len(),
            1
        );
        let result = conflict
            .tryClaimStatusEndpoint(&Context::Background())
            .unwrap();
        assert_eq!(result.state, crate::EndpointClaimState::Conflict);
        assert_eq!(result.existing_id, "first");
        conflict.RemoveServerInfo();
        conflict.RevokeSession();
        let old = first.session.clone().unwrap();
        first.Restart(Context::Background()).unwrap();
        let new_lease = first.session.as_ref().unwrap().Lease();
        assert_ne!(old.Lease(), new_lease);
        assert_eq!(
            a.Get(&Context::Background(), &key, false).unwrap()[0].lease,
            Some(new_lease)
        );
        assert!(
            !a.CompareAndDelete(&Context::Background(), &key, (b"first", old.Lease()))
                .unwrap()
        );
        a.RevokeLease(&Context::Background(), old.Lease()).unwrap();
        // Same value and lease with a newer modRevision must fail the reattachment CAS.
        let (_, observed) = a
            .TryCreateClaim(&Context::Background(), &key, "first", new_lease)
            .unwrap();
        a.Put(
            &Context::Background(),
            &key,
            b"first".to_vec(),
            Some(new_lease),
        )
        .unwrap();
        assert!(
            !a.ReattachClaim(&Context::Background(), &key, &observed, new_lease)
                .unwrap()
        );
        first.RevokeSession();
        assert!(
            a.Get(&Context::Background(), &key, false)
                .unwrap()
                .is_empty()
        );
        assert!(
            a.Get(&Context::Background(), &first.serverInfoPath, false)
                .unwrap()
                .is_empty()
        );
        independent.RemoveServerInfo();
        independent.RevokeSession();
        assert!(
            b.Get(&Context::Background(), &key, false)
                .unwrap()
                .is_empty()
        );
    });
}

#[test]
fn status_endpoint_concurrent_registrations_preserve_both_server_infos() {
    use astersql_util_logutil::log::{BgLogger, LogField};
    with_config(ServerConfig::default(), || {
        let client = Arc::new(MemoryEtcdClient::default());
        let first = NewSyncer(
            "concurrent-reg-a".into(),
            Arc::new(|| 1),
            Some(client.clone()),
            Arc::new(NoopMinStartTSReporter),
        );
        SetGlobalServerConfig(ServerConfig {
            Port: 4001,
            ..ServerConfig::default()
        });
        let second = NewSyncer(
            "concurrent-reg-b".into(),
            Arc::new(|| 2),
            Some(client.clone()),
            Arc::new(NoopMinStartTSReporter),
        );
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let handles: Vec<_> = [first, second]
            .into_iter()
            .map(|mut syncer| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    syncer
                        .NewSessionAndStoreServerInfo(Context::Background())
                        .unwrap();
                    syncer
                })
            })
            .collect();
        barrier.wait();
        let syncers: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let snapshot = client.Snapshot();
        for syncer in &syncers {
            assert!(snapshot.contains_key(&syncer.serverInfoPath));
        }
        let key = syncers[0].statusEndpointClaimKey.as_ref().unwrap();
        let owner = std::str::from_utf8(&snapshot[key].value).unwrap();
        let winner = syncers
            .iter()
            .find(|s| s.GetLocalServerInfo().StaticInfo.ID == owner)
            .unwrap();
        let loser = syncers
            .iter()
            .find(|s| s.GetLocalServerInfo().StaticInfo.ID != owner)
            .unwrap();
        assert_eq!(
            snapshot[key].lease,
            Some(winner.session.as_ref().unwrap().Lease())
        );
        let warnings: Vec<_>=BgLogger().entries().into_iter().filter(|e| e.message=="advertised status endpoint already has an active claim" && e.fields.iter().any(|f| matches!(f, LogField::String(k,v) if k=="local-server-info-id" && v.starts_with("concurrent-reg-")))).collect();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].fields.contains(&LogField::String(
            "existing-server-info-id".into(),
            owner.into()
        )));
        loser.RemoveServerInfo();
        assert_eq!(client.Snapshot()[key].value, owner.as_bytes());
        winner.RemoveServerInfo();
        assert!(!client.Snapshot().contains_key(key));
    });
}
