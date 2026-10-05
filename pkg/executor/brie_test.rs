// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// BRIE（Backup/Restore/Import/Export）基础类型的单元测试。
//
// 校验 `brieKind` 的 SQL 表面字符串、`BrieError` 显示，以及
// `resultChunk` 的推行与 reset 行为。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::brie::{
    BRIEExec, BrieError, BrieErrorKind, BrieResult, ResetGlobalBRIEQueueForTest,
    alterTableModeArgs, backupConfig, backupMetadata, brieErrorClass, brieKind, brieOption,
    brieOptionType, brieRuntime, brieStmt, brieTaskInfo, cipherInfo, commonConfig,
    createTableOption, current_queue, databaseInfo, datum, executorBuilder, globalConfig,
    mapBRIEError, normalizedStorage, now_millis, outdatedDuration, placementPolicyInfo,
    refreshMetaArgs, restoreConfig, resultChunk, showConfig, sqlTime, tableFilter, tableInfo,
    taskContext, tidbGlue,
};

#[derive(Default)]
struct MockRuntime {
    warnings: Mutex<Vec<u64>>,
    kill_error: Mutex<Option<BrieError>>,
}

impl brieRuntime for MockRuntime {
    fn global_config(&self) -> globalConfig {
        globalConfig {
            pdAddresses: vec!["pd-1:2379".into(), "pd-2:2379".into()],
            tls: Default::default(),
            storeType: "tikv".into(),
        }
    }
    fn sem_v1_enabled(&self) -> bool {
        false
    }
    fn normalize_storage_url(
        &self,
        raw: &str,
        _: &mut commonConfig,
    ) -> BrieResult<normalizedStorage> {
        Ok(normalizedStorage {
            url: raw.into(),
            scheme: raw.split(':').next().unwrap_or_default().into(),
        })
    }
    fn restore_brie_query(&self, _: &brieStmt) -> BrieResult<String> {
        Ok("BACKUP TABLE `a` TO 'noop://'".into())
    }
    fn read_cipher_key_file(&self, _: &str) -> BrieResult<Vec<u8>> {
        Ok(vec![b'A'; 128])
    }
    fn parse_timestamp(&self, value: &str, _: &str) -> BrieResult<u64> {
        value.parse().map_err(|e| BrieError::new(format!("{e}")))
    }
    fn connection_id(&self) -> u64 {
        7
    }
    fn timezone(&self) -> String {
        "UTC".into()
    }
    fn check_killed(&self) -> BrieResult {
        match self.kill_error.lock().unwrap().clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    fn append_job_not_found_warning(&self, id: u64) {
        self.warnings.lock().unwrap().push(id);
    }
    fn read_backup_metadata(&self, _: &taskContext, _: &showConfig) -> BrieResult<backupMetadata> {
        Ok(Default::default())
    }
    fn format_tso_time(&self, tso: u64, _: &str) -> BrieResult<sqlTime> {
        Ok(sqlTime {
            unixMillis: tso as i64,
            valid: true,
        })
    }
    fn run_backup(&self, _: &taskContext, _: &mut tidbGlue, _: &backupConfig) -> BrieResult {
        Ok(())
    }
    fn run_restore(&self, _: &taskContext, _: &mut tidbGlue, _: &restoreConfig) -> BrieResult {
        Ok(())
    }
    fn domain_handle(&self, _: u64) -> BrieResult<u64> {
        Ok(1)
    }
    fn storage_handle(&self, _: u64) -> BrieResult<u64> {
        Ok(2)
    }
    fn create_session(&self, _: u64) -> BrieResult<u64> {
        Ok(3)
    }
    fn close_session(&self, _: u64) {}
    fn tidb_info(&self) -> String {
        "Release Version: test\nGit Commit Hash: test\nGoVersion: test".into()
    }
    fn execute_restricted_brie_sql(&self, _: u64, _: &str) -> BrieResult {
        Ok(())
    }
    fn execute_internal_brie_sql(&self, _: u64, _: &str, _: &[datum]) -> BrieResult {
        Ok(())
    }
    fn create_database(&self, _: u64, _: &databaseInfo) -> BrieResult {
        Ok(())
    }
    fn create_table(&self, _: u64, _: &str, _: &tableInfo, _: &[createTableOption]) -> BrieResult {
        Ok(())
    }
    fn create_tables(
        &self,
        _: u64,
        _: &BTreeMap<String, Vec<tableInfo>>,
        _: &[createTableOption],
    ) -> BrieResult {
        Ok(())
    }
    fn query_string(&self, _: u64) -> String {
        String::new()
    }
    fn set_query_string(&self, _: u64, _: &str) {}
    fn create_placement_policy_ignore_existing(
        &self,
        _: u64,
        _: &placementPolicyInfo,
    ) -> BrieResult {
        Ok(())
    }
    fn global_table_value(&self, _: u64, _: &str) -> BrieResult<String> {
        Ok(String::new())
    }
    fn global_system_variable(&self, _: u64, _: &str) -> BrieResult<String> {
        Ok(String::new())
    }
    fn alter_table_mode(&self, _: u64, _: &alterTableModeArgs) -> BrieResult {
        Ok(())
    }
    fn refresh_meta(&self, _: u64, _: &refreshMetaArgs) -> BrieResult {
        Ok(())
    }
    fn log_one_shot_session_closed(&self) {}
}

fn option(optionType: brieOptionType, uintValue: u64, stringValue: &str) -> brieOption {
    brieOption {
        optionType,
        uintValue,
        stringValue: stringValue.into(),
    }
}

fn statement(kind: brieKind, options: Vec<brieOption>) -> brieStmt {
    brieStmt {
        kind,
        storage: "noop://bucket/prefix".into(),
        jobID: 0,
        options,
        tables: vec![tableFilter {
            schema: "db".into(),
            table: "t".into(),
        }],
        schemas: Vec::new(),
    }
}

#[test]
/// kind 显示名、错误文本与 resultChunk 行缓冲行为。
fn brie_kinds_and_result_chunk_match_sql_surface() {
    assert_eq!(brieKind::Backup.to_string(), "BACKUP");
    assert_eq!(brieKind::Restore.to_string(), "RESTORE");
    assert_eq!(brieKind::ShowBackupMeta.to_string(), "SHOW BACKUP META");
    assert_eq!(BrieError::new("backup failed").to_string(), "backup failed");
    let mut chunk = resultChunk::default();
    chunk.push_row(vec![datum::String("db".into()), datum::Unsigned(42)]);
    assert_eq!(
        chunk.rows,
        vec![vec![datum::String("db".into()), datum::Unsigned(42)]]
    );
    chunk.reset();
    assert!(chunk.rows.is_empty());
}

#[test]
fn map_brie_error_preserves_kill_and_cancel_causes() {
    let runtime = MockRuntime::default();
    let ordinary = mapBRIEError(
        &runtime,
        Err(BrieError::new("disk full")),
        Some(brieErrorClass::Backup),
    )
    .unwrap_err();
    assert_eq!(BrieErrorKind::BackupFailed, ordinary.kind());

    *runtime.kill_error.lock().unwrap() = Some(BrieError::query_interrupted(
        "outer wrapper: query interrupted",
    ));
    let killed = mapBRIEError(
        &runtime,
        Err(BrieError::new("context canceled")),
        Some(brieErrorClass::Backup),
    )
    .unwrap_err();
    assert_eq!(BrieErrorKind::QueryInterrupted, killed.kind());

    *runtime.kill_error.lock().unwrap() = None;
    let canceled = mapBRIEError(
        &runtime,
        Err(BrieError::new("context canceled")),
        Some(brieErrorClass::Restore),
    )
    .unwrap_err();
    assert_eq!(BrieErrorKind::RestoreFailed, canceled.kind());
}

#[test]
fn queued_brie_cancellation_records_terminal_state() {
    ResetGlobalBRIEQueueForTest();
    let runtime: Arc<dyn brieRuntime> = Arc::new(MockRuntime::default());
    let info = Arc::new(Mutex::new(brieTaskInfo::new(brieKind::Backup)));
    let mut executor = BRIEExec {
        backupCfg: Some(backupConfig::default()),
        restoreCfg: None,
        showConfig: None,
        info: Some(info.clone()),
        runtime,
        sessionID: 1,
    };
    let context = taskContext::default();
    context.cancel();
    let error = executor
        .Next(&context, &mut resultChunk::default())
        .unwrap_err();
    assert_eq!(BrieErrorKind::ContextCanceled, error.kind());
    let info = info.lock().unwrap();
    assert!(info.finishTime.valid);
    assert_eq!(error.to_string(), info.message);
}

#[test]
fn clear_task_keeps_unfinished_brie_tasks() {
    ResetGlobalBRIEQueueForTest();
    let queue = current_queue();
    let running = Arc::new(Mutex::new(brieTaskInfo::new(brieKind::Backup)));
    let (_, running_id) = queue.registerTask(taskContext::default(), running);
    let finished = Arc::new(Mutex::new(brieTaskInfo::new(brieKind::Backup)));
    let (_, finished_id) = queue.registerTask(taskContext::default(), finished.clone());
    finished.lock().unwrap().finishTime = sqlTime {
        unixMillis: now_millis() - outdatedDuration.as_millis() as i64 - 1,
        valid: true,
    };
    queue.clearTask();

    assert!(queue.queryTask(running_id).is_some());
    assert!(queue.queryTask(finished_id).is_none());
}

#[test]
fn brie_builder_options_match_go_backup_and_restore_config() {
    let runtime: Arc<dyn brieRuntime> = Arc::new(MockRuntime::default());
    let mut builder = executorBuilder {
        runtime: runtime.clone(),
        sessionID: 11,
        error: None,
    };
    let mut backup = statement(
        brieKind::Backup,
        vec![
            option(brieOptionType::ChecksumConcurrency, 4, ""),
            option(brieOptionType::IgnoreStats, 1, ""),
            option(brieOptionType::CompressionLevel, 4, ""),
            option(brieOptionType::Compression, 0, "lz4"),
            option(brieOptionType::EncryptionMethod, 0, "aes256-ctr"),
            option(brieOptionType::EncryptionKeyFile, 0, "/tmp/keyfile"),
            option(brieOptionType::LastBackupTSO, 101, ""),
            option(brieOptionType::BackupTSO, 202, ""),
        ],
    );
    let executor = builder
        .buildBRIE(&mut backup)
        .expect("backup config must build");
    let crate::brie::brieExecutor::Main(executor) = executor else {
        panic!("expected main executor")
    };
    let config = executor.backupCfg.expect("backup config");
    assert_eq!(config.common.pdAddresses, ["pd-1:2379", "pd-2:2379"]);
    assert_eq!(config.common.storage, "noop://bucket/prefix");
    assert_eq!(config.common.checksumConcurrency, 4);
    assert!(!config.common.checksum);
    assert_eq!(
        config.common.cipher,
        cipherInfo {
            method: "aes256-ctr".into(),
            key: vec![b'A'; 128]
        }
    );
    assert_eq!(config.common.filterStrings, ["`db`.`t`"]);
    assert!(config.common.caseInsensitiveFilter);
    assert_eq!(config.compression, "lz4");
    assert_eq!(config.compressionLevel, 4);
    assert!(config.ignoreStats);
    assert_eq!(config.lastBackupTS, 101);
    assert_eq!(config.backupTS, 202);
    assert_eq!(backup.storage, "noop://bucket/prefix");

    let mut restore = statement(
        brieKind::Restore,
        vec![
            option(brieOptionType::ChecksumConcurrency, 4, ""),
            option(brieOptionType::WaitTiFlashReady, 1, ""),
            option(brieOptionType::WithSystemTable, 1, ""),
            option(brieOptionType::LoadStats, 1, ""),
            option(brieOptionType::Online, 1, ""),
        ],
    );
    let executor = builder
        .buildBRIE(&mut restore)
        .expect("restore config must build");
    let crate::brie::brieExecutor::Main(executor) = executor else {
        panic!("expected main executor")
    };
    let config = executor.restoreCfg.expect("restore config");
    assert_eq!(config.common.checksumConcurrency, 4);
    assert!(!config.common.checksum);
    assert!(config.waitTiFlashReady);
    assert!(config.withSystemTable);
    assert!(config.loadStats);
    assert!(config.online);
}

#[test]
fn brie_builder_rejects_invalid_go_options() {
    let runtime: Arc<dyn brieRuntime> = Arc::new(MockRuntime::default());
    for (kind, option, expected) in [
        (
            brieKind::Backup,
            option(brieOptionType::EncryptionMethod, 0, "rot13"),
            "unsupported encryption method: rot13",
        ),
        (
            brieKind::Backup,
            option(brieOptionType::Compression, 0, "gzip"),
            "unsupported compression type: gzip",
        ),
    ] {
        let mut builder = executorBuilder {
            runtime: runtime.clone(),
            sessionID: 1,
            error: None,
        };
        let mut statement = statement(kind, vec![option]);
        assert!(builder.buildBRIE(&mut statement).is_none());
        assert_eq!(builder.error.expect("builder error").to_string(), expected);
    }
}
