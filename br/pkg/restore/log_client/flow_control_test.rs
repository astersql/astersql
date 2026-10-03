// Copyright 2026 AsterSQL.
use super::flow_control::*;
use astersql_br_pkg_restore::{BackupFileSet, BatchBackupFileSet};
use astersql_br_pkg_restore_utils::stubs::backuppb::File;
const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;
const TIB: u64 = 1 << 40;
fn configs(values: &[&str]) -> Vec<TiKVConfigValue> {
    values
        .iter()
        .enumerate()
        .map(|(i, v)| TiKVConfigValue {
            instance: i.to_string(),
            value: v.to_string(),
        })
        .collect()
}
#[test]
fn estimate_compacted_ssts_preserves_physical_sizes_and_checkpoint_bytes() {
    let sets: BatchBackupFileSet = vec![BackupFileSet {
        TableID: 1,
        SSTFiles: vec![
            File {
                Name: "file-1".into(),
                Size_: 16 * MIB,
                ..Default::default()
            },
            File {
                Name: "file-2".into(),
                TotalBytes: 8 * MIB,
                ..Default::default()
            },
        ],
        ..Default::default()
    }];
    let estimate = estimateCompactedSSTFlowControl(&sets, 120 * MIB, 6 * MIB, 5, 3);
    assert_eq!(
        (
            estimate.snapshotRestoreBytes,
            estimate.compactedSSTBytes,
            estimate.l6BytesPerStore,
            estimate.l5BytesPerStore,
            estimate.pendingBytes
        ),
        (120 * MIB, 30 * MIB, 72 * MIB, 18 * MIB, 54 * MIB)
    );
    assert_eq!(
        estimateCompactedSSTFlowControl(&sets, 120, 0, 0, 3).pendingBytes,
        0
    );
    assert_eq!(
        estimateCompactedSSTFlowControl(&sets, 120, 0, 2, 5).l6BytesPerStore,
        120
    );
    assert_eq!(
        estimateCompactedSSTFlowControl(&sets, 0, u64::MAX, 1, 1).compactedSSTBytes,
        u64::MAX
    );
}
#[test]
fn pending_compaction_and_targets_match_restore_limits() {
    assert_eq!(estimatePendingCompactionBytes(11 * TIB, TIB), 0);
    assert_eq!(estimatePendingCompactionBytes(10 * TIB, TIB), 0);
    assert_eq!(estimatePendingCompactionBytes(0, 0), 0);
    let expected = ((512 * GIB) as f64 - TIB as f64 / 10.0) * 3.0;
    assert!((estimatePendingCompactionBytes(TIB, 512 * GIB) as f64 - expected).abs() < MIB as f64);
    let config = CompactedSSTFlowControlConfig {
        soft: configs(&["192GiB"]),
        hard: configs(&["256GiB"]),
    };
    assert_eq!(
        compactedSSTFlowControlTarget(&config, 512 * GIB),
        (TIB, 2 * TIB)
    );
    assert_eq!(
        compactedSSTFlowControlTarget(&config, 3 * TIB),
        (3840 * GIB, 7680 * GIB)
    );
    let config = CompactedSSTFlowControlConfig {
        soft: configs(&["4TiB"]),
        hard: configs(&["9TiB"]),
    };
    assert_eq!(
        compactedSSTFlowControlTarget(&config, 512 * GIB),
        (4 * TIB, 9 * TIB)
    );
    assert_eq!(
        compactedSSTFlowControlTarget(&config, u64::MAX),
        (u64::MAX, u64::MAX)
    );
    assert!(!allTiKVConfigsAtLeast(
        &configs(&["4TiB", "192GiB"]),
        4 * TIB
    ));
    assert!(allTiKVConfigsAtLeast(&configs(&["4TiB", "5TiB"]), 4 * TIB));
    assert!(!allTiKVConfigsAtLeast(&configs(&["bad"]), 0));
    assert!(!allTiKVConfigsAtLeast(&[], 0));
    assert_eq!(formatBytes(TIB), "1TiB");
    assert_eq!(formatBytes(1536 * GIB), "1536GiB");
}

use crate::client::{SstRestoreManager, TEST_NewLogClient};
use crate::stubs::glue::{Row, Session, SessionCtx, SqlArg};
use crate::stubs::{Context, Error, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
#[derive(Default)]
struct ConfigState {
    values: HashMap<String, Vec<String>>,
    calls: Vec<(String, String)>,
    fail_at: Option<usize>,
}
struct ConfigSession {
    context: SessionCtx,
    state: Arc<Mutex<ConfigState>>,
}
impl Session for ConfigSession {
    fn Close(&mut self) {}
    fn ExecuteInternal(&self, _: &Context, _: &str, _: &[SqlArg]) -> Result<()> {
        unreachable!()
    }
    fn GetSessionCtx(&self) -> &SessionCtx {
        &self.context
    }
    fn ExecRestrictedSQL(&self, _: &Context, sql: &str, args: &[SqlArg]) -> Result<Vec<Row>> {
        let SqlArg::Str(value) = &args[0] else {
            panic!("bound config argument")
        };
        let mut state = self.state.lock().unwrap();
        let call = state.calls.len();
        state.calls.push((sql.into(), value.clone()));
        if state.fail_at == Some(call) {
            return Err(Error::new("config transport failure"));
        }
        if sql.starts_with("show config") {
            return Ok(state
                .values
                .get(value)
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(i, v)| Row {
                    cols: vec![
                        SqlArg::Str("tikv".into()),
                        SqlArg::Str(format!("store-{i}")),
                        SqlArg::Str(value.clone()),
                        SqlArg::Str(v.clone()),
                    ],
                })
                .collect());
        }
        let name = sql.split('`').nth(1).expect("quoted config name");
        for config in state.values.get_mut(name).expect("known config") {
            *config = value.clone();
        }
        Ok(Vec::new())
    }
}
fn restore_client(
    soft: &[&str],
    hard: &[&str],
) -> (crate::client::LogClient, Arc<Mutex<ConfigState>>) {
    let state = Arc::new(Mutex::new(ConfigState {
        values: HashMap::from([
            (
                tikvSoftPendingCompactionBytesLimit.into(),
                soft.iter().map(|v| v.to_string()).collect(),
            ),
            (
                tikvHardPendingCompactionBytesLimit.into(),
                hard.iter().map(|v| v.to_string()).collect(),
            ),
        ]),
        ..Default::default()
    }));
    let mut client = TEST_NewLogClient(1, 2);
    client.unsafeSession = Some(Box::new(ConfigSession {
        context: SessionCtx::default(),
        state: state.clone(),
    }));
    client.sstRestoreManager = Some(SstRestoreManager {
        closed: false,
        storeCount: 1,
        replicaCount: 1,
        workerPoolSize: 7186,
        restorer: None,
    });
    (client, state)
}
fn compacted_sets(bytes: u64) -> BatchBackupFileSet {
    vec![BackupFileSet {
        TableID: 42,
        SSTFiles: vec![File {
            Name: "nonempty.sst".into(),
            Size_: bytes,
            TotalBytes: bytes * 2,
            TotalKvs: 123,
            ..Default::default()
        }],
        ..Default::default()
    }]
}
#[test]
fn flow_control_reads_all_stores_then_sets_hard_before_soft() {
    let (client, state) = restore_client(&["192GiB"], &["256GiB"]);
    client
        .adjustTiKVFlowControlForCompactedSSTRestore(
            &Context::Background(),
            &compacted_sets(512 * GIB),
            TIB,
            0,
        )
        .unwrap();
    let state = state.lock().unwrap();
    assert_eq!(state.calls.len(), 4);
    assert_eq!(state.calls[0].1, tikvSoftPendingCompactionBytesLimit);
    assert_eq!(state.calls[1].1, tikvHardPendingCompactionBytesLimit);
    assert_eq!(
        state.calls[2],
        (
            format!("set config tikv `{tikvHardPendingCompactionBytesLimit}`=%?"),
            "3TiB".into()
        )
    );
    assert_eq!(
        state.calls[3],
        (
            format!("set config tikv `{tikvSoftPendingCompactionBytesLimit}`=%?"),
            "1536GiB".into()
        )
    );
    assert_eq!(
        state.values[tikvHardPendingCompactionBytesLimit],
        vec!["3TiB"]
    );
    assert_eq!(
        state.values[tikvSoftPendingCompactionBytesLimit],
        vec!["1536GiB"]
    );
}
#[test]
fn flow_control_skips_unavailable_configs_small_estimates_and_sufficient_stores() {
    let sets = compacted_sets(512 * GIB);
    for missing in 0..4 {
        let (mut client, state) = restore_client(&["192GiB"], &["256GiB"]);
        match missing {
            0 => client.unsafeSession = None,
            1 => client.sstRestoreManager = None,
            2 => client.sstRestoreManager.as_mut().unwrap().storeCount = 0,
            _ => client.sstRestoreManager.as_mut().unwrap().replicaCount = 0,
        }
        client
            .adjustTiKVFlowControlForCompactedSSTRestore(&Context::Background(), &sets, TIB, 0)
            .unwrap();
        assert!(state.lock().unwrap().calls.is_empty());
    }
    for (soft, hard) in [(vec![], vec!["256GiB"]), (vec!["4TiB"], vec!["9TiB"])] {
        let (client, state) = restore_client(&soft, &hard);
        client
            .adjustTiKVFlowControlForCompactedSSTRestore(&Context::Background(), &sets, TIB, 0)
            .unwrap();
        assert_eq!(state.lock().unwrap().calls.len(), 2);
    }
    let (client, state) = restore_client(&["192GiB"], &["256GiB"]);
    client
        .adjustTiKVFlowControlForCompactedSSTRestore(
            &Context::Background(),
            &compacted_sets(100 * GIB),
            0,
            0,
        )
        .unwrap();
    assert_eq!(state.lock().unwrap().calls.len(), 2);
    let (client, state) = restore_client(&["4TiB", "192GiB"], &["9TiB", "256GiB"]);
    client
        .adjustTiKVFlowControlForCompactedSSTRestore(&Context::Background(), &sets, TIB, 0)
        .unwrap();
    assert_eq!(
        state.lock().unwrap().values[tikvSoftPendingCompactionBytesLimit],
        vec!["4TiB", "4TiB"]
    );
    assert_eq!(
        state.lock().unwrap().values[tikvHardPendingCompactionBytesLimit],
        vec!["9TiB", "9TiB"]
    );
}
#[test]
fn flow_control_propagates_read_and_write_failures_without_later_writes() {
    for fail_at in 0..4 {
        let (client, state) = restore_client(&["192GiB"], &["256GiB"]);
        state.lock().unwrap().fail_at = Some(fail_at);
        let error = client
            .adjustTiKVFlowControlForCompactedSSTRestore(
                &Context::Background(),
                &compacted_sets(512 * GIB),
                TIB,
                0,
            )
            .unwrap_err();
        assert!(error.to_string().contains("config transport failure"));
        if fail_at >= 2 {
            assert!(error.to_string().contains("failed to set config"));
        }
        assert_eq!(state.lock().unwrap().calls.len(), fail_at + 1);
    }
}

struct RecordingSSTImporter {
    state: Arc<Mutex<ConfigState>>,
    imported: Arc<Mutex<Vec<BackupFileSet>>>,
}
impl astersql_br_pkg_restore::FileImporter for RecordingSSTImporter {
    fn ConfigureDownloadRetry(
        &self,
        _ctx: &astersql_br_pkg_restore::stubs::Context,
        _stores: &[u64],
    ) -> astersql_br_pkg_restore::stubs::Result<()> {
        Ok(())
    }
    fn Import(
        &self,
        _: &astersql_br_pkg_restore::stubs::Context,
        sets: &[BackupFileSet],
    ) -> astersql_br_pkg_restore::stubs::Result<()> {
        self.state
            .lock()
            .unwrap()
            .calls
            .push(("import-sst".into(), sets[0].TableID.to_string()));
        self.imported.lock().unwrap().extend_from_slice(sets);
        Ok(())
    }
    fn Close(&self) -> astersql_br_pkg_restore::stubs::Result<()> {
        Ok(())
    }
}
struct RestorePD;
impl astersql_br_pkg_restore::stubs::PdClient for RestorePD {
    fn GetTS(
        &self,
        _: &astersql_br_pkg_restore::stubs::Context,
    ) -> astersql_br_pkg_restore::stubs::Result<(i64, i64)> {
        Ok((1, 0))
    }
    fn GetAllStores(
        &self,
        _: &astersql_br_pkg_restore::stubs::Context,
    ) -> astersql_br_pkg_restore::stubs::Result<Vec<astersql_br_pkg_restore::stubs::metapb::Store>>
    {
        Ok(vec![astersql_br_pkg_restore::stubs::metapb::Store {
            Id: 1,
            Address: "store-a".into(),
            Labels: vec![],
        }])
    }
}
struct ModeTransport(Arc<Mutex<ConfigState>>);
impl astersql_br_pkg_restore::stubs::ImportSstSwitcher for ModeTransport {
    fn SwitchMode(
        &self,
        _: &astersql_br_pkg_restore::stubs::Context,
        _: &str,
        mode: astersql_br_pkg_restore::stubs::import_sstpb::SwitchMode,
    ) -> astersql_br_pkg_restore::stubs::Result<()> {
        self.0
            .lock()
            .unwrap()
            .calls
            .push(("switch-mode".into(), format!("{mode:?}")));
        Ok(())
    }
}
#[test]
fn restore_sst_pipeline_adjusts_configs_before_mode_switch_and_import() {
    for online in [false, true] {
        let (mut client, state) = restore_client(&["192GiB"], &["256GiB"]);
        let imported = Arc::new(Mutex::new(Vec::new()));
        let restore_ctx = astersql_br_pkg_restore::stubs::Context::Background();
        client
            .InitSSTFileRestorer(
                &restore_ctx,
                Arc::new(RecordingSSTImporter {
                    state: state.clone(),
                    imported: imported.clone(),
                }),
                None,
            )
            .unwrap();
        let mut mode = astersql_br_pkg_restore::import_mode_switcher::NewImportModeSwitcher(
            Arc::new(RestorePD),
            std::time::Duration::from_secs(60),
            Arc::new(ModeTransport(state.clone())),
        );
        let result = client.RestoreSSTFileSets(
            &Context::Background(),
            &restore_ctx,
            compacted_sets(512 * GIB),
            &mut mode,
            online,
            TIB,
            0,
            Arc::new(|_| {}),
        );
        mode.SwitchToNormalMode(&restore_ctx).unwrap();
        result.unwrap();
        let calls = &state.lock().unwrap().calls;
        assert_eq!(calls[2].1, "3TiB");
        assert_eq!(calls[3].1, "1536GiB");
        let import_index = calls
            .iter()
            .position(|(operation, _)| operation == "import-sst")
            .unwrap();
        assert!(import_index > 3);
        assert_eq!(
            calls
                .iter()
                .any(|(operation, _)| operation == "switch-mode"),
            !online
        );
        let sets = imported.lock().unwrap();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].TableID, 42);
        assert_eq!(sets[0].SSTFiles[0].Size_, 512 * GIB);
        assert_eq!(sets[0].SSTFiles[0].TotalBytes, 1024 * GIB);
        assert_eq!(
            client
                .restoreStat
                .restoreSSTKVCount
                .load(std::sync::atomic::Ordering::Relaxed),
            123
        );
        client.Close(&Context::Background());
    }
}

#[test]
fn live_store_count_and_pd_replica_configuration_match_go() {
    use crate::stubs::metapb::{Store, StoreState};
    assert_eq!(crate::client::liveTiKVStoreCount(&[]), 0);
    assert_eq!(
        crate::client::liveTiKVStoreCount(&[
            Store {
                State: StoreState::Up,
                ..Default::default()
            },
            Store {
                State: StoreState::Offline,
                ..Default::default()
            },
            Store {
                State: StoreState::Tombstone,
                ..Default::default()
            }
        ]),
        1
    );
    for (value, expected) in [
        (serde_json::json!(5), 5),
        (serde_json::json!(0), 3),
        (serde_json::json!(-1), 3),
        (serde_json::json!("5"), 3),
        (serde_json::json!(null), 3),
    ] {
        let config = std::collections::HashMap::from([("max-replicas".to_string(), value)]);
        assert_eq!(
            crate::client::maxReplicaFromReplicateConfig(Some(&config), false),
            expected
        );
        assert_eq!(
            crate::client::maxReplicaFromReplicateConfig(Some(&config), true),
            3
        );
    }
    assert_eq!(crate::client::maxReplicaFromReplicateConfig(None, false), 3);
}

#[test]
fn init_clients_uses_live_stores_for_estimates_and_all_tikv_stores_for_pool() {
    struct Replicas;
    impl crate::stubs::pdhttp::ReplicateConfigClient for Replicas {
        fn GetReplicateConfig(
            &self,
            _: &crate::stubs::Context,
        ) -> std::result::Result<
            std::collections::HashMap<String, serde_json::Value>,
            astersql_errors::SharedError,
        > {
            Ok(std::collections::HashMap::from([(
                "max-replicas".into(),
                serde_json::json!(5),
            )]))
        }
    }
    use crate::stubs::metapb::{Store, StoreLabel, StoreState};
    let stores = vec![
        Store {
            Id: 1,
            State: StoreState::Up,
            ..Default::default()
        },
        Store {
            Id: 2,
            State: StoreState::Up,
            ..Default::default()
        },
        Store {
            Id: 3,
            State: StoreState::Offline,
            ..Default::default()
        },
        Store {
            Id: 4,
            Labels: vec![StoreLabel {
                Key: "engine".into(),
                Value: "tiflash".into(),
            }],
            ..Default::default()
        },
    ];
    for concurrency in [1, 36, 132] {
        let mut client = crate::client::NewLogClient(
            std::sync::Arc::new(crate::stubs::pd::MemPdClient {
                cluster_id: 1,
                stores: stores.clone(),
            }),
            crate::stubs::pdhttp::Client {
                backend: Some(std::sync::Arc::new(Replicas)),
            },
        );
        client
            .InitClients(
                &crate::stubs::Context::Background(),
                None,
                std::sync::Arc::new(crate::stubs::split_client::MemSplitClient::default()),
                std::sync::Arc::new(crate::stubs::importclient::MemImporterClient::default()),
                concurrency,
            )
            .unwrap();
        let manager = client.sstRestoreManager.as_ref().unwrap();
        assert_eq!(manager.storeCount, 2);
        assert_eq!(manager.replicaCount, 5);
        assert_eq!(manager.workerPoolSize, 3 * 7186);
        client.Close(&crate::stubs::Context::Background());
    }
}

#[test]
fn byte_size_config_preserves_docker_units_numeric_and_spacing_rules() {
    assert_eq!(
        crate::flow_control::parseByteSizeConfig("1e3MiB").unwrap(),
        1000 * 1024 * 1024
    );
    assert_eq!(
        crate::flow_control::parseByteSizeConfig("1 MiB").unwrap(),
        1024 * 1024
    );
    assert!(crate::flow_control::parseByteSizeConfig(" 1MiB").is_err());
    assert!(crate::flow_control::parseByteSizeConfig("1MiB ").is_err());
    assert!(crate::flow_control::parseByteSizeConfig("1  MiB").is_err());
}

#[test]
fn sst_restorer_probes_download_retry_before_installing_importer() {
    struct ProbedImporter(std::sync::atomic::AtomicBool, Mutex<Vec<u64>>);
    impl astersql_br_pkg_restore::FileImporter for ProbedImporter {
        fn ConfigureDownloadRetry(
            &self,
            _: &astersql_br_pkg_restore::stubs::Context,
            ids: &[u64],
        ) -> astersql_br_pkg_restore::stubs::Result<()> {
            *self.1.lock().unwrap() = ids.to_vec();
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        fn Import(
            &self,
            _: &astersql_br_pkg_restore::stubs::Context,
            _: &[BackupFileSet],
        ) -> astersql_br_pkg_restore::stubs::Result<()> {
            unreachable!()
        }
        fn Close(&self) -> astersql_br_pkg_restore::stubs::Result<()> {
            Ok(())
        }
    }
    let (mut client, _) = restore_client(&["1GB"], &["2GB"]);
    client.pdClient = Arc::new(crate::stubs::pd::MemPdClient {
        cluster_id: 1,
        stores: vec![
            crate::stubs::metapb::Store {
                Id: 1,
                ..Default::default()
            },
            crate::stubs::metapb::Store {
                Id: 2,
                State: crate::stubs::metapb::StoreState::Offline,
                ..Default::default()
            },
            crate::stubs::metapb::Store {
                Id: 3,
                Labels: vec![crate::stubs::metapb::StoreLabel {
                    Key: "engine".into(),
                    Value: "tiflash".into(),
                }],
                ..Default::default()
            },
        ],
    });
    let ctx = astersql_br_pkg_restore::stubs::Context::Background();
    let importer = Arc::new(ProbedImporter(
        std::sync::atomic::AtomicBool::new(false),
        Mutex::new(Vec::new()),
    ));
    client
        .InitSSTFileRestorer(&ctx, importer.clone(), None)
        .unwrap();
    assert!(
        client
            .sstRestoreManager
            .as_ref()
            .unwrap()
            .restorer
            .is_some()
    );
    assert!(importer.0.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(*importer.1.lock().unwrap(), vec![1]);
}

#[test]
fn sst_restorer_probe_pd_lookup_preserves_parent_cancellation() {
    struct CancelPd(astersql_br_pkg_restore::stubs::Context);
    impl crate::stubs::pd::Client for CancelPd {
        fn GetClusterID(&self, _: &Context) -> u64 {
            1
        }
        fn GetAllStores(
            &self,
            ctx: &Context,
        ) -> crate::stubs::Result<Vec<crate::stubs::metapb::Store>> {
            self.0
                .cancel(astersql_br_pkg_restore::stubs::Error::with_code(
                    "context.Canceled",
                    "cancelled while probing stores",
                ));
            Err(ctx
                .Err()
                .expect("lookup context must observe live parent cancellation"))
        }
    }
    let (mut client, state) = restore_client(&["1GB"], &["2GB"]);
    let ctx = astersql_br_pkg_restore::stubs::Context::Background();
    client.pdClient = Arc::new(CancelPd(ctx.clone()));
    let error = client
        .InitSSTFileRestorer(
            &ctx,
            Arc::new(RecordingSSTImporter {
                state,
                imported: Arc::new(Mutex::new(Vec::new())),
            }),
            None,
        )
        .unwrap_err();
    assert_eq!(error.code, Some("context.Canceled"));
    assert_eq!(error.msg, "cancelled while probing stores");
    assert!(
        client
            .sstRestoreManager
            .as_ref()
            .unwrap()
            .restorer
            .is_none()
    );
}
