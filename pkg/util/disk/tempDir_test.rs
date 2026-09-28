// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 临时目录删除后并发重建的单元测试。
//
// 对应 Go `TestRemoveDir`：目录被清空后多线程同时 `CheckAndInitTempDir`，
// 最终路径应再次存在。

use super::*;
use astersql_config::{get_global_config, restore_func, update_global};
use std::fs;
use std::io;
use std::path::Path;
use std::thread;

/// 测试结束时清理锁并恢复全局配置。
struct ConfigRestore<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> Drop for ConfigRestore<F> {
    fn drop(&mut self) {
        CleanUp();
        (self.0.take().expect("config restore closure missing"))();
    }
}

/// Unix 下以 0755 递归建目录（与产品路径 0750 区分，仅测试夹具用）。
#[cfg(unix)]
fn create_dir_all_0755(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(path)
}

#[cfg(not(unix))]
fn create_dir_all_0755(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

/// 删掉已初始化目录后并发重建，验证探测与初始化仍正确。
#[test]
#[serial_test::serial(temp_dir)]
fn TestRemoveDir() {
    let temp_dir = tempfile::tempdir().expect("create test temp directory");
    let path = temp_dir.path().to_path_buf();

    let restore = restore_func();
    update_global(|config| config.temp_storage_path = path.to_string_lossy().into_owned());
    let _restore = ConfigRestore(Some(restore));

    // Clean the uncleared temp files from the last run, then recreate the path.
    // 清掉上次残留后重建路径，再做一次正常初始化作为基线。
    fs::remove_dir_all(&path).expect("remove initial temp directory");
    create_dir_all_0755(&path).expect("recreate initial temp directory");

    CheckAndInitTempDir().expect("initialize existing temp directory");
    assert!(checkTempDirExist());

    // 整目录删除后探测应为 false，再并发 CheckAndInitTempDir 重建。
    fs::remove_dir_all(&get_global_config().temp_storage_path)
        .expect("remove configured temp directory");
    assert!(!checkTempDirExist());

    let workers: Vec<_> = (0..10)
        .map(|_| thread::spawn(CheckAndInitTempDir))
        .collect();
    for worker in workers {
        worker
            .join()
            .expect("temp directory worker panicked")
            .expect("temp directory worker failed");
    }

    CheckAndInitTempDir().expect("recheck initialized temp directory");
    assert!(checkTempDirExist());
}

#[cfg(unix)]
#[test]
#[serial_test::serial(temp_dir)]
fn initialize_existing_file_matches_go_not_a_directory_error() {
    let root = tempfile::tempdir().expect("create test root");
    let temp_path = root.path().join("configured-as-file");
    fs::write(&temp_path, b"not a directory").expect("create configured file");

    let restore = restore_func();
    update_global(|config| config.temp_storage_path = temp_path.to_string_lossy().into_owned());
    let _restore = ConfigRestore(Some(restore));

    let err = InitializeTempDir().expect_err("a file cannot contain the directory lock");
    assert_eq!(err.kind(), io::ErrorKind::NotADirectory);
}
