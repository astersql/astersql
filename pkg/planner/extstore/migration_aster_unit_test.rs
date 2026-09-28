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

// ExtStorage 迁移对齐测试：本地操作序列与全局存储选路/缓存。
//
// 与 `extstore_test` 类似，但侧重 classic/nextgen 下 CloudStorageURI 与日志
// replayer 探测行为差异，以及全局实例指针缓存。

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::root_feature_extstore::{
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

/// 记录写入内容的探测文件。
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

/// 可配置可写性的假文件系统，用于断言路径探测。
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

/// 本地存储写读删、WalkDir、流式 IO、Rename、批量删除与 Go 行为一致。
#[test]
fn local_storage_operations_match_go_behavior() {
    let root = tempfile::tempdir().unwrap();
    let ctx = Context::background();
    let storage = NewExtStorage(
        &ctx,
        &format!("file://{}", root.path().display()),
        "test_namespace",
    )
    .unwrap();

    let uri = storage.URI();
    assert!(uri.contains(&root.path().display().to_string()));
    assert!(uri.contains("test_namespace"));

    storage
        .WriteFile(&ctx, "test_file.txt", b"hello world")
        .unwrap();
    assert_eq!(
        storage.ReadFile(&ctx, "test_file.txt").unwrap(),
        b"hello world"
    );
    assert!(storage.FileExists(&ctx, "test_file.txt").unwrap());

    let mut found = false;
    storage
        .WalkDir(&ctx, None, &mut |path, size| {
            if path == "test_file.txt" {
                found = true;
                assert_eq!(size, 11);
            }
            Ok(())
        })
        .unwrap();
    assert!(found);

    storage.DeleteFile(&ctx, "test_file.txt").unwrap();
    assert!(!storage.FileExists(&ctx, "test_file.txt").unwrap());

    let mut writer = storage.Create(&ctx, "test_writer.txt", None).unwrap();
    writer.write(&ctx, b"test writer").unwrap();
    writer.close(&ctx).unwrap();

    let mut reader = storage.Open(&ctx, "test_writer.txt", None).unwrap();
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"test writer");
    reader.close().unwrap();

    storage
        .Rename(&ctx, "test_writer.txt", "test_writer_renamed.txt")
        .unwrap();
    assert!(!storage.FileExists(&ctx, "test_writer.txt").unwrap());
    assert!(storage.FileExists(&ctx, "test_writer_renamed.txt").unwrap());

    storage.WriteFile(&ctx, "file1", b"1").unwrap();
    storage.WriteFile(&ctx, "file2", b"2").unwrap();
    storage
        .DeleteFiles(&ctx, &["file1".to_owned(), "file2".to_owned()])
        .unwrap();
    assert!(!storage.FileExists(&ctx, "file1").unwrap());
    assert!(!storage.FileExists(&ctx, "file2").unwrap());
    storage.Close();
}

/// Go filepath.Join 不会因后续 namespace 带根分隔符而丢弃已有存储路径。
#[test]
fn absolute_looking_namespace_stays_beneath_storage_root() {
    let root = tempfile::tempdir().unwrap();
    let storage = NewExtStorage(
        &Context::background(),
        &format!("file://{}", root.path().display()),
        "/test_namespace",
    )
    .unwrap();

    let uri = storage.URI();
    assert!(uri.contains(&root.path().display().to_string()));
    assert!(uri.ends_with("/test_namespace"));
    storage.Close();
}

/// Go filepath.Abs works for non-existent relative paths; canonicalize must not be required.
#[test]
#[serial]
fn relative_local_fallback_is_made_absolute_without_existing_on_disk() {
    let _guard = GlobalStateGuard::capture();
    let fallback = tempfile::tempdir().unwrap();
    config::update_global(|conf| {
        conf.log.file.filename = "relative-log/tidb.log".to_owned();
        conf.temp_dir = fallback.path().display().to_string();
        conf.keyspace_name.clear();
    });
    vardef::CloudStorageURI.Store("");
    let fs = Arc::new(RecordingFs::new(true));
    SetLocalPathFileSystemForTest(Some(fs));
    SetGlobalExtStorageForTest(None);

    let storage = GetGlobalExtStorage(&Context::background()).unwrap();
    let expected = std::env::current_dir().unwrap().join("relative-log");
    assert!(storage.URI().contains(&expected.display().to_string()));
    storage.Close();
}

/// nextgen 优先云 URI；classic 探测可写日志 replayer，且全局实例被缓存。
#[test]
#[serial]
fn global_storage_prefers_writable_log_replayer_and_is_cached() {
    let _guard = GlobalStateGuard::capture();
    let root = tempfile::tempdir().unwrap();
    let log_dir = root.path().join("log");
    let temp_dir = root.path().join("tmp");
    std::fs::create_dir_all(&log_dir).unwrap();

    config::update_global(|conf| {
        conf.log.file.filename = log_dir.join("tidb.log").display().to_string();
        conf.temp_dir = temp_dir.display().to_string();
    });
    // nextgen 使用 file:// 云路径；classic 下 s3 URI 应被忽略。
    vardef::CloudStorageURI.Store(if cfg!(feature = "nextgen") {
        format!("file://{}", root.path().join("cloud").display())
    } else {
        "s3://must-be-ignored-in-classic".to_owned()
    });

    let fs = Arc::new(RecordingFs::new(true));
    SetLocalPathFileSystemForTest(Some(fs.clone()));
    SetGlobalExtStorageForTest(None);

    let ctx = Context::background();
    let first = GetGlobalExtStorage(&ctx).unwrap();
    let second = GetGlobalExtStorage(&ctx).unwrap();
    // 两次获取应返回同一 Arc 实例（全局缓存）。
    assert!(Arc::ptr_eq(&first, &second));

    let opened = fs.opened.lock().unwrap();
    if cfg!(feature = "nextgen") {
        assert!(
            first
                .URI()
                .contains(&root.path().join("cloud").display().to_string())
        );
        assert!(opened.is_empty());
        assert!(fs.writes.lock().unwrap().is_empty());
        assert!(fs.removed.lock().unwrap().is_empty());
    } else {
        assert!(first.URI().contains(&log_dir.display().to_string()));
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].parent().unwrap(), log_dir.join("replayer"));
        assert_eq!(&*fs.writes.lock().unwrap(), &[vec![0]]);
        assert_eq!(&*fs.removed.lock().unwrap(), &*opened);
    }
}

/// 探测 open 失败时全局存储回退到 temp_dir，且不删除探测路径。
#[test]
#[serial]
fn global_storage_falls_back_to_temp_when_probe_cannot_open() {
    let _guard = GlobalStateGuard::capture();
    let root = tempfile::tempdir().unwrap();
    let log_dir = root.path().join("readonly");
    let temp_dir = root.path().join("tmp");

    config::update_global(|conf| {
        conf.log.file.filename = log_dir.join("tidb.log").display().to_string();
        conf.temp_dir = temp_dir.display().to_string();
    });
    vardef::CloudStorageURI.Store("");

    let fs = Arc::new(RecordingFs::new(false));
    SetLocalPathFileSystemForTest(Some(fs.clone()));
    SetGlobalExtStorageForTest(None);

    let storage = GetGlobalExtStorage(&Context::background()).unwrap();
    assert!(storage.URI().contains(&temp_dir.display().to_string()));
    assert_eq!(fs.opened.lock().unwrap().len(), 1);
    assert!(fs.removed.lock().unwrap().is_empty());
}
