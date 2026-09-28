// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// Watcher 功能单测。
//
// 以临时目录模拟 binlog 文件滚动：覆盖 Create/Modify/Chmod/Rename/Remove/Move
// 完整事件链，并通过 `assert_event` 在超时内等待精确匹配。

use super::{Chmod, Create, Modify, Move, NewWatcher, Op, Remove, Rename, Watcher};
use crossbeam_channel::{TryRecvError, after};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

/// 端到端验证监视器对常见文件操作序列的事件投递。
#[test]
fn test_watcher() {
    let old_file_name = "mysql-bin.000001";
    let new_file_name = "mysql-bin.000002";

    let dir = tempfile::tempdir().expect("create first temporary directory");
    let old_file_path = dir.path().join(old_file_name);
    let new_file_path = dir.path().join(new_file_name);

    let watcher = NewWatcher();
    watcher.Add(dir.path()).expect("watch first directory");
    watcher
        .Start(Duration::from_millis(10))
        .expect("start watcher");

    // Create → Modify → Chmod → Rename → Remove。
    fs::File::create(&old_file_path).expect("create old file");
    assert_event(&watcher, &old_file_path, Create);

    OpenOptions::new()
        .write(true)
        .open(&old_file_path)
        .expect("open old file")
        .write_all(b"meaningless content")
        .expect("write old file");
    assert_event(&watcher, &old_file_path, Modify);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&old_file_path, fs::Permissions::from_mode(0o777))
            .expect("chmod old file");
        assert_event(&watcher, &old_file_path, Chmod);
    }
    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(&old_file_path)
            .expect("read old file metadata")
            .permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&old_file_path, permissions).expect("chmod old file");
        assert_event(&watcher, &old_file_path, Chmod);
    }

    fs::rename(&old_file_path, &new_file_path).expect("rename old file");
    assert_event(&watcher, &old_file_path, Rename);

    fs::remove_file(&new_file_path).expect("remove new file");
    assert_event(&watcher, &new_file_path, Remove);

    // 重建后跨两个已监视目录移动 → Move。
    fs::File::create(&old_file_path).expect("create old file again");
    assert_event(&watcher, &old_file_path, Create);

    let dir2 = tempfile::tempdir().expect("create second temporary directory");
    let old_file_path2 = dir2.path().join(old_file_name);
    watcher.Add(dir2.path()).expect("watch second directory");

    fs::rename(&old_file_path, &old_file_path2).expect("move file across watched directories");
    assert_event(&watcher, &old_file_path, Move);

    watcher.Close();
}

/// Go 的 Close 会关闭 Events 与 Errors；Rust 接收端也必须立即呈现断开状态。
#[test]
fn close_disconnects_event_and_error_channels() {
    let watcher = NewWatcher();
    watcher
        .Start(Duration::from_millis(10))
        .expect("start watcher");

    watcher.Close();

    assert!(matches!(
        watcher.Events.try_recv(),
        Err(TryRecvError::Disconnected)
    ));
    assert!(matches!(
        watcher.Errors.try_recv(),
        Err(TryRecvError::Disconnected)
    ));
}

/// 在超时内等待匹配路径与操作的文件事件；目录事件跳过，错误则 panic。
fn assert_event(watcher: &Watcher, path: &Path, op: Op) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let timeout = after(deadline.saturating_duration_since(Instant::now()));
        crossbeam_channel::select! {
            recv(watcher.Events) -> event => {
                let event = event.expect("watcher event channel closed");
                if event.IsDirEvent() {
                    continue;
                }
                assert!(event.HasOps(&[op]), "unexpected operation for {}: {}", path.display(), event.Op);
                assert_eq!(event.Path, path, "unexpected event path");
                return;
            }
            recv(watcher.Errors) -> error => {
                panic!("watcher error: {:?}", error.expect("watcher error channel closed"));
            }
            recv(timeout) -> _ => {
                panic!("timed out waiting for operation {op} on {}", path.display());
            }
        }
    }
}
