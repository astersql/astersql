// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// EngineManager 生命周期与外部引擎查询单元测试。
//
// 覆盖 open/close/reset/cleanup、未知外部引擎统计为 None，以及
// cleanupAllLocalEngines 清空本地 file size 视图。

#![allow(dead_code)]
#![allow(non_snake_case)]

use std::sync::Arc;

use crate::engine_mgr::{StoreHelper, newEngineManager};
use crate::local::BackendConfig;
use crate::{CancellationToken, EngineId, Result};

/// 测试用 StoreHelper：固定返回 (42, 7) TSO 与 api-v2 codec。
struct TestStoreHelper;

impl StoreHelper for TestStoreHelper {
    fn GetTS(&self, token: &CancellationToken) -> Result<(i64, i64)> {
        token.check()?;
        Ok((42, 7))
    }

    fn GetTiKVCodec(&self) -> String {
        "api-v2".to_owned()
    }
}

/// Like Go's t.TempDir, reclaim fixture storage even when a test panics.
struct BackendDirectory(std::path::PathBuf);

impl Drop for BackendDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Atomically reserve a directory, including across test processes and PID reuse.
fn getBackendConfig() -> (BackendConfig, BackendDirectory) {
    let directory = loop {
        let path = std::env::temp_dir().join(format!(
            "astersql-ingestctrl-{}-{}",
            std::process::id(),
            EngineId::new()
        ));
        match std::fs::create_dir(&path) {
            Ok(()) => break BackendDirectory(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!(
                "cannot create backend directory {}: {error}",
                path.display()
            ),
        }
    };
    let mut config = BackendConfig::default();
    config.local_store_dir = directory.0.to_string_lossy().into_owned();
    config.worker_concurrency = 8;
    (config, directory)
}

// TestEngineManager 对应 Go 的同名测试：覆盖 open、close、reset、cleanup 和本地目录清空。
/// 覆盖 open 分配 TSO、close、未知 ID 失败、reset、cleanup 与 codec 透传。
#[test]
pub fn TestEngineManager() {
    let (config, _directory) = getBackendConfig();
    let path = config.local_store_dir.clone();
    let manager = newEngineManager(config, Arc::new(TestStoreHelper)).unwrap();
    let token = CancellationToken::default();
    let engine_id = EngineId::new();
    let engine = manager.openEngine(&token, engine_id, 96, 32).unwrap();
    // open 后应已写入非零合成 TSO
    assert_ne!(
        0,
        engine
            .engine_meta
            .ts
            .load(std::sync::atomic::Ordering::Acquire)
    );
    assert_eq!(1, manager.engineFileSizes().len());
    manager.closeEngine(engine_id, false).unwrap();
    assert_eq!(0, manager.getImportedKVCount(engine_id));
    assert!(manager.closeEngine(EngineId::new(), false).is_err());
    manager.resetEngine(&token, engine_id, false).unwrap();
    manager.cleanupEngine(EngineId::new()).unwrap();
    manager.cleanupEngine(engine_id).unwrap();
    assert!(manager.engineFileSizes().is_empty());
    assert_eq!("api-v2", manager.GetTiKVCodec());
    manager.close();
    assert!(!std::path::Path::new(&path).exists());
}

/// 与 Go 的 LoadOrStore/open 语义一致：重复打开同一 ID 不新增引擎也不报错。
#[test]
pub fn open_engine_is_idempotent_for_an_existing_id() {
    let (config, _directory) = getBackendConfig();
    let path = config.local_store_dir.clone();
    let manager = newEngineManager(config, Arc::new(TestStoreHelper)).unwrap();
    let token = CancellationToken::default();
    let engine_id = EngineId::new();

    let first = manager.openEngine(&token, engine_id, 96, 32).unwrap();
    let second = manager.openEngine(&token, engine_id, 96, 32).unwrap();

    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(1, manager.engineFileSizes().len());
    manager.close();
    assert!(!std::path::Path::new(&path).exists());
}

/// Go 的 resetEngine 对未知本地/外部引擎记录告警后返回成功。
#[test]
pub fn reset_missing_engine_is_a_noop() {
    let (config, _directory) = getBackendConfig();
    let path = config.local_store_dir.clone();
    let manager = newEngineManager(config, Arc::new(TestStoreHelper)).unwrap();

    manager
        .resetEngine(&CancellationToken::default(), EngineId::new(), true)
        .unwrap();

    manager.close();
    assert!(!std::path::Path::new(&path).exists());
}

// TestGetExternalEngineKVStatistics 对应 Go 的同名测试：未知外部引擎返回零统计。
/// 未注册外部引擎时 getExternalEngineKVStatistics 返回 None。
#[test]
pub fn TestGetExternalEngineKVStatistics() {
    let (config, _directory) = getBackendConfig();
    let path = config.local_store_dir.clone();
    let manager = newEngineManager(config, Arc::new(TestStoreHelper)).unwrap();
    assert_eq!(None, manager.getExternalEngineKVStatistics(EngineId::new()));
    manager.close();
    assert!(!std::path::Path::new(&path).exists());
}

// TestCleanupAllLocalEnginesLogsErrorOnly 对应 Go 测试：cleanupAllLocalEngines 记录错误但清空 file size 视图。
/// 打开多个引擎后 cleanupAllLocalEngines 应清空 engineFileSizes。
#[test]
pub fn TestCleanupAllLocalEnginesLogsErrorOnly() {
    let (config, _directory) = getBackendConfig();
    let path = config.local_store_dir.clone();
    let manager = newEngineManager(config, Arc::new(TestStoreHelper)).unwrap();
    let token = CancellationToken::default();
    manager.openEngine(&token, EngineId::new(), 64, 16).unwrap();
    manager.openEngine(&token, EngineId::new(), 64, 16).unwrap();
    manager.cleanupAllLocalEngines();
    assert!(manager.engineFileSizes().is_empty());
    manager.close();
    assert!(!std::path::Path::new(&path).exists());
}

/// Independent test processes must not share or remove each other's backend directory.
#[test]
fn cross_process_engine_directories_are_isolated() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, Command, Stdio};

    const CHILD_ENV: &str = "ASTERSQL_ENGINE_DIRECTORY_CHILD";
    const PATH_PREFIX: &str = "ENGINE_DIRECTORY=";
    if std::env::var_os(CHILD_ENV).is_some() {
        let (config, _directory) = getBackendConfig();
        let path = config.local_store_dir.clone();
        let manager = newEngineManager(config, Arc::new(TestStoreHelper)).unwrap();
        manager
            .openEngine(&CancellationToken::default(), EngineId::new(), 96, 32)
            .unwrap();
        println!("{PATH_PREFIX}{path}");
        std::io::stdout().flush().unwrap();
        let mut release = String::new();
        std::io::stdin().read_line(&mut release).unwrap();
        manager.close();
        assert!(!std::path::Path::new(&path).exists());
        return;
    }

    // Reap children even if a setup assertion fails, so no backend outlives the test.
    struct TestProcess(Child);
    impl Drop for TestProcess {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let spawn = || {
        let mut process = TestProcess(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "engine_mgr_test::cross_process_engine_directories_are_isolated",
                    "--nocapture",
                ])
                .env(CHILD_ENV, "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let path = BufReader::new(process.0.stdout.as_mut().unwrap())
            .lines()
            .map(|line| line.unwrap())
            .find_map(|line| {
                line.split_once(PATH_PREFIX)
                    .map(|(_, path)| path.to_owned())
            })
            .expect("child must report its open backend directory");
        (process, path)
    };
    let (mut first, first_path) = spawn();
    let (mut second, second_path) = spawn();
    first.0.stdin.take().unwrap().write_all(b"\n").unwrap();
    let first_status = first.0.wait().unwrap();
    let second_survived = std::path::Path::new(&second_path).is_dir();
    second.0.stdin.take().unwrap().write_all(b"\n").unwrap();
    let second_status = second.0.wait().unwrap();

    assert!(first_status.success());
    assert!(second_status.success());
    assert_ne!(
        first_path, second_path,
        "processes must own distinct directories"
    );
    assert!(
        second_survived,
        "closing one manager removed the other's directory"
    );
    assert!(!std::path::Path::new(&first_path).exists());
    assert!(!std::path::Path::new(&second_path).exists());
}
