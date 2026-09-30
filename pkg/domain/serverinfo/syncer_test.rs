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
