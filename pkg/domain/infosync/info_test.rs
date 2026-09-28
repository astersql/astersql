// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// InfoSyncer 相关单元测试。
//
// 覆盖 Bundle 写入重试、TiFlash 管理与列存熔断、ServerInfo JSON 编解码，
// 以及 Keyspace 配置更新在有/无 PD 客户端与错误传播下的行为。

use crate::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread;
use std::time::Duration;

/// 串行化依赖全局 InfoSyncer 的测试，避免并发互相覆盖。
pub(crate) fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}
/// 初始化全局 InfoSyncer；`client` 为 None 时使用 mock 子管理器。
fn init(client: Option<Arc<dyn PdHttpClient>>) -> Arc<InfoSyncer> {
    GlobalInfoSyncerInit(
        "test".into(),
        Arc::new(|| 1),
        None,
        None,
        client,
        Codec::default(),
        false,
        None,
    )
    .unwrap()
}

#[derive(Default)]
/// 可配置失败次数的 PD HTTP mock，用于验证 Bundle 重试逻辑。
struct RetryClient {
    /// 剩余应失败次数。
    failures: Mutex<usize>,
    /// 为 true 时返回 DomainService（不可重试）错误。
    service: bool,
    /// 实际调用次数计数。
    attempts: Mutex<usize>,
    /// 成功写入后保存的 Bundle。
    bundles: Mutex<HashMap<String, placement::Bundle>>,
}
impl PdHttpClient for RetryClient {
    fn set_placement_rule_bundles(&self, bundles: &[placement::Bundle], _: bool) -> Result<()> {
        *self.attempts.lock().unwrap() += 1;
        let mut failures = self.failures.lock().unwrap();
        if *failures > 0 {
            *failures -= 1;
            return if self.service {
                Err(Error::DomainService("mock service error".into()))
            } else {
                Err(Error::External("mock other error".into()))
            };
        }
        for bundle in bundles {
            self.bundles
                .lock()
                .unwrap()
                .insert(bundle.ID.clone(), bundle.clone());
        }
        Ok(())
    }
    fn get_placement_rule_bundle(&self, name: &str) -> Result<placement::Bundle> {
        Ok(self
            .bundles
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .unwrap_or_default())
    }
    fn get_all_placement_rule_bundles(&self) -> Result<Vec<placement::Bundle>> {
        Ok(self.bundles.lock().unwrap().values().cloned().collect())
    }
}

#[test]
/// DomainService 不重试；可恢复错误会重试直至成功或耗尽。
fn test_put_bundles_retry() {
    let _guard = serial();
    let bundle = placement::Bundle {
        ID: "bundle-1".into(),
        Index: 1,
        ..Default::default()
    };

    // 领域服务错误：立即失败，只尝试 1 次。
    let service = Arc::new(RetryClient {
        failures: Mutex::new(1),
        service: true,
        ..Default::default()
    });
    init(Some(service.clone()));
    assert!(matches!(
        PutRuleBundlesWithRetry(&[bundle.clone()], 3, Duration::ZERO),
        Err(Error::DomainService(_))
    ));
    assert_eq!(*service.attempts.lock().unwrap(), 1);

    // 瞬时外部错误：重试后成功（1 次失败 + 最多 3 次重试 = 4 次）。
    let transient = Arc::new(RetryClient {
        failures: Mutex::new(3),
        ..Default::default()
    });
    init(Some(transient.clone()));
    PutRuleBundlesWithRetry(&[bundle.clone()], 3, Duration::ZERO).unwrap();
    assert_eq!(*transient.attempts.lock().unwrap(), 4);
    assert_eq!(GetRuleBundle(&bundle.ID).unwrap(), bundle);

    // 失败次数超过 maxRetry：最终仍失败。
    let exhausted = Arc::new(RetryClient {
        failures: Mutex::new(4),
        ..Default::default()
    });
    init(Some(exhausted.clone()));
    assert_eq!(
        PutRuleBundlesWithRetry(&[bundle], 3, Duration::ZERO)
            .unwrap_err()
            .to_string(),
        "mock other error"
    );
    assert_eq!(*exhausted.attempts.lock().unwrap(), 4);
}

#[test]
/// TiFlash 规则 CRUD、列存采集超时熔断，以及分区加速配置。
fn test_tiflash_manager() {
    let _guard = serial();
    init(None);
    let tiflash = NewMockTiFlash();
    SetMockTiFlash(tiflash.clone()).unwrap();
    let rule = MakeNewRule(1, 2, vec!["a".into()]);
    SetTiFlashPlacementRule(&rule).unwrap();
    let rules = GetTiFlashGroupRules("tiflash").unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].ID, "table-1-r");
    assert_eq!(rules[0].Count, 2);
    assert_eq!(GetTiFlashStoresStat().unwrap().Count, 1);

    /// 阻塞直到收到取消信号的列存采集器，用于触发超时熔断。
    struct BlockingCollector {
        cancelled: Mutex<Option<mpsc::Sender<()>>>,
    }
    impl ColumnarProgressCollector for BlockingCollector {
        fn collect(
            &self,
            cancelled: Arc<AtomicBool>,
            _table_id: i64,
            _stores: HashMap<i64, StoreInfo>,
        ) -> Result<f64> {
            while !cancelled.load(Ordering::Acquire) {
                thread::yield_now();
            }
            if let Some(sender) = self.cancelled.lock().unwrap().take() {
                let _ = sender.send(());
            }
            Ok(0.0)
        }
    }
    let (cancelled_sender, cancelled_receiver) = mpsc::channel();
    let restore_collector =
        SetColumnarProgressCollectorForTest(Some(Arc::new(BlockingCollector {
            cancelled: Mutex::new(Some(cancelled_sender)),
        })));
    let restore_timeout = SetColumnarCollectTimeoutForTest(Duration::from_millis(10));
    let tikv_stores = HashMap::from([(1, StoreInfo::default())]);
    // 超时后应返回进度 1.0 且标记熔断已触发。
    let (progress, triggered) =
        MustGetTiFlashProgressWithCircuitBreaker(1024, 1, &HashMap::new(), &tikv_stores).unwrap();
    assert_eq!(progress, 1.0);
    assert!(triggered);
    cancelled_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("progress collection must observe cancellation");
    restore_timeout();
    restore_collector();

    DeleteTiFlashPlacementRules(&[1]).unwrap();
    assert!(GetTiFlashGroupRules("tiflash").unwrap().is_empty());
    ConfigureTiFlashPDForTable(1, 2, &["a".into()]).unwrap();
    let partitions = vec![
        model::PartitionDefinition {
            ID: 2,
            ..Default::default()
        },
        model::PartitionDefinition {
            ID: 3,
            ..Default::default()
        },
    ];
    ConfigureTiFlashPDForPartitions(true, &partitions, 3, &[], 100).unwrap();
    assert_eq!(GetTiFlashGroupRules("tiflash").unwrap().len(), 3);
    assert!(tiflash.GetTableSyncStatus(2).unwrap().Accel);
    assert!(tiflash.GetTableSyncStatus(3).unwrap().Accel);
    CloseTiFlashManager().unwrap();
}

#[test]
/// 验证 ServerInfo 的 serde 字段名与 Go JSON 对齐。
fn test_info_syncer_marshal() {
    let info = ServerInfo {
        Version: "8.8.8".into(),
        GitHash: "123456".into(),
        ID: "tidb1".into(),
        IP: "127.0.0.1".into(),
        Port: 4000,
        StatusPort: 10080,
        Lease: "1s".into(),
        StartTimestamp: 10000,
        JSONServerID: 1,
        Labels: HashMap::from([("zone".into(), "ap-northeast-1a".into())]),
    };
    let data = serde_json::to_vec(&info).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&data).unwrap();
    assert_eq!(value["ddl_id"], "tidb1");
    assert_eq!(value["listening_port"], 4000);
    assert_eq!(value["labels"]["zone"], "ap-northeast-1a");
    assert_eq!(serde_json::from_slice::<ServerInfo>(&data).unwrap(), info);
}

/// 记录 `update_keyspace_config` 调用的 PD HTTP mock。
struct KeyspaceClient {
    calls: Mutex<Vec<(String, UpdateKeyspaceConfigParams)>>,
    error: Option<String>,
}
impl PdHttpClient for KeyspaceClient {
    fn update_keyspace_config(
        &self,
        name: &str,
        params: &UpdateKeyspaceConfigParams,
    ) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((name.into(), params.clone()));
        match &self.error {
            Some(error) => Err(Error::External(error.clone())),
            None => Ok(()),
        }
    }
}

#[test]
/// 有 PD 客户端时正确转发 Keyspace 配置更新参数。
fn test_set_keyspace_config() {
    let _guard = serial();
    init(None);
    let client = Arc::new(KeyspaceClient {
        calls: Mutex::new(Vec::new()),
        error: None,
    });
    let restore = SetPDHttpCliForTest(client.clone()).unwrap();
    let params = UpdateKeyspaceConfigParams {
        Config: HashMap::from([(
            "serverless_is_bootstrapped_for_restore".into(),
            Some("True".into()),
        )]),
        Preconditions: HashMap::from([(
            "serverless_is_bootstrapped_for_restore".into(),
            Some("False".into()),
        )]),
    };
    SetKeyspaceConfig("test-keyspace", params.clone()).unwrap();
    assert_eq!(
        client.calls.lock().unwrap().as_slice(),
        &[("test-keyspace".into(), params)]
    );
    restore();
}

#[test]
/// 无 PD 客户端时应返回 `PdHttpClientMissing`。
fn test_set_keyspace_config_without_pdhttp_client() {
    let _guard = serial();
    init(None);
    assert!(matches!(
        SetKeyspaceConfig("test-keyspace", Default::default()),
        Err(Error::PdHttpClientMissing)
    ));
}

#[test]
/// PD 返回错误时应原样向上传播。
fn test_set_keyspace_config_propagates_pdhttp_error() {
    let _guard = serial();
    init(None);
    let client = Arc::new(KeyspaceClient {
        calls: Mutex::new(Vec::new()),
        error: Some("update keyspace config failed".into()),
    });
    let _restore = SetPDHttpCliForTest(client).unwrap();
    let error = SetKeyspaceConfig(
        "test-keyspace",
        UpdateKeyspaceConfigParams {
            Config: HashMap::from([("k".into(), None)]),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "update keyspace config failed");
}

#[test]
/// 分区表只清理各物理分区缓存；普通表清理表缓存，并保持 Go 的缓存优先语义。
fn tiflash_progress_cache_and_partition_cleanup_match_go() {
    let _guard = serial();
    init(None);

    UpdateTiFlashProgressCache(100, 0.25).unwrap();
    UpdateTiFlashProgressCache(101, 0.5).unwrap();
    UpdateTiFlashProgressCache(102, 0.75).unwrap();
    let table = model::TableInfo {
        ID: 100,
        Partition: Some(model::PartitionInfo {
            Enable: true,
            Definitions: vec![
                model::PartitionDefinition {
                    ID: 101,
                    ..Default::default()
                },
                model::PartitionDefinition {
                    ID: 102,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }),
        ..Default::default()
    };
    DeleteTiFlashTableSyncProgress(&table).unwrap();
    assert_eq!(GetTiFlashProgressFromCache(100).unwrap(), Some(0.25));
    assert_eq!(GetTiFlashProgressFromCache(101).unwrap(), None);
    assert_eq!(GetTiFlashProgressFromCache(102).unwrap(), None);

    assert_eq!(
        MustGetTiFlashProgress(100, 1, &HashMap::new(), &HashMap::new()).unwrap(),
        0.25
    );
    CleanTiFlashProgressCache().unwrap();
    assert_eq!(
        MustGetTiFlashProgress(200, 1, &HashMap::new(), &HashMap::new()).unwrap(),
        1.0
    );
    assert_eq!(GetTiFlashProgressFromCache(200).unwrap(), Some(1.0));
}

#[test]
/// PD handler、API V2 规则 ID 与规则匹配条件保持 Go 的精确边界。
fn pd_status_and_tiflash_rule_contract_match_go() {
    assert!(pdResponseHandler(200, b"").is_ok());
    assert!(pdResponseHandler(404, b"not found").is_ok());
    assert!(pdResponseHandler(412, b"precondition failed").is_ok());
    assert!(matches!(
        pdResponseHandler(201, b"created"),
        Err(Error::DomainService(message)) if message == "created"
    ));
    assert!(matches!(
        pdResponseHandler(500, b"server error"),
        Err(Error::DomainService(message)) if message == "server error"
    ));

    let codec = Codec {
        keyspace_id: Some(42),
        keyspace_aware_rules: true,
    };
    assert_eq!(
        encodeRuleID(codec, "table-7-r".into()),
        "keyspace-42-table-7-r"
    );

    let rule = MakeNewRule(7, 2, vec!["zone".into()]);
    assert!(isRuleMatch(
        rule.clone(),
        rule.StartKey.clone(),
        rule.EndKey.clone(),
        2,
        vec!["zone".into()]
    ));
    let mut wrong_role = rule.clone();
    wrong_role.Role = placement::pd::Leader;
    assert!(!isRuleMatch(
        wrong_role,
        rule.StartKey.clone(),
        rule.EndKey.clone(),
        2,
        vec!["zone".into()]
    ));
    let mut missing_engine_constraint = rule.clone();
    missing_engine_constraint.LabelConstraints.clear();
    assert!(!isRuleMatch(
        missing_engine_constraint,
        rule.StartKey,
        rule.EndKey,
        2,
        vec!["zone".into()]
    ));
}

#[test]
/// 配置分区规则不得额外删除已存在的表级规则。
fn configure_partitions_preserves_existing_table_rule() {
    let _guard = serial();
    init(None);
    let tiflash = NewMockTiFlash();
    SetMockTiFlash(tiflash).unwrap();
    ConfigureTiFlashPDForTable(100, 2, &[]).unwrap();
    ConfigureTiFlashPDForPartitions(
        false,
        &[
            model::PartitionDefinition {
                ID: 101,
                ..Default::default()
            },
            model::PartitionDefinition {
                ID: 102,
                ..Default::default()
            },
        ],
        2,
        &[],
        100,
    )
    .unwrap();
    let ids: std::collections::HashSet<_> = GetTiFlashGroupRules("tiflash")
        .unwrap()
        .into_iter()
        .map(|rule| rule.ID)
        .collect();
    assert_eq!(
        ids,
        std::collections::HashSet::from([
            "table-100-r".to_owned(),
            "table-101-r".to_owned(),
            "table-102-r".to_owned(),
        ])
    );
}

#[derive(Default)]
struct MemoryEtcd {
    values: RwLock<HashMap<String, Vec<u8>>>,
}

impl EtcdClient for MemoryEtcd {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.values.read().unwrap().get(key).cloned())
    }

    fn get_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>> {
        Ok(self
            .values
            .read()
            .unwrap()
            .iter()
            .filter(|(key, _)| key.starts_with(prefix))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect())
    }

    fn put(&self, key: &str, value: Vec<u8>) -> Result<()> {
        self.values.write().unwrap().insert(key.into(), value);
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.values.write().unwrap().remove(key);
        Ok(())
    }
}

#[test]
/// 测试注入 etcd 后，TiProxy/TiCDC 拓扑键和 JSON 字段按 Go 解析。
fn etcd_injection_and_topology_decoding_match_go() {
    let _guard = serial();
    init(None);
    let etcd = Arc::new(MemoryEtcd::default());
    etcd.put(
        "/topology/tiproxy/127.0.0.1:3080/info",
        br#"{"version":"v1","git_hash":"abc","ip":"127.0.0.1","port":"3080","status_port":"3081","start_timestamp":9}"#.to_vec(),
    )
    .unwrap();
    etcd.put(
        "/topology/ticdc/cluster-a/node-1",
        br#"{"id":"node-1","address":"127.0.0.1:8300","version":"v8.5.0","git-hash":"def","deploy-path":"/ticdc","start-timestamp":10}"#.to_vec(),
    )
    .unwrap();
    etcd.put(
        "/tidb/server/info/remote-node",
        br#"{"version":"v1","git_hash":"abc","ddl_id":"remote-node","ip":"2001:db8::1","listening_port":4000,"status_port":10080,"lease":"45s","start_timestamp":1,"server_id":2,"labels":{}}"#.to_vec(),
    )
    .unwrap();

    SetEtcdClient(Some(etcd)).unwrap();
    assert!(GetEtcdClient().unwrap().is_some());

    let proxies = GetTiProxyServerInfo().unwrap();
    let proxy = proxies.get("127.0.0.1:3080").unwrap();
    let proxy_json = serde_json::to_value(proxy).unwrap();
    assert_eq!(proxy_json["port"], "3080");
    assert_eq!(proxy_json["status_port"], "3081");
    assert_eq!(proxy_json["start_timestamp"], 9);

    let cdc = GetTiCDCServerInfo().unwrap();
    assert_eq!(cdc.len(), 1);
    let cdc_json = serde_json::to_value(&cdc[0]).unwrap();
    assert_eq!(cdc_json["version"], "8.5.0");
    assert_eq!(cdc_json["git-hash"], "def");
    assert_eq!(cdc_json["deploy-path"], "/ticdc");
    assert_eq!(cdc_json["start-timestamp"], 10);

    let servers = GetAllServerInfo().unwrap();
    assert_eq!(servers["remote-node"].IP, "2001:db8::1");
    assert_eq!(servers["remote-node"].Port, 4000);
}

#[test]
/// mock server manager 使用递增端口，并按 `ip:port` 删除、Close 后重置端口。
fn mock_server_info_manager_matches_go_lifecycle() {
    let manager = MockGlobalServerInfoManager::default();
    manager.Add("first".into(), Arc::new(|| 1));
    manager.Add("second".into(), Arc::new(|| 2));
    let infos = manager.GetAllServerInfo();
    assert_eq!(infos["first"].Port, 4000);
    assert_eq!(infos["second"].Port, 4001);

    manager.DeleteByExecID("127.0.0.1:4000");
    let infos = manager.GetAllServerInfo();
    assert!(!infos.contains_key("first"));
    assert!(infos.contains_key("second"));

    manager.Close();
    manager.Add("third".into(), Arc::new(|| 3));
    assert_eq!(manager.GetAllServerInfo()["third"].Port, 4000);
}

#[test]
/// TiFlash NextGen compute 节点不存 Region，不能归入写节点或 TiKV 节点。
fn tiflash_progress_store_partition_excludes_compute_nodes() {
    let stores = vec![
        StoreInfo {
            Store: StoreMeta {
                ID: 1,
                Labels: HashMap::from([("engine".into(), "tiflash".into())]),
                ..Default::default()
            },
        },
        StoreInfo {
            Store: StoreMeta {
                ID: 2,
                Labels: HashMap::from([("engine".into(), "tiflash_compute".into())]),
                ..Default::default()
            },
        },
        StoreInfo {
            Store: StoreMeta {
                ID: 3,
                Labels: HashMap::from([("engine".into(), "tikv".into())]),
                ..Default::default()
            },
        },
    ];

    let (tiflash, tikv) = partitionTiFlashProgressStores(stores);
    assert_eq!(tiflash.keys().copied().collect::<Vec<_>>(), vec![1]);
    assert_eq!(tikv.keys().copied().collect::<Vec<_>>(), vec![3]);
}
