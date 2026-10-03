// Copyright 2026 AsterSQL.

use crate::restore_lifecycle::{RestoreGlue, RestoreImporter, RestoreKind, RestoreLifecycle};
use crate::stubs::backuppb::{BackupMeta, File, RawRange};
use crate::stubs::{MemGlue, MemStorage, MetaFile};
use astersql_br_pkg_restore as restore;
use std::sync::{Arc, Mutex};

struct Importer(Arc<Mutex<Vec<String>>>, bool);
impl restore::FileImporter for Importer {
    fn ConfigureDownloadRetry(
        &self,
        _: &restore::stubs::Context,
        _: &[u64],
    ) -> restore::stubs::Result<()> {
        Ok(())
    }
    fn Import(
        &self,
        _: &restore::stubs::Context,
        sets: &[restore::BackupFileSet],
    ) -> restore::stubs::Result<()> {
        self.0.lock().unwrap().extend(
            sets.iter()
                .flat_map(|s| &s.SSTFiles)
                .map(|f| f.Name.clone()),
        );
        if self.1 {
            return Err(restore::stubs::Error::new("ingest failed"));
        }
        Ok(())
    }
    fn Close(&self) -> restore::stubs::Result<()> {
        Ok(())
    }
}

fn selected_file() -> File {
    File {
        Name: "table_default.sst".into(),
        StartKey: b"a".to_vec(),
        EndKey: b"z".to_vec(),
        Size_: 42,
        Cf: "default".into(),
    }
}

fn verify_entry(kind: RestoreKind) {
    {
        for online in [false, true] {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let transport = Arc::new(restore::stubs::RecordingImportSstSwitcher::new());
            let scheduler = Arc::new(restore::stubs::MemConnMgr::default());
            let calls_copy = calls.clone();
            let mode_copy = transport.clone();
            let scheduler_copy = scheduler.clone();
            let inner = MemGlue::default();
            let glue = RestoreGlue {
                inner: &inner,
                factory: Arc::new(move |observed, files| {
                    assert_eq!(observed, kind);
                    Ok(RestoreLifecycle {
                        context: restore::stubs::Context::Background(),
                        pd: Arc::new(restore::stubs::MemPdClient::new(vec![
                            restore::stubs::metapb::Store {
                                Id: 1,
                                Address: "tikv".into(),
                                Labels: vec![],
                            },
                        ])),
                        schedulers: scheduler_copy.clone(),
                        mode_transport: mode_copy.clone(),
                        importer: importer_for(kind, calls_copy.clone(), false),
                        file_sets: prepared_sets(files),
                        key_ranges: None,
                        checkpoint_runner: None,
                        checkpoint_compacted_size: 0,
                    })
                }),
            };
            let storage = MemStorage::new();
            storage.put(
                MetaFile,
                serde_json::to_vec(&BackupMeta {
                    IsRawKv: kind == RestoreKind::Raw,
                    IsTxnKv: kind == RestoreKind::Txn,
                    RawRanges: vec![RawRange {
                        StartKey: b"a".to_vec(),
                        EndKey: b"z".to_vec(),
                        Cf: "default".into(),
                    }],
                    Files: vec![selected_file()],
                    ..Default::default()
                })
                .unwrap(),
            );
            let mut config = crate::common::Config::default();
            config.PD = vec!["pd".into()];
            config.SwitchModeInterval = std::time::Duration::from_secs(60);
            config.Storage = "local:///tmp".into();
            match kind {
                RestoreKind::Raw => {
                    let mut cfg = crate::restore_raw::RestoreRawConfig::default();
                    cfg.RawKvConfig.Config = config;
                    cfg.RawKvConfig.StartKey = b"a".to_vec();
                    cfg.RawKvConfig.EndKey = b"z".to_vec();
                    cfg.RawKvConfig.CF = "default".into();
                    cfg.RestoreCommonConfig.Online = online;
                    cfg.RestoreStorage = Some(storage);
                    crate::restore_raw::RunRestoreRaw(&glue, "restore raw", &mut cfg).unwrap();
                }
                RestoreKind::Txn => crate::restore_txn::RunRestoreTxnWithStorage(
                    &glue,
                    "restore txn",
                    &mut config,
                    &storage,
                )
                .unwrap(),
                RestoreKind::Snapshot => {
                    let mut cfg = crate::restore::RestoreConfig::default();
                    cfg.Config = config;
                    cfg.RestoreCommonConfig.Online = online;
                    cfg.RestoreStorage = Some(storage);
                    crate::restore::RunRestore(&glue, crate::restore::FullRestoreCmd, &mut cfg)
                        .unwrap();
                }
                _ => unreachable!(),
            }
            assert_eq!(
                *calls.lock().unwrap(),
                vec!["table_default.sst".to_string()]
            );
            let modes: Vec<_> = transport
                .calls
                .lock()
                .unwrap()
                .iter()
                .map(|(_, m)| *m)
                .collect();
            let effective_online = online && kind != RestoreKind::Txn;
            assert_eq!(
                modes,
                if effective_online {
                    vec![]
                } else {
                    vec![
                        restore::stubs::import_sstpb::SwitchMode::Import,
                        restore::stubs::import_sstpb::SwitchMode::Normal,
                    ]
                }
            );
            assert_eq!(
                scheduler
                    .remove_called
                    .load(std::sync::atomic::Ordering::SeqCst),
                !effective_online
            );
        }
    }
}

fn prepared_sets(files: &[File]) -> restore::BatchBackupFileSet {
    let mut set = restore::BackupFileSet {
        TableID: 7,
        RewriteRules: None,
        SSTFiles: vec![],
    };
    for f in files {
        set.SSTFiles.push(Default::default());
        let out = set.SSTFiles.last_mut().unwrap();
        out.Name = f.Name.clone();
        out.StartKey = f.StartKey.clone();
        out.EndKey = f.EndKey.clone();
        out.Cf = f.Cf.clone();
        out.Size_ = f.Size_;
        out.TotalKvs = 3;
        out.TotalBytes = 64;
    }
    vec![set]
}

#[test]
fn raw_restore_consumes_real_restorer_and_online_mode_lifecycle() {
    verify_entry(RestoreKind::Raw);
}
#[test]
fn txn_restore_consumes_real_restorer_and_offline_mode_lifecycle() {
    verify_entry(RestoreKind::Txn);
}
#[test]
fn snapshot_restore_consumes_real_restorer_and_online_mode_lifecycle() {
    verify_entry(RestoreKind::Snapshot);
}

pub(crate) fn fixture_glue(inner: &dyn crate::stubs::Glue) -> RestoreGlue<'_> {
    RestoreGlue {
        inner,
        factory: Arc::new(|_, files| Ok(fixture_runtime(files))),
    }
}

pub(crate) fn fixture_runtime(files: &[File]) -> RestoreLifecycle {
    let mut sets = prepared_sets(files);
    for file in sets.iter_mut().flat_map(|set| &mut set.SSTFiles) {
        file.TotalKvs = 1;
    }
    RestoreLifecycle {
        context: restore::stubs::Context::Background(),
        pd: Arc::new(restore::stubs::MemPdClient::new(vec![
            restore::stubs::metapb::Store {
                Id: 1,
                Address: "tikv".into(),
                Labels: vec![],
            },
        ])),
        schedulers: Arc::new(restore::stubs::MemConnMgr::default()),
        mode_transport: Arc::new(restore::stubs::RecordingImportSstSwitcher::new()),
        importer: importer_for(RestoreKind::Snapshot, Arc::new(Mutex::new(vec![])), false),
        file_sets: sets,
        key_ranges: Some(
            files
                .iter()
                .map(|file| [file.StartKey.clone(), file.EndKey.clone()])
                .collect(),
        ),
        checkpoint_runner: None,
        checkpoint_compacted_size: 0,
    }
}

#[test]
fn snapshot_checkpoint_retains_modes_on_error_and_other_restores_cleanup() {
    for checkpoint in [false, true] {
        let mut runtime = fixture_runtime(&[selected_file()]);
        let calls = Arc::new(Mutex::new(vec![]));
        let transport = Arc::new(restore::stubs::RecordingImportSstSwitcher::new());
        runtime.importer = importer_for(RestoreKind::Snapshot, calls.clone(), true);
        runtime.mode_transport = transport.clone();
        if checkpoint {
            runtime.checkpoint_runner =
                Some(Arc::new(restore::stubs::MemRestoreCheckpoint::default()));
        }
        let error = runtime
            .RestoreFiles(
                RestoreKind::Snapshot,
                std::time::Duration::from_secs(3600),
                2,
                false,
                checkpoint,
                Arc::new(|_| {}),
            )
            .unwrap_err();
        assert!(error.to_string().contains("ingest failed"));
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["table_default.sst".to_string()]
        );
        let modes: Vec<_> = transport
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|(_, mode)| *mode)
            .collect();
        assert_eq!(
            modes,
            if checkpoint {
                vec![restore::stubs::import_sstpb::SwitchMode::Import]
            } else {
                vec![
                    restore::stubs::import_sstpb::SwitchMode::Import,
                    restore::stubs::import_sstpb::SwitchMode::Normal,
                ]
            }
        );
    }
}

#[test]
fn prepared_files_cannot_omit_selected_data() {
    let mut runtime = fixture_runtime(&[selected_file()]);
    runtime.file_sets.clear();
    assert!(runtime.ValidateFiles(&[selected_file()]).is_err());
}

#[test]
fn stream_body_imports_nonempty_compacted_sst_with_online_mode() {
    for online in [false, true] {
        let calls = Arc::new(Mutex::new(vec![]));
        let transport = Arc::new(restore::stubs::RecordingImportSstSwitcher::new());
        let recorded = calls.clone();
        let modes = transport.clone();
        let inner = MemGlue::default();
        let glue = RestoreGlue {
            inner: &inner,
            factory: Arc::new(move |kind, files| {
                assert_eq!(kind, RestoreKind::Stream);
                assert!(files.is_empty());
                let mut runtime = fixture_runtime(&[selected_file()]);
                runtime.mode_transport = modes.clone();
                runtime.importer = importer_for(RestoreKind::Stream, recorded.clone(), false);
                Ok(runtime)
            }),
        };
        let mut cfg = crate::restore::RestoreConfig::default();
        cfg.RestoreCommonConfig.Online = online;
        cfg.Config.SwitchModeInterval = std::time::Duration::from_secs(3600);
        let mut client = astersql_br_pkg_restore_log_client::TEST_NewLogClient(1, 2);
        client.pdClient = Arc::new(astersql_br_pkg_restore_log_client::stubs::pd::MemPdClient {
            cluster_id: 1,
            stores: vec![astersql_br_pkg_restore_log_client::stubs::metapb::Store {
                Id: 1,
                State: astersql_br_pkg_restore_log_client::stubs::metapb::StoreState::Up,
                ..Default::default()
            }],
        });
        client.sstRestoreManager = Some(astersql_br_pkg_restore_log_client::SstRestoreManager {
            closed: false,
            storeCount: 1,
            replicaCount: 1,
            workerPoolSize: 2,
            restorer: None,
        });
        let result = crate::stream::restoreStreamBody(&glue, &mut cfg, &mut client);
        client.Close(&astersql_br_pkg_restore_log_client::stubs::Context::Background());
        result.unwrap();
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["table_default.sst".to_string()]
        );
        let actual: Vec<_> = transport
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|(_, m)| *m)
            .collect();
        assert_eq!(
            actual,
            if online {
                vec![]
            } else {
                vec![
                    restore::stubs::import_sstpb::SwitchMode::Import,
                    restore::stubs::import_sstpb::SwitchMode::Normal,
                ]
            }
        );
    }
}

impl restore::BalancedFileImporter for Importer {
    fn PauseForBackpressure(&self) {}
}
fn importer_for(kind: RestoreKind, calls: Arc<Mutex<Vec<String>>>, fail: bool) -> RestoreImporter {
    let importer = Arc::new(Importer(calls, fail));
    if kind == RestoreKind::Snapshot {
        RestoreImporter::Snapshot(importer)
    } else {
        RestoreImporter::Simple(importer)
    }
}
