// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// ExtStorage 功能测试：本地 file URI 读写与全局存储路径探测。
//
// ExtStorage 是 Plan Replayer 等导出场景使用的外部对象/本地文件存储抽象。
// 本模块验证 CRUD、流式读写、批量删除，以及日志目录可写探测失败时回退到临时目录。

#![allow(non_snake_case)]

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use astersql_planner_extstore::{
    Context, GetGlobalExtStorage, NewExtStorage, ProbeFile, ProbeFileSystem,
    SetGlobalExtStorageForTest, SetLocalPathFileSystemForTest, config, vardef,
};
use serial_test::serial;

/// 捕获并在 Drop 时恢复全局 config / CloudStorageURI / ExtStorage 测试钩子。
struct GlobalStateGuard {
    config: config::Config,
    cloud_storage_uri: String,
}

impl GlobalStateGuard {
    /// 快照当前全局配置与云存储 URI。
    fn capture() -> Self {
        Self {
            config: config::get_global_config().as_ref().clone(),
            cloud_storage_uri: vardef::CloudStorageURI.Load(),
        }
    }
}

impl Drop for GlobalStateGuard {
    fn drop(&mut self) {
        // 测试结束还原全局状态，避免串扰后续 #[serial] 用例。
        config::store_global_config(self.config.clone());
        vardef::CloudStorageURI.Store(self.cloud_storage_uri.clone());
        SetGlobalExtStorageForTest(None);
        SetLocalPathFileSystemForTest(None);
    }
}

/// 记录写入内容的探测文件，用于断言可写探测写入了探测字节。
#[derive(Default)]
struct RecordingFile {
    writes: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Write for RecordingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writes.lock().unwrap().push(buf.to_vec());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl ProbeFile for RecordingFile {
    fn close(self: Box<Self>) -> io::Result<()> {
        Ok(())
    }
}

/// 可配置可写性的假文件系统：记录 open/remove 路径与写入内容。
struct RecordingFs {
    writable: bool,
    opened: Mutex<Vec<PathBuf>>,
    removed: Mutex<Vec<PathBuf>>,
    writes: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl RecordingFs {
    /// 构造假 FS；`writable=false` 时 open 返回 PermissionDenied。
    fn new(writable: bool) -> Self {
        Self {
            writable,
            opened: Mutex::new(Vec::new()),
            removed: Mutex::new(Vec::new()),
            writes: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl ProbeFileSystem for RecordingFs {
    fn open_file(&self, path: &Path) -> io::Result<Box<dyn ProbeFile>> {
        self.opened.lock().unwrap().push(path.to_path_buf());
        if !self.writable {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "read-only test filesystem",
            ));
        }
        Ok(Box::new(RecordingFile {
            writes: Arc::clone(&self.writes),
        }))
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.removed.lock().unwrap().push(path.to_path_buf());
        Ok(())
    }
}

/// 配置日志目录与临时目录，并清空云 URI / 全局 ExtStorage。
fn configure_paths(log_dir: &Path, temp_dir: &Path) {
    config::update_global(|conf| {
        conf.log.file.filename = log_dir.join("tidb.log").display().to_string();
        conf.temp_dir = temp_dir.display().to_string();
    });
    vardef::CloudStorageURI.Store("");
    SetGlobalExtStorageForTest(None);
}

/// 本地 file:// ExtStorage：写读删、WalkDir、流式 Create/Open、Rename、批量删除。
#[test]
fn TestExtStorage() {
    let temp_dir = tempfile::tempdir().unwrap();
    let ctx = Context::background();
    let storage = NewExtStorage(
        &ctx,
        &format!("file://{}", temp_dir.path().display()),
        "test_namespace",
    )
    .unwrap();

    // URI 应包含本地根路径与命名空间。
    let uri = storage.URI();
    assert!(uri.contains(&temp_dir.path().display().to_string()));
    assert!(uri.contains("test_namespace"));

    let file_name = "test_file.txt";
    let file_content = b"hello world";
    storage.WriteFile(&ctx, file_name, file_content).unwrap();
    assert_eq!(storage.ReadFile(&ctx, file_name).unwrap(), file_content);
    assert!(storage.FileExists(&ctx, file_name).unwrap());

    // WalkDir 回调应能枚举到刚写入的文件与正确 size。
    let mut found_file = false;
    storage
        .WalkDir(&ctx, None, &mut |path, size| {
            if path == file_name {
                found_file = true;
                assert_eq!(size, file_content.len() as i64);
            }
            Ok(())
        })
        .unwrap();
    assert!(found_file);

    storage.DeleteFile(&ctx, file_name).unwrap();
    assert!(!storage.FileExists(&ctx, file_name).unwrap());

    // 流式写入后再 Open 读回。
    let mut writer = storage.Create(&ctx, "test_writer.txt", None).unwrap();
    writer.write(&ctx, b"test writer").unwrap();
    writer.close(&ctx).unwrap();
    assert_eq!(
        storage.ReadFile(&ctx, "test_writer.txt").unwrap(),
        b"test writer"
    );

    let mut reader = storage.Open(&ctx, "test_writer.txt", None).unwrap();
    let mut buf = [0_u8; 11];
    reader.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"test writer");
    reader.close().unwrap();

    storage
        .Rename(&ctx, "test_writer.txt", "test_writer_renamed.txt")
        .unwrap();
    assert!(!storage.FileExists(&ctx, "test_writer.txt").unwrap());
    assert!(storage.FileExists(&ctx, "test_writer_renamed.txt").unwrap());

    // DeleteFiles 批量删除。
    storage.WriteFile(&ctx, "file1", b"1").unwrap();
    storage.WriteFile(&ctx, "file2", b"2").unwrap();
    storage
        .DeleteFiles(&ctx, &["file1".to_owned(), "file2".to_owned()])
        .unwrap();
    assert!(!storage.FileExists(&ctx, "file1").unwrap());
    assert!(!storage.FileExists(&ctx, "file2").unwrap());

    let uri = storage.URI();
    assert!(uri.contains(&temp_dir.path().display().to_string()));
    assert!(uri.contains("test_namespace"));
    storage.Close();
}

/// 日志目录可写时，全局 ExtStorage 落在 log_dir/replayer，并完成探测写删。
#[test]
#[serial]
fn TestGetLocalPathDirNameWithWritePerm() {
    let _guard = GlobalStateGuard::capture();
    let temp_dir = tempfile::tempdir().unwrap();
    let log_dir = temp_dir.path().join("var/log/tidb");
    let fallback_dir = temp_dir.path().join("tmp/tidb");
    configure_paths(&log_dir, &fallback_dir);

    let fs = Arc::new(RecordingFs::new(true));
    SetLocalPathFileSystemForTest(Some(fs.clone()));
    let storage = GetGlobalExtStorage(&Context::background()).unwrap();

    assert!(storage.URI().contains(&log_dir.display().to_string()));
    let opened = fs.opened.lock().unwrap();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].parent().unwrap(), log_dir.join("replayer"));
    assert_eq!(&*fs.writes.lock().unwrap(), &[vec![0]]);
    assert_eq!(&*fs.removed.lock().unwrap(), &*opened);
    storage.Close();
}

/// 日志目录不可写时，全局 ExtStorage 回退到 temp_dir，且不删除探测文件。
#[test]
#[serial]
fn TestGetLocalPathDirNameWithoutWritePerm() {
    let _guard = GlobalStateGuard::capture();
    let temp_dir = tempfile::tempdir().unwrap();
    let log_dir = temp_dir.path().join("var/log/tidb");
    let fallback_dir = temp_dir.path().join("tmp/tidb");
    configure_paths(&log_dir, &fallback_dir);

    let fs = Arc::new(RecordingFs::new(false));
    SetLocalPathFileSystemForTest(Some(fs.clone()));
    let storage = GetGlobalExtStorage(&Context::background()).unwrap();

    assert!(storage.URI().contains(&fallback_dir.display().to_string()));
    assert_eq!(fs.opened.lock().unwrap().len(), 1);
    assert!(fs.removed.lock().unwrap().is_empty());
    storage.Close();
}

/// 可写探测成功时 GetGlobalExtStorage 选择日志目录。
#[test]
#[serial]
fn TestGetGlobalExtStorageWithWritePerm() {
    let _guard = GlobalStateGuard::capture();
    let temp_dir = tempfile::tempdir().unwrap();
    let log_dir = temp_dir.path().join("log");
    let fallback_dir = temp_dir.path().join("tmp");
    configure_paths(&log_dir, &fallback_dir);

    let fs = Arc::new(RecordingFs::new(true));
    SetLocalPathFileSystemForTest(Some(fs));
    let storage = GetGlobalExtStorage(&Context::background()).unwrap();

    assert!(storage.URI().contains(&log_dir.display().to_string()));
    storage.Close();
}

/// 可写探测失败时 GetGlobalExtStorage 回退到临时目录。
#[test]
#[serial]
fn TestGetGlobalExtStorageWithoutWritePerm() {
    let _guard = GlobalStateGuard::capture();
    let temp_dir = tempfile::tempdir().unwrap();
    let log_dir = temp_dir.path().join("readonly");
    let fallback_dir = temp_dir.path().join("tmp");
    configure_paths(&log_dir, &fallback_dir);

    let fs = Arc::new(RecordingFs::new(false));
    SetLocalPathFileSystemForTest(Some(fs));
    let storage = GetGlobalExtStorage(&Context::background()).unwrap();

    assert!(storage.URI().contains(&fallback_dir.display().to_string()));
    storage.Close();
}
